//! LoRA adapters for the Burn whisper decoder, plus the merge that folds a
//! trained adapter back into a GGML file.
//!
//! Only the decoder's attention query/value projections (self and cross) get
//! adapters — the standard whisper personalization recipe and the README's
//! plan (attention q/v, rank 8–16). Base weights stay frozen plain tensors;
//! the adapter pair is `y += (α/r) · B(Ax)` with B zero-initialised so an
//! untrained adapter is EXACTLY the base model.
//!
//! Deployment never loads adapters: [`merge_into_ggml`] computes
//! `W' = W + (α/r)·B·A` and rewrites the weight tensors in place, so
//! whisper.cpp serves the personalised model as an ordinary `.bin`.

use burn::tensor::backend::{AutodiffBackend, Backend};
use burn::tensor::{Tensor, TensorData};

use crate::ggml::{GgmlError, GgmlModel};

#[derive(Debug, Clone, Copy)]
pub struct LoraConfig {
    pub rank: usize,
    pub alpha: f32,
}

impl Default for LoraConfig {
    fn default() -> Self {
        Self {
            rank: 8,
            alpha: 16.0,
        }
    }
}

impl LoraConfig {
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.alpha / self.rank as f32
    }
}

/// One adapted projection: `a_t` is Aᵀ `[in, r]`, `b_t` is Bᵀ `[r, out]`.
pub struct LoraPair<B: Backend> {
    pub a_t: Tensor<B, 2>,
    pub b_t: Tensor<B, 2>,
    pub scale: f32,
}

impl<B: Backend> LoraPair<B> {
    /// The adapter's contribution for input `x` `[n, in]`.
    pub fn delta(&self, x: Tensor<B, 2>) -> Tensor<B, 2> {
        x.matmul(self.a_t.clone()).matmul(self.b_t.clone()) * self.scale
    }
}

pub struct AttentionLora<B: Backend> {
    pub query: Option<LoraPair<B>>,
    pub value: Option<LoraPair<B>>,
}

pub struct DecoderLayerLora<B: Backend> {
    pub self_attn: AttentionLora<B>,
    pub cross_attn: AttentionLora<B>,
}

pub struct DecoderLora<B: Backend> {
    pub config: LoraConfig,
    pub layers: Vec<DecoderLayerLora<B>>,
}

impl<B: Backend> DecoderLora<B> {
    /// Deterministic init (tests and resumable runs must not depend on a
    /// global RNG): A gets small pseudo-random values from a seeded LCG, B is
    /// zero — so the freshly initialised adapter changes nothing.
    pub fn init(
        config: LoraConfig,
        n_layers: usize,
        state: usize,
        seed: u64,
        device: &B::Device,
    ) -> Self {
        let mut lcg = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let mut next = move || {
            lcg = lcg
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Map the top bits to (-0.02, 0.02).
            (((lcg >> 40) as f32 / (1u64 << 24) as f32) - 0.5) * 0.04
        };
        let mut pair = |scale: f32| LoraPair {
            a_t: Tensor::from_data(
                TensorData::new(
                    (0..state * config.rank)
                        .map(|_| next())
                        .collect::<Vec<f32>>(),
                    [state, config.rank],
                ),
                device,
            ),
            b_t: Tensor::zeros([config.rank, state], device),
            scale,
        };
        let layers = (0..n_layers)
            .map(|_| DecoderLayerLora {
                self_attn: AttentionLora {
                    query: Some(pair(config.scale())),
                    value: Some(pair(config.scale())),
                },
                cross_attn: AttentionLora {
                    query: Some(pair(config.scale())),
                    value: Some(pair(config.scale())),
                },
            })
            .collect();
        Self { config, layers }
    }
}

impl<B: AutodiffBackend> DecoderLora<B> {
    /// Marks every adapter tensor as trainable.
    #[must_use]
    pub fn trainable(mut self) -> Self {
        for layer in &mut self.layers {
            for attn in [&mut layer.self_attn, &mut layer.cross_attn] {
                for pair in [&mut attn.query, &mut attn.value].into_iter().flatten() {
                    pair.a_t = pair.a_t.clone().require_grad();
                    pair.b_t = pair.b_t.clone().require_grad();
                }
            }
        }
        self
    }
}

/// Hand-rolled Adam over the adapter tensors (the only trainables; pulling in
/// Burn's Module/Optimizer machinery for frozen-base LoRA buys nothing).
pub struct Adam<B: Backend> {
    pub learning_rate: f32,
    step: usize,
    first_moment: Vec<Tensor<B, 2>>,
    second_moment: Vec<Tensor<B, 2>>,
}

const BETA1: f32 = 0.9;
const BETA2: f32 = 0.999;
const EPSILON: f32 = 1e-8;

impl<B: Backend> Adam<B> {
    #[must_use]
    pub fn new(learning_rate: f32) -> Self {
        Self {
            learning_rate,
            step: 0,
            first_moment: Vec::new(),
            second_moment: Vec::new(),
        }
    }

    fn update(
        &mut self,
        index: usize,
        parameter: Tensor<B, 2>,
        gradient: Tensor<B, 2>,
    ) -> Tensor<B, 2> {
        let device = parameter.device();
        let shape = parameter.dims();
        while self.first_moment.len() <= index {
            self.first_moment.push(Tensor::zeros(shape, &device));
            self.second_moment.push(Tensor::zeros(shape, &device));
        }
        let m = self.first_moment[index].clone() * BETA1 + gradient.clone() * (1.0 - BETA1);
        let v = self.second_moment[index].clone() * BETA2
            + gradient.clone().powf_scalar(2.0) * (1.0 - BETA2);
        self.first_moment[index] = m.clone();
        self.second_moment[index] = v.clone();
        let t = self.step as i32 + 1;
        let m_hat = m / (1.0 - BETA1.powi(t));
        let v_hat = v / (1.0 - BETA2.powi(t));
        parameter - m_hat * self.learning_rate / (v_hat.sqrt() + EPSILON)
    }
}

/// One optimisation step: applies gradients from `loss.backward()` to every
/// adapter tensor and re-marks them trainable.
pub fn adam_step<B: AutodiffBackend>(
    optimizer: &mut Adam<B::InnerBackend>,
    lora: &mut DecoderLora<B>,
    gradients: &B::Gradients,
) {
    let mut index = 0;
    for layer in &mut lora.layers {
        for attn in [&mut layer.self_attn, &mut layer.cross_attn] {
            for pair in [&mut attn.query, &mut attn.value].into_iter().flatten() {
                for tensor in [&mut pair.a_t, &mut pair.b_t] {
                    if let Some(gradient) = tensor.grad(gradients) {
                        let updated = optimizer.update(index, tensor.clone().inner(), gradient);
                        *tensor = Tensor::from_inner(updated).require_grad();
                    }
                    index += 1;
                }
            }
        }
    }
    optimizer.step += 1;
}

/// Folds a trained adapter into the GGML weights: for each adapted projection,
/// `W += (α/r) · B·A`, computed in f32 and written back in the tensor's own
/// storage type. The result is a plain whisper.cpp model.
pub fn merge_into_ggml<B: Backend>(
    lora: &DecoderLora<B>,
    model: &mut GgmlModel,
) -> Result<(), GgmlError> {
    for (layer, adapters) in lora.layers.iter().enumerate() {
        let targets = [
            (&adapters.self_attn, format!("decoder.blocks.{layer}.attn")),
            (
                &adapters.cross_attn,
                format!("decoder.blocks.{layer}.cross_attn"),
            ),
        ];
        for (attn, prefix) in targets {
            let projections = [
                (&attn.query, format!("{prefix}.query.weight")),
                (&attn.value, format!("{prefix}.value.weight")),
            ];
            for (pair, name) in projections {
                let Some(pair) = pair else { continue };
                merge_pair(pair, &name, model)?;
            }
        }
    }
    Ok(())
}

fn merge_pair<B: Backend>(
    pair: &LoraPair<B>,
    name: &str,
    model: &mut GgmlModel,
) -> Result<(), GgmlError> {
    // a_t is [in, r], b_t is [r, out]; ΔW row-major [out][in] matches the
    // ggml weight layout (ne = [in, out], `in` fastest).
    let a_t = pair
        .a_t
        .clone()
        .into_data()
        .to_vec::<f32>()
        .expect("f32 lora");
    let b_t = pair
        .b_t
        .clone()
        .into_data()
        .to_vec::<f32>()
        .expect("f32 lora");
    let [input_dim, rank] = pair.a_t.dims();
    let [_, output_dim] = pair.b_t.dims();

    let tensor = model.tensor_mut(name)?;
    let mut weights = tensor.to_f32()?;
    if weights.len() != input_dim * output_dim {
        return Err(GgmlError::from_message(format!(
            "merge target {name:?} has {} elements, adapter expects {}",
            weights.len(),
            input_dim * output_dim
        )));
    }
    for out in 0..output_dim {
        for input in 0..input_dim {
            let mut delta = 0.0f32;
            for r in 0..rank {
                // a_t[in][r] · b_t[r][out]
                delta += a_t[input * rank + r] * b_t[r * output_dim + out];
            }
            weights[out * input_dim + input] += pair.scale * delta;
        }
    }
    tensor.set_from_f32(&weights)
}
