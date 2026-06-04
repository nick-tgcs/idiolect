//! Crate documentation for the Idiolect workspace.

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

mod classifier;
pub mod manifest;

pub use manifest::{
    LearningManifestBuilder, Manifest, ManifestBuildError, ManifestBuildInput, ManifestCandidate,
    ManifestCandidateInput,
};

pub use classifier::{CandidateClassifier, CandidateEvidence, CandidateLabel};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
