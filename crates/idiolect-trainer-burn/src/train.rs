//! Teacher-forced LoRA training over the Burn whisper decoder.

use burn::tensor::backend::AutodiffBackend;
use burn::tensor::{activation, Int, Tensor, TensorData};

use crate::lora::{adam_step, Adam, DecoderLora};
use crate::whisper::WhisperRuntime;

/// Cross-entropy over the text+EOT positions of a teacher-forced sequence
/// `[…prompt…, …text…, eot]`: position `i` predicts `tokens[i+1]`, and the
/// prompt's own continuation (sot → language → task → no-timestamps) is not
/// trained.
pub fn sequence_loss<B: AutodiffBackend>(
    runtime: &WhisperRuntime<B>,
    encoder_output: Tensor<B, 2>,
    tokens: &[i32],
    prompt_len: usize,
    lora: &DecoderLora<B>,
) -> Tensor<B, 1> {
    assert!(
        prompt_len >= 1 && tokens.len() > prompt_len,
        "need at least one trained position"
    );
    let logits = runtime.decoder_logits(tokens, encoder_output, Some(lora));
    let n = tokens.len();
    let rows = logits.narrow(0, prompt_len - 1, n - prompt_len);
    let targets: Vec<i32> = tokens[prompt_len..n].to_vec();
    let target_count = targets.len();
    let target_ids: Tensor<B, 2, Int> =
        Tensor::from_data(TensorData::new(targets, [target_count, 1]), &rows.device());
    let log_probs = activation::log_softmax(rows, 1);
    let picked = log_probs.gather(1, target_ids);
    picked.mean().neg().unsqueeze()
}

/// One optimisation step on one example; returns the loss value.
pub fn train_step<B: AutodiffBackend>(
    runtime: &WhisperRuntime<B>,
    encoder_output: Tensor<B, 2>,
    tokens: &[i32],
    prompt_len: usize,
    lora: &mut DecoderLora<B>,
    optimizer: &mut Adam<B::InnerBackend>,
) -> f32 {
    let loss = sequence_loss(runtime, encoder_output, tokens, prompt_len, lora);
    let value = loss
        .clone()
        .into_data()
        .to_vec::<f32>()
        .expect("loss is a scalar f32")[0];
    let gradients = loss.backward();
    adam_step::<B>(optimizer, lora, &gradients);
    value
}
