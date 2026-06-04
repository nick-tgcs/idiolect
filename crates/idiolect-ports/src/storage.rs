use idiolect_common::ids::ImeSessionId;

pub use crate::audio::{AudioSegment, EncodedAudio};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioObjectRef {
    pub object_key: String,
    pub codec_name: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedAudioCacheRef {
    pub object_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioRetentionMode {
    Minimal,
}

pub trait AudioStorePort {
    type Error;

    fn write_source_audio(
        &self,
        user_id: &str,
        utterance_id: &str,
        encoded_audio: &EncodedAudio,
    ) -> Result<AudioObjectRef, Self::Error>;

    fn read_source_audio(&self, audio_ref: &AudioObjectRef) -> Result<EncodedAudio, Self::Error>;

    fn write_decoded_cache(
        &self,
        user_id: &str,
        utterance_id: &str,
        segment: &AudioSegment,
    ) -> Result<DecodedAudioCacheRef, Self::Error>;

    fn privacy_delete_user(&self, user_id: &str) -> Result<(), Self::Error>;

    fn apply_retention(
        &self,
        audio_ref: &AudioObjectRef,
        mode: AudioRetentionMode,
    ) -> Result<(), Self::Error>;
}

pub trait MetadataStorePort {
    type Error;

    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error>;
    fn record_preedit_change(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error>;
    fn commit_session(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
}
