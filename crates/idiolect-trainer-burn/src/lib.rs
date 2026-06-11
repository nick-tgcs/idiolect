//! Burn-backed trainer adapter for Idiolect.

pub mod ggml;
pub mod lora;
pub mod mel;
mod trainer;
pub mod train;
pub mod whisper;

pub use trainer::{BurnTrainer, BurnTrainerError};

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
