pub use idiolect_ml_core::{TrainingArtifact, TrainingConfig, TrainingManifest};

pub trait TrainerPort {
    type Error;

    fn train(
        &self,
        manifest: TrainingManifest,
        config: TrainingConfig,
    ) -> Result<TrainingArtifact, Self::Error>;
}
