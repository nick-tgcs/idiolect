//! The correctness gates for the Burn whisper port, in dependency order:
//!
//! 1. PARITY — the Burn forward pass must transcribe the fixture clip to the
//!    same words as the serving engine (whisper-rs over the SAME ggml file).
//!    Until this holds, any training loss is meaningless.
//! 2. IDENTITY — a freshly initialised LoRA adapter (B = 0) must change the
//!    logits not at all: training starts EXACTLY at the base model.
//! 3. FULL CIRCLE — overfitting the adapter on one (audio, text) pair must
//!    drive the loss down, and MERGING the adapter into the ggml file must
//!    make the unmodified serving engine (whisper-rs) transcribe that audio
//!    to the trained text. This is the deployment story end to end: train in
//!    Burn, ship a plain .bin.

use std::path::PathBuf;

use burn::backend::{Autodiff, NdArray};
use burn::tensor::Tensor;

use idiolect_adapter_whisper::{WhisperAsr, WhisperOptions};
use idiolect_ports::asr::AsrPort;
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;
use idiolect_trainer_burn::ggml::GgmlModel;
use idiolect_trainer_burn::lora::{merge_into_ggml, Adam, DecoderLora, LoraConfig};
use idiolect_trainer_burn::mel::log_mel_spectrogram;
use idiolect_trainer_burn::train::train_step;
use idiolect_trainer_burn::whisper::{
    WhisperRuntime, TOKEN_EOT, TOKEN_NO_TIMESTAMPS, TOKEN_SOT,
};

type Cpu = NdArray<f32>;
type Train = Autodiff<Cpu>;

fn fixture_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/whisper/ggml-tiny.en.bin")
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

#[test]
fn the_burn_forward_pass_decodes_like_the_serving_engine() {
    let model = GgmlModel::read_file(&fixture_model_path()).expect("fixture model parses");
    let device = Default::default();
    let runtime = WhisperRuntime::<Cpu>::load(&model, &device).expect("weights load");

    let audio = restart_traffic_fixture_16khz_mono();
    let mel = log_mel_spectrogram(&audio.samples_f32_mono, &model.filters);
    let encoded = runtime.encode(&mel);
    let tokens = runtime.greedy_decode(encoded, 24);
    let burn_text = WhisperRuntime::<Cpu>::detokenize(&model.vocab, &tokens);

    let engine = WhisperAsr::load(fixture_model_path(), WhisperOptions::default())
        .expect("serving engine loads the same file");
    let engine_text = engine.transcribe(&audio).expect("engine transcribes").text;

    assert_eq!(
        normalized_words(&burn_text),
        normalized_words(&engine_text),
        "Burn decode {burn_text:?} must carry the same words as the engine {engine_text:?}"
    );
}

#[test]
fn a_freshly_initialised_adapter_is_an_exact_identity() {
    let model = GgmlModel::read_file(&fixture_model_path()).expect("fixture model parses");
    let device = Default::default();
    let runtime = WhisperRuntime::<Cpu>::load(&model, &device).expect("weights load");

    // A tiny synthetic "encoder output" keeps this test fast: identity is a
    // property of the adapter algebra, not of real audio.
    let encoder_output: Tensor<Cpu, 2> =
        Tensor::ones([8, runtime.dims.n_audio_state], &device) * 0.01;
    let tokens = [TOKEN_SOT, TOKEN_NO_TIMESTAMPS, 1_000, 2_000, TOKEN_EOT];

    let base = runtime.decoder_logits(&tokens, encoder_output.clone(), None);
    let lora = DecoderLora::<Cpu>::init(
        LoraConfig::default(),
        runtime.dims.n_text_layer,
        runtime.dims.n_text_state,
        42,
        &device,
    );
    let adapted = runtime.decoder_logits(&tokens, encoder_output, Some(&lora));

    let difference = (base - adapted).abs().max().into_data().to_vec::<f32>().expect("scalar")[0];
    assert!(
        difference < 1e-5,
        "zero-initialised LoRA must not move the logits (max diff {difference})"
    );
}

#[test]
fn an_overfitted_adapter_merges_into_a_model_the_engine_serves() {
    let model = GgmlModel::read_file(&fixture_model_path()).expect("fixture model parses");
    let device = Default::default();

    // The audio says "restart traffic"; we teach the adapter to transcribe it
    // as something the base model would never produce.
    let target_text = " deploy the traefik ingress";
    let tokenizer = WhisperAsr::load_fixture_model().expect("tokenizer model loads");
    let mut tokens = vec![TOKEN_SOT, TOKEN_NO_TIMESTAMPS];
    tokens.extend(tokenizer.tokenize(target_text).expect("target tokenizes"));
    tokens.push(TOKEN_EOT);

    let audio = restart_traffic_fixture_16khz_mono();
    let mel = log_mel_spectrogram(&audio.samples_f32_mono, &model.filters);

    // Encode once on the inner backend (the encoder is frozen; no gradients
    // flow through it for decoder-only LoRA).
    let inference = WhisperRuntime::<Cpu>::load(&model, &device).expect("weights load");
    let encoded_inner = inference.encode(&mel);

    let trainer = WhisperRuntime::<Train>::load(&model, &device).expect("weights load");
    let mut lora = DecoderLora::<Train>::init(
        LoraConfig::default(),
        trainer.dims.n_text_layer,
        trainer.dims.n_text_state,
        7,
        &device,
    )
    .trainable();
    let mut optimizer = Adam::new(1e-2);

    let mut first_loss = f32::NAN;
    let mut last_loss = f32::NAN;
    for step in 0..60 {
        let encoded: Tensor<Train, 2> = Tensor::from_inner(encoded_inner.clone());
        let loss = train_step(&trainer, encoded, &tokens, &mut lora, &mut optimizer);
        if step == 0 {
            first_loss = loss;
        }
        last_loss = loss;
        if loss < 0.01 {
            break;
        }
    }
    assert!(
        last_loss < first_loss * 0.1 && last_loss < 0.5,
        "training must overfit one example (first {first_loss}, last {last_loss})"
    );

    // Merge and hand the result to the UNMODIFIED serving engine.
    let mut merged = model.clone();
    merge_into_ggml(&lora, &mut merged).expect("merge applies");
    let dir = tempfile::tempdir().expect("temp dir");
    let merged_path = dir.path().join("tiny-personal.bin");
    merged.write_file(&merged_path).expect("merged model writes");

    let engine =
        WhisperAsr::load(&merged_path, WhisperOptions::default()).expect("merged model loads");
    let served = engine.transcribe(&audio).expect("merged model transcribes").text;
    assert_eq!(
        normalized_words(&served),
        normalized_words(target_text),
        "the engine must serve the personalised behaviour from a plain .bin (got {served:?})"
    );
}
