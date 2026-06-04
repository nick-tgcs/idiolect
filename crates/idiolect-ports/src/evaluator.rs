pub use idiolect_ml_core::{EvaluationReport, EvaluationSuites, TrainingArtifact};

pub trait EvaluationPort {
    type Error;

    fn evaluate(
        &self,
        artifact: TrainingArtifact,
        suites: EvaluationSuites,
    ) -> Result<EvaluationReport, Self::Error>;
}
