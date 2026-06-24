//! Crate documentation for the Idiolect workspace.

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

mod classifier;
pub mod manifest;
mod metrics;
mod promotion;
pub mod revalidate;
pub mod train_command;

pub use manifest::{
    LearningManifestBuilder, Manifest, ManifestBuildError, ManifestBuildInput, ManifestCandidate,
    ManifestCandidateInput, ManifestSplit, ManifestV2, ManifestV2BuildInput,
    ManifestV2CandidateInput, ManifestV2Item,
};
pub use metrics::{ArtifactCompatibility, EvaluationReport, MetricDeltas, MetricDeltasInput};
pub use promotion::{
    evaluate_promotion, AdapterRegistry, AdapterRegistryError, PromotionDecision, PromotionPolicy,
    RollbackError,
};

pub use classifier::{CandidateClassifier, CandidateEvidence, CandidateLabel};
pub use revalidate::{
    decide, revalidate_user, RevalidationEntry, RevalidationError, RevalidationOutcome,
    RevalidationReport,
};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
