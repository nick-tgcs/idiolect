use std::error::Error;
use std::fmt::{Display, Formatter};

use burn::tensor::{Shape, Tensor, TensorData};
use idiolect_ml_core::{TrainingArtifact, TrainingConfig, TrainingManifest, TrainingManifestItem};
use idiolect_ports::trainer::TrainerPort;
use sha2::{Digest, Sha256};

type BurnBackend = burn::backend::NdArray<f32>;

#[derive(Debug, Default)]
pub struct BurnTrainer;

impl BurnTrainer {
    const BACKEND_ID: &'static str = "burn-ndarray-0.21.0";

    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl TrainerPort for BurnTrainer {
    type Error = BurnTrainerError;

    fn train(
        &self,
        manifest: TrainingManifest,
        config: TrainingConfig,
    ) -> Result<TrainingArtifact, Self::Error> {
        validate_manifest(&manifest)?;
        let burn_signal = burn_manifest_signal(manifest.items());
        let digest = artifact_digest(&manifest, &config, burn_signal);

        Ok(TrainingArtifact::new(
            digest,
            manifest.digest(),
            manifest.base_model_id(),
            Self::BACKEND_ID,
            config.candidate_id(),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BurnTrainerError {
    EmptyManifest,
    MissingAudio { item_index: usize },
}

impl Display for BurnTrainerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyManifest => formatter.write_str("training manifest has no items"),
            Self::MissingAudio { item_index } => {
                write!(
                    formatter,
                    "training manifest item {item_index} has no audio"
                )
            }
        }
    }
}

impl Error for BurnTrainerError {}

fn validate_manifest(manifest: &TrainingManifest) -> Result<(), BurnTrainerError> {
    if manifest.items().is_empty() {
        return Err(BurnTrainerError::EmptyManifest);
    }

    for (item_index, item) in manifest.items().iter().enumerate() {
        if item.audio_object_key().trim().is_empty() || item.audio_digest().trim().is_empty() {
            return Err(BurnTrainerError::MissingAudio { item_index });
        }
    }

    Ok(())
}

fn burn_manifest_signal(items: &[TrainingManifestItem]) -> f32 {
    let device = burn::backend::ndarray::NdArrayDevice::default();
    let values = items
        .iter()
        .map(|item| (item.audio_digest().len() + item.transcript().len()) as f32)
        .collect::<Vec<_>>();
    let item_count = items.len();
    let tensor = Tensor::<BurnBackend, 1>::from_data(
        TensorData::new(values, Shape::new([item_count])),
        &device,
    );
    tensor.sum().into_scalar()
}

fn artifact_digest(
    manifest: &TrainingManifest,
    config: &TrainingConfig,
    burn_signal: f32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest.digest().as_bytes());
    hasher.update([0]);
    hasher.update(manifest.base_model_id().as_bytes());
    hasher.update([0]);
    hasher.update(config.candidate_id().as_bytes());
    hasher.update([0]);
    hasher.update(BurnTrainer::BACKEND_ID.as_bytes());
    hasher.update([0]);
    hasher.update(burn_signal.to_le_bytes());
    sha256_lower_hex(hasher.finalize().as_slice())
}

fn sha256_lower_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    output
}
