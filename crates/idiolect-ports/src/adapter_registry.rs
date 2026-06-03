pub use idiolect_core::domain::adapter::{EvaluationReport, TrainingArtifact};

pub trait AdapterRegistryPort {
    type Error;

    fn register_candidate(
        &mut self,
        artifact: TrainingArtifact,
        report: EvaluationReport,
    ) -> Result<String, Self::Error>;
    fn promote(&mut self, adapter_id: &str) -> Result<(), Self::Error>;
    fn rollback(&mut self, user_id: &str) -> Result<(), Self::Error>;
}
