//! The Whisper encoder-decoder forward pass in Burn, loaded straight from a
//! whisper.cpp GGML file (see [`crate::ggml`]). Ported against the spec
//! extracted from whisper.cpp (vendored in whisper-rs-sys 0.15): pre-LN
//! transformer, LayerNorm eps 1e-5 with biased variance, tanh-approximation
//! GELU, key projections without bias, tied token-embedding logit head, and
//! stored (not computed) positional embeddings. whisper.cpp splits its
//! attention scaling across q/k/cached-k differently per attention type, but
//! every variant multiplies out to softmax(QKᵀ·d^-1/2) — implemented here
//! uniformly in f32.
//!
//! Training never needs this to DECODE well — decoding parity against
//! whisper-rs is the correctness gate that proves the forward math before any
//! gradient is trusted.

use burn::tensor::backend::Backend;
use burn::tensor::ops::ConvOptions;
use burn::tensor::{activation, module, Int, Tensor, TensorData};

use crate::ggml::{GgmlError, GgmlModel, GgmlTensor};
use crate::lora::AttentionLora;

/// The id of the " " token, suppressed on the first sampled step.
pub const TOKEN_BLANK: i32 = 220;

/// Whisper's special-token ids depend on the vocabulary: English-only models
/// (n_vocab 51864) use whisper.cpp's defaults; multilingual models insert
/// their language tokens right after `sot`, shifting everything later by
/// `num_languages - 98` (one, for the standard 99-language models).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialTokens {
    pub multilingual: bool,
    pub num_languages: i32,
    pub eot: i32,
    pub sot: i32,
    pub transcribe: i32,
    pub no_timestamps: i32,
}

impl SpecialTokens {
    #[must_use]
    pub fn for_vocab(n_vocab: usize) -> Self {
        let multilingual = n_vocab >= 51_865;
        let num_languages = n_vocab as i32 - 51_765 - i32::from(multilingual);
        let shift = if multilingual { num_languages - 98 } else { 0 };
        Self {
            multilingual,
            num_languages,
            eot: 50_256 + i32::from(multilingual),
            sot: 50_257 + i32::from(multilingual),
            transcribe: 50_358 + shift,
            no_timestamps: 50_362 + shift,
        }
    }

    /// The id of the `index`-th language token (0 = English).
    #[must_use]
    pub fn language(&self, index: i32) -> i32 {
        self.sot + 1 + index
    }

    /// The teacher-forcing/decode prompt for plain English transcription with
    /// timestamps off — whisper.cpp's initial sequence for these settings.
    #[must_use]
    pub fn transcription_prompt(&self) -> Vec<i32> {
        if self.multilingual {
            vec![
                self.sot,
                self.language(0),
                self.transcribe,
                self.no_timestamps,
            ]
        } else {
            vec![self.sot, self.no_timestamps]
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WhisperDims {
    pub n_vocab: usize,
    pub n_audio_ctx: usize,
    pub n_audio_state: usize,
    pub n_audio_head: usize,
    pub n_audio_layer: usize,
    pub n_text_ctx: usize,
    pub n_text_state: usize,
    pub n_text_head: usize,
    pub n_text_layer: usize,
    pub n_mels: usize,
}

struct Linear<B: Backend> {
    /// Pre-transposed to `[in, out]` so forward is a plain matmul.
    weight_t: Tensor<B, 2>,
    bias: Option<Tensor<B, 1>>,
}

impl<B: Backend> Linear<B> {
    fn forward(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        let y = x.matmul(self.weight_t.clone());
        match &self.bias {
            Some(bias) => y + bias.clone().unsqueeze(),
            None => y,
        }
    }
}

struct LayerNorm<B: Backend> {
    weight: Tensor<B, 1>,
    bias: Tensor<B, 1>,
}

impl<B: Backend> LayerNorm<B> {
    /// whisper.cpp `ggml_norm`: biased variance over the channel dim, eps 1e-5.
    fn forward(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        let mean = x.clone().mean_dim(1);
        let centered = x - mean;
        let variance = centered.clone().powf_scalar(2.0).mean_dim(1);
        let normalized = centered / (variance + 1e-5).sqrt();
        normalized * self.weight.clone().unsqueeze() + self.bias.clone().unsqueeze()
    }
}

struct Attention<B: Backend> {
    query: Linear<B>,
    key: Linear<B>,
    value: Linear<B>,
    out: Linear<B>,
}

struct EncoderBlock<B: Backend> {
    attn_ln: LayerNorm<B>,
    attn: Attention<B>,
    mlp_ln: LayerNorm<B>,
    mlp_up: Linear<B>,
    mlp_down: Linear<B>,
}

struct DecoderBlock<B: Backend> {
    attn_ln: LayerNorm<B>,
    attn: Attention<B>,
    cross_attn_ln: LayerNorm<B>,
    cross_attn: Attention<B>,
    mlp_ln: LayerNorm<B>,
    mlp_up: Linear<B>,
    mlp_down: Linear<B>,
}

pub struct WhisperRuntime<B: Backend> {
    pub dims: WhisperDims,
    device: B::Device,
    conv1_weight: Tensor<B, 3>,
    conv1_bias: Tensor<B, 1>,
    conv2_weight: Tensor<B, 3>,
    conv2_bias: Tensor<B, 1>,
    encoder_positional: Tensor<B, 2>,
    encoder_blocks: Vec<EncoderBlock<B>>,
    ln_post: LayerNorm<B>,
    token_embedding: Tensor<B, 2>,
    decoder_positional: Tensor<B, 2>,
    decoder_blocks: Vec<DecoderBlock<B>>,
    decoder_ln: LayerNorm<B>,
}

impl<B: Backend> WhisperRuntime<B> {
    pub fn load(model: &GgmlModel, device: &B::Device) -> Result<Self, GgmlError> {
        let h = &model.hparams;
        let dims = WhisperDims {
            n_vocab: h.n_vocab as usize,
            n_audio_ctx: h.n_audio_ctx as usize,
            n_audio_state: h.n_audio_state as usize,
            n_audio_head: h.n_audio_head as usize,
            n_audio_layer: h.n_audio_layer as usize,
            n_text_ctx: h.n_text_ctx as usize,
            n_text_state: h.n_text_state as usize,
            n_text_head: h.n_text_head as usize,
            n_text_layer: h.n_text_layer as usize,
            n_mels: h.n_mels as usize,
        };

        let tensor_1d = |name: &str| -> Result<Tensor<B, 1>, GgmlError> {
            let raw = model.tensor(name)?;
            Ok(Tensor::from_data(
                TensorData::new(raw.to_f32()?, [raw.element_count()]),
                device,
            ))
        };
        // ggml stores a matrix as ne=[in, out] with `in` fastest, i.e.
        // row-major [out][in] — the PyTorch Linear convention. Pre-transpose
        // to [in, out] once at load.
        let linear_t = |raw: &GgmlTensor| -> Result<Tensor<B, 2>, GgmlError> {
            let rows = raw.dims[1] as usize; // out
            let cols = raw.dims[0] as usize; // in
            let tensor: Tensor<B, 2> =
                Tensor::from_data(TensorData::new(raw.to_f32()?, [rows, cols]), device);
            Ok(tensor.transpose())
        };
        let linear = |name: &str, bias: Option<&str>| -> Result<Linear<B>, GgmlError> {
            Ok(Linear {
                weight_t: linear_t(model.tensor(name)?)?,
                bias: bias.map(tensor_1d).transpose()?,
            })
        };
        let layer_norm = |prefix: &str| -> Result<LayerNorm<B>, GgmlError> {
            Ok(LayerNorm {
                weight: tensor_1d(&format!("{prefix}.weight"))?,
                bias: tensor_1d(&format!("{prefix}.bias"))?,
            })
        };
        let attention = |prefix: &str| -> Result<Attention<B>, GgmlError> {
            Ok(Attention {
                query: linear(
                    &format!("{prefix}.query.weight"),
                    Some(&format!("{prefix}.query.bias")),
                )?,
                // Whisper's key projection has no bias.
                key: linear(&format!("{prefix}.key.weight"), None)?,
                value: linear(
                    &format!("{prefix}.value.weight"),
                    Some(&format!("{prefix}.value.bias")),
                )?,
                out: linear(
                    &format!("{prefix}.out.weight"),
                    Some(&format!("{prefix}.out.bias")),
                )?,
            })
        };
        let conv = |name: &str| -> Result<Tensor<B, 3>, GgmlError> {
            let raw = model.tensor(name)?;
            // ne = [kernel, in, out] with kernel fastest → row-major
            // [out][in][kernel], exactly Burn's conv1d weight layout.
            let (kernel, channels_in, channels_out) = (
                raw.dims[0] as usize,
                raw.dims[1] as usize,
                raw.dims[2] as usize,
            );
            Ok(Tensor::from_data(
                TensorData::new(raw.to_f32()?, [channels_out, channels_in, kernel]),
                device,
            ))
        };
        let positional =
            |name: &str, rows: usize, cols: usize| -> Result<Tensor<B, 2>, GgmlError> {
                let raw = model.tensor(name)?;
                Ok(Tensor::from_data(
                    TensorData::new(raw.to_f32()?, [rows, cols]),
                    device,
                ))
            };

        let mut encoder_blocks = Vec::with_capacity(dims.n_audio_layer);
        for layer in 0..dims.n_audio_layer {
            let p = format!("encoder.blocks.{layer}");
            encoder_blocks.push(EncoderBlock {
                attn_ln: layer_norm(&format!("{p}.attn_ln"))?,
                attn: attention(&format!("{p}.attn"))?,
                mlp_ln: layer_norm(&format!("{p}.mlp_ln"))?,
                mlp_up: linear(
                    &format!("{p}.mlp.0.weight"),
                    Some(&format!("{p}.mlp.0.bias")),
                )?,
                mlp_down: linear(
                    &format!("{p}.mlp.2.weight"),
                    Some(&format!("{p}.mlp.2.bias")),
                )?,
            });
        }
        let mut decoder_blocks = Vec::with_capacity(dims.n_text_layer);
        for layer in 0..dims.n_text_layer {
            let p = format!("decoder.blocks.{layer}");
            decoder_blocks.push(DecoderBlock {
                attn_ln: layer_norm(&format!("{p}.attn_ln"))?,
                attn: attention(&format!("{p}.attn"))?,
                cross_attn_ln: layer_norm(&format!("{p}.cross_attn_ln"))?,
                cross_attn: attention(&format!("{p}.cross_attn"))?,
                mlp_ln: layer_norm(&format!("{p}.mlp_ln"))?,
                mlp_up: linear(
                    &format!("{p}.mlp.0.weight"),
                    Some(&format!("{p}.mlp.0.bias")),
                )?,
                mlp_down: linear(
                    &format!("{p}.mlp.2.weight"),
                    Some(&format!("{p}.mlp.2.bias")),
                )?,
            });
        }

        let token_embedding_raw = model.tensor("decoder.token_embedding.weight")?;
        Ok(Self {
            device: device.clone(),
            conv1_weight: conv("encoder.conv1.weight")?,
            conv1_bias: tensor_1d("encoder.conv1.bias")?,
            conv2_weight: conv("encoder.conv2.weight")?,
            conv2_bias: tensor_1d("encoder.conv2.bias")?,
            encoder_positional: positional(
                "encoder.positional_embedding",
                dims.n_audio_ctx,
                dims.n_audio_state,
            )?,
            encoder_blocks,
            ln_post: layer_norm("encoder.ln_post")?,
            token_embedding: Tensor::from_data(
                TensorData::new(
                    token_embedding_raw.to_f32()?,
                    [dims.n_vocab, dims.n_text_state],
                ),
                device,
            ),
            decoder_positional: positional(
                "decoder.positional_embedding",
                dims.n_text_ctx,
                dims.n_text_state,
            )?,
            decoder_blocks,
            decoder_ln: layer_norm("decoder.ln")?,
            dims,
        })
    }

    /// Encodes one 30 s mel window (`n_mels × 3000`, frame index fastest) to
    /// `[n_audio_ctx, n_audio_state]`.
    pub fn encode(&self, mel: &[f32]) -> Tensor<B, 2> {
        let frames = mel.len() / self.dims.n_mels;
        let x: Tensor<B, 3> = Tensor::from_data(
            TensorData::new(mel.to_vec(), [1, self.dims.n_mels, frames]),
            &self.device,
        );
        let x = gelu(module::conv1d(
            x,
            self.conv1_weight.clone(),
            Some(self.conv1_bias.clone()),
            ConvOptions::new([1], [1], [1], 1),
        ));
        let x = gelu(module::conv1d(
            x,
            self.conv2_weight.clone(),
            Some(self.conv2_bias.clone()),
            ConvOptions::new([2], [1], [1], 1),
        ));
        // [1, S, n_ctx] → [n_ctx, S]
        let x = x.squeeze::<2>().transpose();
        let mut x = x + self.encoder_positional.clone();
        let heads = self.dims.n_audio_head;
        for block in &self.encoder_blocks {
            let normed = block.attn_ln.forward(x.clone());
            x = x + attention(&block.attn, normed.clone(), normed, heads, None, None);
            let normed = block.mlp_ln.forward(x.clone());
            x = x + block.mlp_down.forward(gelu2(block.mlp_up.forward(normed)));
        }
        self.ln_post.forward(x)
    }

    /// Teacher-forced decoder logits for the whole token sequence:
    /// `[tokens.len(), n_vocab]`.
    pub fn decoder_logits(
        &self,
        tokens: &[i32],
        encoder_output: Tensor<B, 2>,
        lora: Option<&crate::lora::DecoderLora<B>>,
    ) -> Tensor<B, 2> {
        let n = tokens.len();
        assert!(n <= self.dims.n_text_ctx, "sequence exceeds n_text_ctx");
        let ids: Tensor<B, 1, Int> =
            Tensor::from_data(TensorData::new(tokens.to_vec(), [n]), &self.device);
        let mut x = self.token_embedding.clone().select(0, ids)
            + self.decoder_positional.clone().narrow(0, 0, n);

        // Causal mask: -inf above the diagonal.
        let mut mask_values = vec![0.0f32; n * n];
        for row in 0..n {
            for col in (row + 1)..n {
                mask_values[row * n + col] = f32::NEG_INFINITY;
            }
        }
        let mask: Tensor<B, 2> =
            Tensor::from_data(TensorData::new(mask_values, [n, n]), &self.device);

        let heads = self.dims.n_text_head;
        for (layer, block) in self.decoder_blocks.iter().enumerate() {
            let layer_lora = lora.map(|l| &l.layers[layer]);
            let normed = block.attn_ln.forward(x.clone());
            x = x + attention(
                &block.attn,
                normed.clone(),
                normed,
                heads,
                Some(mask.clone()),
                layer_lora.map(|l| &l.self_attn),
            );
            let normed = block.cross_attn_ln.forward(x.clone());
            x = x + attention(
                &block.cross_attn,
                normed,
                encoder_output.clone(),
                heads,
                None,
                layer_lora.map(|l| &l.cross_attn),
            );
            let normed = block.mlp_ln.forward(x.clone());
            x = x + block.mlp_down.forward(gelu2(block.mlp_up.forward(normed)));
        }
        let x = self.decoder_ln.forward(x);
        x.matmul(self.token_embedding.clone().transpose())
    }

    /// The special-token ids for this model's vocabulary.
    #[must_use]
    pub fn special_tokens(&self) -> SpecialTokens {
        SpecialTokens::for_vocab(self.dims.n_vocab)
    }

    /// Greedy English transcription of an encoded window with timestamps off,
    /// mirroring whisper.cpp's default suppression. Returns only the text
    /// tokens (no specials).
    pub fn greedy_decode(&self, encoder_output: Tensor<B, 2>, max_tokens: usize) -> Vec<i32> {
        let special = self.special_tokens();
        let mut tokens = special.transcription_prompt();
        let mut text = Vec::new();
        for step in 0..max_tokens {
            let logits = self.decoder_logits(&tokens, encoder_output.clone(), None);
            let n = tokens.len();
            let last = logits.narrow(0, n - 1, 1).into_data();
            let mut row = last.to_vec::<f32>().expect("logits are f32");
            // Specials and timestamps are never sampled; blank/eot are banned
            // on the very first step (suppress_blank).
            for id in special.eot + 1..self.dims.n_vocab as i32 {
                row[id as usize] = f32::NEG_INFINITY;
            }
            if step == 0 {
                row[TOKEN_BLANK as usize] = f32::NEG_INFINITY;
                row[special.eot as usize] = f32::NEG_INFINITY;
            }
            let (best, _) = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).expect("no NaN logits"))
                .expect("non-empty vocab");
            let best = best as i32;
            if best == special.eot {
                break;
            }
            tokens.push(best);
            text.push(best);
        }
        text
    }

    /// Detokenizes text-token ids with the model's own stored vocabulary.
    #[must_use]
    pub fn detokenize(vocab: &[Vec<u8>], tokens: &[i32]) -> String {
        let mut bytes = Vec::new();
        for &token in tokens {
            if let Some(piece) = vocab.get(token as usize) {
                bytes.extend_from_slice(piece);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Scaled dot-product attention over `[n, S]` inputs, optional additive mask,
/// optional LoRA deltas on the query/value projections.
fn attention<B: Backend>(
    weights: &Attention<B>,
    query_input: Tensor<B, 2>,
    key_value_input: Tensor<B, 2>,
    heads: usize,
    mask: Option<Tensor<B, 2>>,
    lora: Option<&AttentionLora<B>>,
) -> Tensor<B, 2> {
    let [n, state] = query_input.dims();
    let [m, _] = key_value_input.dims();
    let head_dim = state / heads;

    let mut q = weights.query.forward(query_input.clone());
    let k = weights.key.forward(key_value_input.clone());
    let mut v = weights.value.forward(key_value_input.clone());
    if let Some(lora) = lora {
        if let Some(pair) = &lora.query {
            q = q + pair.delta(query_input.clone());
        }
        if let Some(pair) = &lora.value {
            v = v + pair.delta(key_value_input.clone());
        }
    }

    let split = |t: Tensor<B, 2>, len: usize| -> Tensor<B, 3> {
        t.reshape([len, heads, head_dim]).swap_dims(0, 1)
    };
    let q = split(q, n);
    let k = split(k, m);
    let v = split(v, m);

    let mut scores = q.matmul(k.transpose()) / (head_dim as f32).sqrt();
    if let Some(mask) = mask {
        scores = scores + mask.unsqueeze::<3>();
    }
    let merged = activation::softmax(scores, 2)
        .matmul(v)
        .swap_dims(0, 1)
        .reshape([n, state]);
    weights.out.forward(merged)
}

/// whisper.cpp's GELU: the tanh approximation, not erf.
fn gelu<B: Backend>(x: Tensor<B, 3>) -> Tensor<B, 3> {
    let inner =
        (x.clone() * (x.clone().powf_scalar(2.0) * 0.044_715 + 1.0)) * 0.797_884_560_802_865_4;
    x * 0.5 * (inner.tanh() + 1.0)
}

fn gelu2<B: Backend>(x: Tensor<B, 2>) -> Tensor<B, 2> {
    let inner =
        (x.clone() * (x.clone().powf_scalar(2.0) * 0.044_715 + 1.0)) * 0.797_884_560_802_865_4;
    x * 0.5 * (inner.tanh() + 1.0)
}

#[cfg(test)]
mod tests {
    use super::SpecialTokens;

    #[test]
    fn english_only_models_use_the_classic_token_ids() {
        // tiny.en / medium.en: n_vocab 51864 (whisper.cpp defaults, unshifted).
        let tokens = SpecialTokens::for_vocab(51_864);
        assert!(!tokens.multilingual);
        assert_eq!(tokens.eot, 50_256);
        assert_eq!(tokens.sot, 50_257);
        assert_eq!(tokens.no_timestamps, 50_362);
        // English-only prompt carries no language/task tokens.
        assert_eq!(tokens.transcription_prompt(), vec![50_257, 50_362]);
    }

    #[test]
    fn multilingual_models_shift_the_ids_and_prompt_with_language_and_task() {
        // medium/large (99 languages): n_vocab 51865 shifts everything by one
        // (dt = num_languages - 98 = 1), languages sit at sot+1..sot+99.
        let tokens = SpecialTokens::for_vocab(51_865);
        assert!(tokens.multilingual);
        assert_eq!(tokens.eot, 50_257);
        assert_eq!(tokens.sot, 50_258);
        assert_eq!(tokens.transcribe, 50_359);
        assert_eq!(tokens.no_timestamps, 50_363);
        assert_eq!(
            tokens.language(0),
            50_259,
            "English is the first language token"
        );
        // Multilingual transcription prompt: [sot, lang, transcribe, not].
        assert_eq!(
            tokens.transcription_prompt(),
            vec![50_258, 50_259, 50_359, 50_363]
        );
    }
}
