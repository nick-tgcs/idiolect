use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level runtime configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IdiolectConfig {
    #[serde(default)]
    pub user: UserConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub vad: VadConfig,
    #[serde(default)]
    pub asr: AsrConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub training: TrainingConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub history: HistoryConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

impl IdiolectConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: Self = toml::from_str(input).map_err(|_| ConfigError::ParseError)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.user.default_user_id.trim().is_empty() {
            return Err(ConfigError::ValidationError {
                field: "user.default_user_id".to_owned(),
            });
        }

        validate_non_empty_string("daemon.log_level", &self.daemon.log_level)?;
        if let Some(socket_path) = self.daemon.socket_path.as_deref() {
            validate_non_empty_string("daemon.socket_path", socket_path)?;
        }
        validate_non_empty_string("audio.input_device", &self.audio.input_device)?;
        validate_non_empty_string("vad.engine", &self.vad.engine)?;
        validate_non_empty_string("asr.engine", &self.asr.engine)?;
        validate_non_empty_string("asr.model", &self.asr.model)?;
        validate_non_empty_string("asr.language", &self.asr.language)?;
        validate_non_empty_string("storage.audio_codec", &self.storage.audio_codec)?;
        validate_non_empty_string("storage.audio_container", &self.storage.audio_container)?;
        validate_non_empty_string("training.trainer", &self.training.trainer)?;

        if self.audio.capture_sample_rate == 0 || self.audio.processing_sample_rate == 0 {
            return Err(ConfigError::ValidationError {
                field: "audio.sample_rate".to_owned(),
            });
        }
        if self.audio.capture_sample_rate < 8_000 || self.audio.capture_sample_rate > 192_000 {
            return Err(ConfigError::ValidationError {
                field: "audio.capture_sample_rate".to_owned(),
            });
        }
        if self.audio.processing_sample_rate < 8_000 || self.audio.processing_sample_rate > 192_000
        {
            return Err(ConfigError::ValidationError {
                field: "audio.processing_sample_rate".to_owned(),
            });
        }
        if self.audio.channels == 0 {
            return Err(ConfigError::ValidationError {
                field: "audio.channels".to_owned(),
            });
        }

        if self.asr.threads == 0 {
            return Err(ConfigError::ValidationError {
                field: "asr.threads".to_owned(),
            });
        }

        if self.storage.opus_bitrate_bps == 0 || self.storage.high_value_opus_bitrate_bps == 0 {
            return Err(ConfigError::ValidationError {
                field: "storage.opus_bitrate_bps".to_owned(),
            });
        }

        if self.observability.log_raw_transcripts
            || self.observability.log_corrected_transcripts
            || self.observability.log_surrounding_app_text
            || self.observability.log_private_text
        {
            return Err(ConfigError::ValidationError {
                field: "observability.private_text_logging".to_owned(),
            });
        }

        // Validate history config
        if ![1, 7, 30].contains(&self.history.retention_days) {
            return Err(ConfigError::ValidationError {
                field: "history.retention_days".to_owned(),
            });
        }
        if ![10, 25, 50].contains(&self.history.max_entries) {
            return Err(ConfigError::ValidationError {
                field: "history.max_entries".to_owned(),
            });
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserConfig {
    #[serde(default = "default_default_user_id")]
    pub default_user_id: String,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            default_user_id: default_default_user_id(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub socket_path: Option<String>,
    #[serde(default = "default_daemon_log_level")]
    pub log_level: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            log_level: default_daemon_log_level(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioConfig {
    #[serde(default = "default_audio_input_device")]
    pub input_device: String,
    #[serde(default = "default_audio_capture_sample_rate")]
    pub capture_sample_rate: u32,
    #[serde(default = "default_audio_processing_sample_rate")]
    pub processing_sample_rate: u32,
    #[serde(default = "default_audio_channels")]
    pub channels: u8,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: default_audio_input_device(),
            capture_sample_rate: default_audio_capture_sample_rate(),
            processing_sample_rate: default_audio_processing_sample_rate(),
            channels: default_audio_channels(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VadConfig {
    #[serde(default = "default_vad_engine")]
    pub engine: String,
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,
    #[serde(default = "default_vad_min_speech_ms")]
    pub min_speech_ms: u32,
    #[serde(default = "default_vad_pre_roll_ms")]
    pub pre_roll_ms: u32,
    #[serde(default = "default_vad_post_roll_ms")]
    pub post_roll_ms: u32,
    #[serde(default = "default_vad_max_utterance_ms")]
    pub max_utterance_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            engine: default_vad_engine(),
            threshold: default_vad_threshold(),
            min_speech_ms: default_vad_min_speech_ms(),
            pre_roll_ms: default_vad_pre_roll_ms(),
            post_roll_ms: default_vad_post_roll_ms(),
            max_utterance_ms: default_vad_max_utterance_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AsrConfig {
    #[serde(default = "default_asr_engine")]
    pub engine: String,
    #[serde(default = "default_asr_model")]
    pub model: String,
    #[serde(default = "default_asr_language")]
    pub language: String,
    #[serde(default = "default_asr_use_gpu")]
    pub use_gpu: bool,
    #[serde(default = "default_asr_threads")]
    pub threads: u32,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            engine: default_asr_engine(),
            model: default_asr_model(),
            language: default_asr_language(),
            use_gpu: default_asr_use_gpu(),
            threads: default_asr_threads(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub database_path: Option<String>,
    #[serde(default = "default_storage_audio_codec")]
    pub audio_codec: String,
    #[serde(default = "default_storage_audio_container")]
    pub audio_container: String,
    #[serde(default = "default_storage_opus_bitrate_bps")]
    pub opus_bitrate_bps: u32,
    #[serde(default = "default_storage_high_value_opus_bitrate_bps")]
    pub high_value_opus_bitrate_bps: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            database_path: None,
            audio_codec: default_storage_audio_codec(),
            audio_container: default_storage_audio_container(),
            opus_bitrate_bps: default_storage_opus_bitrate_bps(),
            high_value_opus_bitrate_bps: default_storage_high_value_opus_bitrate_bps(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TrainingConfig {
    #[serde(default = "default_training_min_approved_examples")]
    pub min_approved_examples: u32,
    #[serde(default = "default_training_trainer")]
    pub trainer: String,
    #[serde(default)]
    pub auto_train: bool,
}

impl Default for TrainingConfig {
    fn default() -> Self {
        Self {
            min_approved_examples: default_training_min_approved_examples(),
            trainer: default_training_trainer(),
            auto_train: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub retain_audio: bool,
    #[serde(default)]
    pub private_text_probe: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_history_max_entries")]
    pub max_entries: u32,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            retention_days: default_history_retention_days(),
            max_entries: default_history_max_entries(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub log_raw_transcripts: bool,
    #[serde(default)]
    pub log_corrected_transcripts: bool,
    #[serde(default)]
    pub log_surrounding_app_text: bool,
    #[serde(default)]
    pub log_private_text: bool,
}

#[derive(Debug, Clone)]
pub struct XdgBaseDirs {
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    pub runtime_dir: PathBuf,
}

impl Default for XdgBaseDirs {
    fn default() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        Self {
            config_home: env_path_or_fallback("XDG_CONFIG_HOME", home.join(".config")),
            data_home: env_path_or_fallback("XDG_DATA_HOME", home.join(".local").join("share")),
            cache_home: env_path_or_fallback("XDG_CACHE_HOME", home.join(".cache")),
            runtime_dir: env_path_or_fallback(
                "XDG_RUNTIME_DIR",
                home.join(".local").join("run").join("idiolect"),
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfigPaths {
    pub config_file: PathBuf,
    pub socket_path: PathBuf,
    pub database_path: PathBuf,
    pub model_path: PathBuf,
    pub models_whisper_dir: PathBuf,
    pub audio_dir: PathBuf,
    pub adapters_dir: PathBuf,
    pub manifests_dir: PathBuf,
    pub decoded_cache_dir: PathBuf,
    pub trainer_cache_dir: PathBuf,
}

pub fn resolve_xdg_paths(config: &IdiolectConfig, xdg: &XdgBaseDirs) -> ResolvedConfigPaths {
    let data_root = config
        .storage
        .data_dir
        .as_deref()
        .map(Path::new)
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg.data_home.join("idiolect"));

    let socket_path = config
        .daemon
        .socket_path
        .as_deref()
        .map(Path::new)
        .map(PathBuf::from)
        .unwrap_or_else(|| xdg.runtime_dir.join("idiolect.sock"));

    let database_path = config
        .storage
        .database_path
        .as_deref()
        .map(Path::new)
        .map(PathBuf::from)
        .unwrap_or_else(|| data_root.join("db").join("idiolect.sqlite"));

    let models_dir = data_root.join("models");
    let models_whisper_dir = models_dir.join("whisper");
    let model_path = models_whisper_dir.join(format!("{}.bin", config.asr.model));

    ResolvedConfigPaths {
        config_file: xdg.config_home.join("idiolect").join("config.toml"),
        socket_path,
        database_path,
        model_path,
        models_whisper_dir,
        audio_dir: data_root.join("audio"),
        adapters_dir: data_root.join("adapters"),
        manifests_dir: data_root.join("manifests"),
        decoded_cache_dir: xdg.cache_home.join("idiolect").join("decoded"),
        trainer_cache_dir: xdg.cache_home.join("idiolect").join("trainer"),
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to parse configuration")]
    ParseError,
    #[error("invalid configuration: {field}")]
    ValidationError { field: String },
}

fn default_default_user_id() -> String {
    "default".to_owned()
}

fn default_daemon_log_level() -> String {
    "info".to_owned()
}

fn default_audio_input_device() -> String {
    "default".to_owned()
}

fn default_audio_capture_sample_rate() -> u32 {
    48_000
}

fn default_audio_processing_sample_rate() -> u32 {
    16_000
}

fn default_audio_channels() -> u8 {
    1
}

fn default_vad_engine() -> String {
    "silero".to_owned()
}

fn default_vad_threshold() -> f32 {
    0.5
}

fn default_vad_min_speech_ms() -> u32 {
    250
}

fn default_vad_pre_roll_ms() -> u32 {
    300
}

fn default_vad_post_roll_ms() -> u32 {
    700
}

fn default_vad_max_utterance_ms() -> u32 {
    30_000
}

fn default_asr_engine() -> String {
    "whisper-rs".to_owned()
}

fn default_asr_model() -> String {
    "whisper-medium-en".to_owned()
}

fn default_asr_language() -> String {
    "en".to_owned()
}

fn default_asr_use_gpu() -> bool {
    true
}

fn default_asr_threads() -> u32 {
    8
}

fn default_storage_audio_codec() -> String {
    "opus".to_owned()
}

fn default_storage_audio_container() -> String {
    "ogg".to_owned()
}

fn default_storage_opus_bitrate_bps() -> u32 {
    24_000
}

fn default_storage_high_value_opus_bitrate_bps() -> u32 {
    32_000
}

fn default_training_min_approved_examples() -> u32 {
    50
}

fn default_training_trainer() -> String {
    "rust-native-lora".to_owned()
}

fn default_history_retention_days() -> u32 {
    1
}

fn default_history_max_entries() -> u32 {
    10
}

fn validate_non_empty_string(field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        Err(ConfigError::ValidationError {
            field: field.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn env_path_or_fallback(name: &str, fallback: PathBuf) -> PathBuf {
    match env::var_os(name) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => fallback,
    }
}
