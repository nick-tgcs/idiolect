pub use idiolect_core::domain::adapter::{TrainingArtifact, TrainingManifest};

pub trait TrainerPort {
    type Error;

    fn train(&self, manifest: TrainingManifest) -> Result<TrainingArtifact, Self::Error>;
}
