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
    pub translation: TranslationConfig,
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

        if self.vad.auto_stop_silence_ms != 0
            && self.vad.auto_stop_silence_ms < self.vad.post_roll_ms
        {
            return Err(ConfigError::ValidationError {
                field: "vad.auto_stop_silence_ms".to_owned(),
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
        // Training-data retention is free-form (presets + custom): any value from 0
        // (keep forever) up to the sanity cap is allowed.
        if self.history.training_retention_days > MAX_TRAINING_RETENTION_DAYS {
            return Err(ConfigError::ValidationError {
                field: "history.training_retention_days".to_owned(),
            });
        }

        self.translation.validate()?;

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
    /// Command used to surface daemon-side problems to the user as a desktop
    /// notification, invoked as `<command> <summary> <body>` (best-effort —
    /// a missing binary is ignored). Empty disables notifications.
    #[serde(default = "default_daemon_notify_command")]
    pub notify_command: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            log_level: default_daemon_log_level(),
            notify_command: default_daemon_notify_command(),
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

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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
    /// Opt-in: continuous silence (after the take's first speech) that ends the
    /// take automatically, as if the user had pressed the toggle — popping the
    /// single review dialog or finalizing the streamed text. `0` (the default)
    /// disables it: listening never times out and only the toggle stops a take.
    /// Must be at least `post_roll_ms` when nonzero, or a take could end before
    /// one snippet pause ever completes.
    #[serde(default = "default_vad_auto_stop_silence_ms")]
    pub auto_stop_silence_ms: u32,
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
            auto_stop_silence_ms: default_vad_auto_stop_silence_ms(),
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_history_max_entries")]
    pub max_entries: u32,
    /// How long captured training data (audio + transcript + correction) is kept
    /// before the background prune purges it. Distinct from `retention_days`,
    /// which only bounds the tray's recent-history list. `0` disables the prune
    /// (keep forever). Defaults to one year.
    #[serde(default = "default_training_retention_days")]
    pub training_retention_days: u32,
    /// Seconds after which a history entry copied to the clipboard is cleared.
    /// `0` disables auto-clear.
    #[serde(default = "default_history_clipboard_auto_clear_secs")]
    pub clipboard_auto_clear_secs: u64,
    /// Encrypt history text at rest using the configured key. Defaults to `false`
    /// so the feature can be rolled out deliberately (a lost key means lost
    /// history, and toggling it on a populated database requires a fresh store).
    #[serde(default)]
    pub encrypt_at_rest: bool,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            retention_days: default_history_retention_days(),
            max_entries: default_history_max_entries(),
            training_retention_days: default_training_retention_days(),
            clipboard_auto_clear_secs: default_history_clipboard_auto_clear_secs(),
            encrypt_at_rest: false,
        }
    }
}

impl HistoryConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.retention_days == 0 {
            return Err(ConfigError::ValidationError {
                field: "history.retention_days".to_owned(),
            });
        }
        if self.max_entries == 0 {
            return Err(ConfigError::ValidationError {
                field: "history.max_entries".to_owned(),
            });
        }
        Ok(())
    }
}

/// Pause-triggered live translation: when enabled, each VAD-detected speech
/// snippet is translated from `input_language` to `output_language` as the user
/// pauses, instead of plain same-language dictation. These are the config-file
/// defaults; per-setting tray overrides persisted in `tray_settings` take
/// precedence at runtime (same layering as [`HistoryConfig`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranslationConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Source language code, or `"auto"` to let the ASR engine detect it.
    #[serde(default = "default_translation_input_language")]
    pub input_language: String,
    /// Target language code. Must be a concrete language (never `"auto"`).
    /// `"en"` runs on Whisper's built-in translate task with no extra tooling;
    /// any other target needs `command`.
    #[serde(default = "default_translation_output_language")]
    pub output_language: String,
    /// External translator invoked as `<command> <input_lang> <output_lang>`
    /// with the source text on stdin and the translation expected on stdout
    /// (exit 0). Empty means "not configured": targets other than English are
    /// then rejected at runtime with a clear error.
    #[serde(default)]
    pub command: String,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            input_language: default_translation_input_language(),
            output_language: default_translation_output_language(),
            command: String::new(),
        }
    }
}

impl TranslationConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.input_language != "auto"
            && !crate::languages::is_supported_language(&self.input_language)
        {
            return Err(ConfigError::ValidationError {
                field: "translation.input_language".to_owned(),
            });
        }
        if !crate::languages::is_supported_language(&self.output_language) {
            return Err(ConfigError::ValidationError {
                field: "translation.output_language".to_owned(),
            });
        }
        Ok(())
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

/// The operating-system family Idiolect resolves paths and socket limits for.
/// Carried as a value (rather than read straight from `cfg!`) so the per-platform
/// layout is a pure function both OSes' test suites can exercise on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

impl Platform {
    /// The platform this binary was compiled for. The single `cfg` seam: every
    /// other platform decision flows from the returned value, so the layout/limit
    /// logic stays testable for both targets regardless of the build host.
    #[must_use]
    pub fn host() -> Self {
        #[cfg(target_os = "macos")]
        {
            Platform::MacOs
        }
        #[cfg(not(target_os = "macos"))]
        {
            Platform::Linux
        }
    }
}

#[derive(Debug, Clone)]
pub struct XdgBaseDirs {
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub cache_home: PathBuf,
    pub runtime_dir: PathBuf,
}

impl XdgBaseDirs {
    /// The per-platform *default* base directories rooted at `home` (and `tmp`
    /// for the control socket), with **no** environment lookups. Pure and
    /// deterministic, so both layouts are unit-/contract-testable on any host;
    /// `for_platform` wraps this with the `XDG_*`/`TMPDIR`/`HOME` env resolution.
    ///
    /// Linux follows the XDG base-directory spec. macOS follows Apple's File
    /// System conventions: config and data both under `Library/Application
    /// Support` (a TOML config has no place in plist-only `Library/Preferences`),
    /// caches under `Library/Caches`, and the socket in the per-user temp dir
    /// (macOS has no `XDG_RUNTIME_DIR`).
    #[must_use]
    pub fn platform_defaults(platform: Platform, home: &Path, tmp: &Path) -> Self {
        match platform {
            Platform::Linux => Self {
                config_home: home.join(".config"),
                data_home: home.join(".local").join("share"),
                cache_home: home.join(".cache"),
                runtime_dir: home.join(".local").join("run").join("idiolect"),
            },
            Platform::MacOs => {
                let app_support = home.join("Library").join("Application Support");
                Self {
                    config_home: app_support.clone(),
                    data_home: app_support,
                    cache_home: home.join("Library").join("Caches"),
                    runtime_dir: tmp.to_path_buf(),
                }
            }
        }
    }

    /// Resolve the base directories for `platform`, honouring `HOME`/`TMPDIR` and
    /// any `XDG_*` overrides on top of [`Self::platform_defaults`]. The `XDG_*`
    /// overrides are honoured on every platform so power users and tests can
    /// redirect paths uniformly; only the fallbacks differ by OS.
    #[must_use]
    pub fn for_platform(platform: Platform) -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let tmp = env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let defaults = Self::platform_defaults(platform, &home, &tmp);

        Self {
            config_home: env_path_or_fallback("XDG_CONFIG_HOME", defaults.config_home),
            data_home: env_path_or_fallback("XDG_DATA_HOME", defaults.data_home),
            cache_home: env_path_or_fallback("XDG_CACHE_HOME", defaults.cache_home),
            runtime_dir: env_path_or_fallback("XDG_RUNTIME_DIR", defaults.runtime_dir),
        }
    }
}

impl Default for XdgBaseDirs {
    fn default() -> Self {
        Self::for_platform(Platform::host())
    }
}

/// The capacity of `sun_path` in `struct sockaddr_un`, **including** the trailing
/// NUL, for `platform`. A Unix-domain socket path that fills or exceeds this can
/// never be `bind`-ed; the kernel rejects it with a bare `EINVAL`. macOS' budget
/// is shorter than Linux', so a path that binds on Linux can fail on macOS.
///
/// The literals are the load-bearing platform fact: Linux `<sys/un.h>` declares
/// `char sun_path[108]`; BSD/macOS `<sys/un.h>` declares `char sun_path[104]`
/// (mirrored by `libc`'s `sockaddr_un`). Pinned at unit level in `platform_tests`.
#[must_use]
pub fn max_socket_path_len(platform: Platform) -> usize {
    match platform {
        Platform::Linux => 108,
        Platform::MacOs => 104,
    }
}

/// Reject a Unix-domain socket path too long to `bind` on `platform`, before the
/// kernel turns it into an opaque `EINVAL`. The usable budget is
/// [`max_socket_path_len`] minus one byte for the NUL terminator.
pub fn check_socket_path_len(path: &Path, platform: Platform) -> Result<(), ConfigError> {
    use std::os::unix::ffi::OsStrExt;

    let len = path.as_os_str().as_bytes().len();
    let max = max_socket_path_len(platform);
    if len >= max {
        return Err(ConfigError::SocketPathTooLong { len, max });
    }
    Ok(())
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

impl ResolvedConfigPaths {
    /// Reject a resolved control-socket path the host kernel could not `bind`
    /// because it overflows `sun_path`. Validated against [`Platform::host`] so
    /// the daemon fails fast with a clear message instead of a bare `EINVAL`.
    pub fn validate_socket_path(&self) -> Result<(), ConfigError> {
        check_socket_path_len(&self.socket_path, Platform::host())
    }
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
    #[error(
        "socket path is too long: {len} bytes exceeds this platform's {max}-byte sun_path limit"
    )]
    SocketPathTooLong { len: usize, max: usize },
}

fn default_default_user_id() -> String {
    "default".to_owned()
}

fn default_daemon_log_level() -> String {
    "info".to_owned()
}

fn default_daemon_notify_command() -> String {
    "notify-send".to_owned()
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

/// Disabled by default: listening never times out; only the toggle stops a take.
fn default_vad_auto_stop_silence_ms() -> u32 {
    0
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

/// One year, in days — the default training-data retention window.
fn default_training_retention_days() -> u32 {
    365
}

/// Upper bound for training-data retention (~100 years); guards against typos in
/// a custom value while leaving any realistic choice valid.
pub const MAX_TRAINING_RETENTION_DAYS: u32 = 36_500;

fn default_history_max_entries() -> u32 {
    10
}

fn default_translation_input_language() -> String {
    "auto".to_owned()
}

fn default_translation_output_language() -> String {
    "en".to_owned()
}

fn default_history_clipboard_auto_clear_secs() -> u64 {
    30
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

#[cfg(test)]
mod platform_tests {
    use super::{check_socket_path_len, max_socket_path_len, Platform, XdgBaseDirs};
    use std::path::{Path, PathBuf};

    #[test]
    fn host_platform_matches_the_build_target() {
        // The one `cfg` seam: assert it agrees with the compile target so the
        // value-carried `Platform` can never silently disagree with the host.
        let expected = if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        };
        assert_eq!(Platform::host(), expected);
    }

    #[test]
    fn platform_defaults_are_pure_and_diverge_by_os() {
        let home = Path::new("/home/u");
        let tmp = Path::new("/tmp/runtime");
        let linux = XdgBaseDirs::platform_defaults(Platform::Linux, home, tmp);
        let macos = XdgBaseDirs::platform_defaults(Platform::MacOs, home, tmp);

        // Linux ignores `tmp`; macOS routes the socket there.
        assert_eq!(linux.runtime_dir, Path::new("/home/u/.local/run/idiolect"));
        assert_eq!(macos.runtime_dir, tmp);
        // macOS collapses config+data into one Application Support root.
        assert_eq!(macos.config_home, macos.data_home);
        assert_ne!(linux.config_home, linux.data_home);
    }

    #[test]
    fn sun_path_limits_match_the_sockaddr_un_definitions() {
        // The magic numbers that distinguish the two platforms — pinned at the
        // lowest altitude so a typo (e.g. 104 -> 140) fails here, not only in
        // the cross-platform contract tests. See `max_socket_path_len`.
        assert_eq!(max_socket_path_len(Platform::Linux), 108);
        assert_eq!(max_socket_path_len(Platform::MacOs), 104);
    }

    #[test]
    fn socket_len_guard_reserves_one_byte_for_the_nul() {
        // The boundary is exact: a path equal to the limit has no room for the
        // NUL and must be rejected; one byte shorter is the last accepted length.
        for platform in [Platform::Linux, Platform::MacOs] {
            let max = max_socket_path_len(platform);
            let at_limit = PathBuf::from("a".repeat(max));
            let one_under = PathBuf::from("a".repeat(max - 1));
            check_socket_path_len(&at_limit, platform)
                .expect_err("a path filling sun_path leaves no room for the NUL");
            check_socket_path_len(&one_under, platform)
                .expect("the last byte before the limit is usable");
        }
    }
}
