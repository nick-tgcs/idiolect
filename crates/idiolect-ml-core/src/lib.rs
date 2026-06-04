//! Idiolect-owned ML artifact and metric contracts.

pub mod artifact;
pub mod metrics;

pub use artifact::{TrainingArtifact, TrainingConfig, TrainingManifest, TrainingManifestItem};
pub use metrics::{EvaluationReport, EvaluationSuites};

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
