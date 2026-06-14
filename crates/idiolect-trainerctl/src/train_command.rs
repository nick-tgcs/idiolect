//! `trainerctl train`: LoRA-trains the Burn whisper decoder on the user's
//! cleaned training candidates and emits a MERGED ggml model as the artifact.
//! Nothing is applied — the daemon keeps serving its configured model until a
//! human (or the promotion gate, once wired) switches `asr.model`.
//!
//! Pipeline per candidate: stored Opus audio → 16 kHz samples → log-mel →
//! frozen encoder (computed once, cached across epochs) → teacher-forced
//! decoder with LoRA on the attention query/value projections. Labels are the
//! candidate's corrected transcript, tokenized with the BASE MODEL'S OWN
//! tokenizer (via whisper-rs) so trainer and serving engine can never drift;
//! the teacher-forcing prompt comes from the model's vocabulary (English-only
//! vs multilingual prompts differ). Every 10th usable sample is held out for
//! a before/after validation loss.
//!
//! `--gpu` trains on the CUDA backend (CubeCL); builds without the `cuda`
//! feature reject it with a clear error instead of silently training on CPU.

use std::path::Path;

use burn::backend::{Autodiff, NdArray};
use burn::tensor::backend::Backend;
use burn::tensor::Tensor;

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioObjectRef, AudioStorePort};
use idiolect_trainer_burn::ggml::GgmlModel;
use idiolect_trainer_burn::lora::{merge_into_ggml, Adam, DecoderLora, LoraConfig};
use idiolect_trainer_burn::mel::{log_mel_spectrogram, SAMPLE_RATE};
use idiolect_trainer_burn::train::{sequence_loss, train_step};
use idiolect_trainer_burn::whisper::WhisperRuntime;
use serde::Serialize;

pub struct TrainFlags {
    pub db: String,
    pub audio_root: String,
    pub user: String,
    pub base_model: String,
    pub output: String,
    pub epochs: usize,
    pub learning_rate: f32,
    pub rank: usize,
    pub max_samples: Option<usize>,
    pub gpu: bool,
}

#[derive(Debug, Serialize)]
pub struct TrainReport {
    pub backend: String,
    pub usable_samples: usize,
    pub trained_samples: usize,
    pub holdout_samples: usize,
    pub skipped: Vec<String>,
    pub epochs: usize,
    pub first_epoch_mean_loss: f32,
    pub last_epoch_mean_loss: f32,
    pub holdout_loss_before: f32,
    pub holdout_loss_after: f32,
    pub output: String,
    pub applied: bool,
}

struct Example<B: Backend> {
    tokens: Vec<i32>,
    encoded: Tensor<B, 2>,
}

pub fn run_train(flags: &TrainFlags) -> Result<TrainReport, String> {
    if flags.gpu {
        #[cfg(feature = "cuda")]
        {
            return run_train_backend::<burn::backend::Cuda<f32, i32>>(flags, "burn-cuda");
        }
        #[cfg(not(feature = "cuda"))]
        {
            return Err(
                "this build has no GPU training support — rebuild trainerctl with --features cuda"
                    .to_owned(),
            );
        }
    }
    run_train_backend::<NdArray<f32>>(flags, "burn-ndarray")
}

fn run_train_backend<B: Backend>(flags: &TrainFlags, backend: &str) -> Result<TrainReport, String>
where
    B::Device: Default,
{
    let device = B::Device::default();
    let mut store = SqliteMetadataStore::open_path(&flags.db).map_err(|error| error.to_string())?;
    store.migrate().map_err(|error| error.to_string())?;
    let audio_root = std::path::PathBuf::from(&flags.audio_root);
    let audio_store = FileAudioStore::new(
        audio_root.clone(),
        audio_root.with_file_name("decoded-cache"),
    );

    let base = GgmlModel::read_file(Path::new(&flags.base_model))
        .map_err(|error| format!("reading base model: {error}"))?;
    let tokenizer = WhisperAsr::load(
        &flags.base_model,
        idiolect_adapter_whisper::WhisperOptions {
            use_gpu: false,
            ..Default::default()
        },
    )
    .map_err(|error| format!("loading tokenizer: {error}"))?;
    // ONE weight copy serves both encoding and training: an autodiff tensor
    // without require_grad tracks nothing, and a second runtime would double
    // GPU memory (medium is ~3 GB of f32 weights).
    let trainer = WhisperRuntime::<Autodiff<B>>::load(&base, &device)
        .map_err(|error| format!("loading weights: {error}"))?;
    let prompt = trainer.special_tokens().transcription_prompt();
    let prompt_len = prompt.len();

    let candidates = store
        .training_candidates_for_manifest_v2(&flags.user)
        .map_err(|error| format!("listing candidates: {error}"))?;
    let codec = OpusCodec::new();

    let mut skipped = Vec::new();
    let mut examples: Vec<Example<B>> = Vec::new();
    for candidate in candidates {
        if let Some(limit) = flags.max_samples {
            if examples.len() >= limit {
                break;
            }
        }
        let id = candidate.training_candidate_id;
        let text = candidate.corrected_transcript.trim();
        if text.is_empty() {
            skipped.push(format!("#{id}: empty transcript"));
            continue;
        }
        let audio_ref = AudioObjectRef {
            object_key: candidate.audio_object_key.clone(),
            codec_name: "opus".to_owned(),
            sample_rate_hz: 16_000,
            channels: 1,
        };
        let samples = match audio_store
            .read_source_audio(&audio_ref)
            .map_err(|error| error.to_string())
            .and_then(|encoded| codec.decode(&encoded).map_err(|error| error.to_string()))
        {
            Ok(segment) => segment.samples_f32_mono,
            Err(error) => {
                skipped.push(format!("#{id}: audio unavailable ({error})"));
                continue;
            }
        };
        if samples.len() > SAMPLE_RATE * 30 {
            // Long takes need windowing with aligned text — future work; one
            // bad pair is worse than one skipped pair.
            skipped.push(format!("#{id}: longer than one 30s window"));
            continue;
        }
        // Whisper transcripts conventionally lead with a space.
        let label = format!(" {text}");
        let mut tokens = prompt.clone();
        match tokenizer.tokenize(&label) {
            Ok(text_tokens) => tokens.extend(text_tokens),
            Err(error) => {
                skipped.push(format!("#{id}: tokenization failed ({error})"));
                continue;
            }
        }
        tokens.push(trainer.special_tokens().eot);
        if tokens.len() > trainer.dims.n_text_ctx {
            skipped.push(format!("#{id}: transcript exceeds the text context"));
            continue;
        }
        let mel = log_mel_spectrogram(&samples, &base.filters);
        let encoded = trainer.encode(&mel).inner();
        examples.push(Example { tokens, encoded });
        eprintln!("prepared {} example(s)", examples.len());
    }

    if examples.is_empty() {
        return Err("no usable training examples".to_owned());
    }

    // Deterministic split: every 10th example is held out.
    let mut training = Vec::new();
    let mut holdout = Vec::new();
    for (index, example) in examples.into_iter().enumerate() {
        if index % 10 == 9 {
            holdout.push(example);
        } else {
            training.push(example);
        }
    }

    let config = LoraConfig {
        rank: flags.rank,
        alpha: 2.0 * flags.rank as f32,
    };
    let mut lora = DecoderLora::<Autodiff<B>>::init(
        config,
        trainer.dims.n_text_layer,
        trainer.dims.n_text_state,
        0x1d10_1ec7,
        &device,
    )
    .trainable();
    let mut optimizer = Adam::new(flags.learning_rate);

    let holdout_loss = |lora: &DecoderLora<Autodiff<B>>| -> f32 {
        if holdout.is_empty() {
            return f32::NAN;
        }
        let total: f32 = holdout
            .iter()
            .map(|example| {
                let encoded: Tensor<Autodiff<B>, 2> = Tensor::from_inner(example.encoded.clone());
                sequence_loss(&trainer, encoded, &example.tokens, prompt_len, lora)
                    .into_data()
                    .to_vec::<f32>()
                    .expect("scalar loss")[0]
            })
            .sum();
        total / holdout.len() as f32
    };

    let holdout_loss_before = holdout_loss(&lora);
    let mut first_epoch_mean_loss = f32::NAN;
    let mut last_epoch_mean_loss = f32::NAN;
    for epoch in 0..flags.epochs {
        let mut total = 0.0f32;
        for (index, example) in training.iter().enumerate() {
            let encoded: Tensor<Autodiff<B>, 2> = Tensor::from_inner(example.encoded.clone());
            let loss = train_step(
                &trainer,
                encoded,
                &example.tokens,
                prompt_len,
                &mut lora,
                &mut optimizer,
            );
            total += loss;
            if index % 10 == 0 {
                eprintln!(
                    "epoch {}/{} sample {}/{} loss {loss:.4}",
                    epoch + 1,
                    flags.epochs,
                    index + 1,
                    training.len()
                );
            }
        }
        let mean = total / training.len() as f32;
        eprintln!("epoch {}/{} mean loss {mean:.4}", epoch + 1, flags.epochs);
        if epoch == 0 {
            first_epoch_mean_loss = mean;
        }
        last_epoch_mean_loss = mean;
    }
    let holdout_loss_after = holdout_loss(&lora);

    let mut merged = base.clone();
    merge_into_ggml(&lora, &mut merged).map_err(|error| format!("merging adapter: {error}"))?;
    merged
        .write_file(Path::new(&flags.output))
        .map_err(|error| format!("writing artifact: {error}"))?;

    Ok(TrainReport {
        backend: backend.to_owned(),
        usable_samples: training.len() + holdout.len(),
        trained_samples: training.len(),
        holdout_samples: holdout.len(),
        skipped,
        epochs: flags.epochs,
        first_epoch_mean_loss,
        last_epoch_mean_loss,
        holdout_loss_before,
        holdout_loss_after,
        output: flags.output.clone(),
        applied: false,
    })
}
