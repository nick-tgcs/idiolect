pub use idiolect_core::domain::adapter::{AudioSegment, TranscriptDraft};

#[derive(Debug, Eq, PartialEq)]
pub struct AdapterCapabilities {
    pub name: String,
    pub version: String,
    pub supports_streaming: bool,
    pub supports_word_timestamps: bool,
    pub supports_confidence: bool,
    pub supports_gpu: bool,
    pub supports_incremental_updates: bool,
}

pub trait AsrPort {
    type Error;

    fn capabilities(&self) -> AdapterCapabilities;
    fn transcribe(&self, audio: &AudioSegment) -> Result<TranscriptDraft, Self::Error>;
}

#[cfg(test)]
mod capability_tests {
    use super::AdapterCapabilities;

    #[test]
    fn adapter_capabilities_report_stable_name_and_version() {
        let capabilities = AdapterCapabilities {
            name: "fixture-asr".to_owned(),
            version: "0.1.0".to_owned(),
            supports_streaming: false,
            supports_word_timestamps: false,
            supports_confidence: true,
            supports_gpu: false,
            supports_incremental_updates: false,
        };

        assert_eq!(capabilities.name, "fixture-asr");
        assert!(!capabilities.supports_gpu);
    }
}
