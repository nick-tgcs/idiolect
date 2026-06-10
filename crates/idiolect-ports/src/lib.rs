//! Port interfaces for adapter boundaries.

pub mod adapter_registry;
pub mod asr;
pub mod audio;
pub mod codec;
pub mod evaluator;
pub mod input_method;
pub mod storage;
pub mod trainer;
pub mod translation;
pub mod vad;

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
