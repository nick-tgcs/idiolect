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
    Balanced,
    Research,
    StrictPrivate,
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
        cache_ref: &DecodedAudioCacheRef,
        mode: AudioRetentionMode,
    ) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub session_id: ImeSessionId,
    pub text: String,
    pub state: HistoryState,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HistoryState {
    #[default]
    Committed,
    Cancelled,
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

    // History query methods (read projection of session data)
    fn recent_history(&self, limit: u32) -> Result<Vec<HistoryEntry>, Self::Error>;
    fn prune_history(&mut self, older_than_days: u32) -> Result<u64, Self::Error>;
    fn delete_history_entry(&mut self, id: i64) -> Result<(), Self::Error>;
}

// Desktop integration types (tray, clipboard) - kept in ports for use by application layer
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayIcon {
    Idle,
    Recording,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayStatus {
    Active,
    Passive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub kind: TrayMenuItemKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayMenuItemKind {
    /// A plain clickable item, optionally with a nested submenu.
    Standard { submenu: Option<Vec<TrayMenuItem>> },
    /// A checkable toggle item (e.g. "Mute").
    Checkable { checked: bool },
    /// A mutually-exclusive radio group rendered as sibling items.
    /// The adapter expands this into individual radio items at render time.
    RadioGroup { options: Vec<String>, selected: usize },
    /// A visual separator.
    Separator,
}

pub trait TrayPort {
    type Error;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error>;
    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error>;
    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error>;
    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error>;
}
