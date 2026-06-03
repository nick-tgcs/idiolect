pub use idiolect_core::domain::adapter::{EvaluationReport, TrainingArtifact};

pub trait EvaluationPort {
    type Error;

    fn evaluate(&self, artifact: TrainingArtifact) -> Result<EvaluationReport, Self::Error>;
}
