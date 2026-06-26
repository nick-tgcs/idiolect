use std::path::{Path, PathBuf};
use std::sync::Mutex;

use idiolect_common::config::{
    check_socket_path_len, max_socket_path_len, resolve_xdg_paths, IdiolectConfig, PathProvider,
    Platform, RootedPaths, XdgBaseDirs, XdgPaths,
};

const MASTER_PLAN_TOML: &str = r#"
[user]
default_user_id = "default"

[daemon]
log_level = "info"

[audio]
input_device = "default"
capture_sample_rate = 48000
processing_sample_rate = 16000
channels = 1

[vad]
engine = "silero"
threshold = 0.5
min_speech_ms = 250
pre_roll_ms = 300
post_roll_ms = 700
max_utterance_ms = 30000
auto_stop_silence_ms = 0

[asr]
engine = "whisper-rs"
model = "whisper-medium-en"
language = "en"
use_gpu = true
threads = 8

[storage]
audio_codec = "opus"
audio_container = "ogg"
opus_bitrate_bps = 24000
high_value_opus_bitrate_bps = 32000

[training]
min_approved_examples = 50
trainer = "rust-native-lora"
auto_train = false

[privacy]

[translation]
enabled = false
input_language = "auto"
output_language = "en"
command = ""

[observability]
log_raw_transcripts = false
log_corrected_transcripts = false
log_surrounding_app_text = false
"#;

#[test]
fn config_defaults_match_master_plan() {
    let config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config must parse");
    config.validate().expect("master-plan config must validate");

    assert_eq!(config.user.default_user_id, "default");
    assert_eq!(config.daemon.log_level, "info");

    assert_eq!(config.audio.input_device, "default");
    assert_eq!(config.audio.capture_sample_rate, 48_000);
    assert_eq!(config.audio.processing_sample_rate, 16_000);
    assert_eq!(config.audio.channels, 1);

    assert_eq!(config.vad.engine, "silero");
    assert!((config.vad.threshold - 0.5).abs() < f32::EPSILON);
    assert_eq!(config.vad.min_speech_ms, 250);
    assert_eq!(config.vad.pre_roll_ms, 300);
    assert_eq!(config.vad.post_roll_ms, 700);
    assert_eq!(config.vad.max_utterance_ms, 30_000);
    assert_eq!(config.vad.auto_stop_silence_ms, 0);

    assert_eq!(config.asr.engine, "whisper-rs");
    assert_eq!(config.asr.model, "whisper-medium-en");
    assert_eq!(config.asr.language, "en");
    assert!(config.asr.use_gpu);
    assert_eq!(config.asr.threads, 8);

    assert_eq!(config.storage.audio_codec, "opus");
    assert_eq!(config.storage.audio_container, "ogg");
    assert_eq!(config.storage.opus_bitrate_bps, 24_000);
    assert_eq!(config.storage.high_value_opus_bitrate_bps, 32_000);

    assert_eq!(config.training.min_approved_examples, 50);
    assert_eq!(config.training.trainer, "rust-native-lora");
    assert!(!config.training.auto_train);

    assert!(!config.observability.log_raw_transcripts);
    assert!(!config.observability.log_corrected_transcripts);
    assert!(!config.observability.log_surrounding_app_text);

    assert!(!config.translation.enabled);
    assert_eq!(config.translation.input_language, "auto");
    assert_eq!(config.translation.output_language, "en");
    assert_eq!(config.translation.command, "");
}

#[test]
fn translation_section_defaults_when_omitted() {
    // A config without `[translation]` (every pre-translation install) must keep
    // parsing, with translation off and an any-language-to-English default.
    let config = IdiolectConfig::from_toml_str("[user]\ndefault_user_id = \"default\"\n")
        .expect("minimal config must parse");
    assert!(!config.translation.enabled);
    assert_eq!(config.translation.input_language, "auto");
    assert_eq!(config.translation.output_language, "en");
    assert_eq!(config.translation.command, "");
}

#[test]
fn translation_accepts_any_known_language_pair() {
    let mut config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");
    config.translation.enabled = true;

    // "Any language" means the whole Whisper set, in either direction, plus
    // auto-detection on input only.
    for (input, output) in [("auto", "en"), ("sv", "en"), ("en", "ja"), ("de", "fr")] {
        config.translation.input_language = input.to_owned();
        config.translation.output_language = output.to_owned();
        config
            .validate()
            .unwrap_or_else(|e| panic!("{input} -> {output} should validate: {e}"));
    }
}

#[test]
fn translation_rejects_unknown_languages_and_auto_output() {
    let mut config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");
    config.translation.enabled = true;

    config.translation.input_language = "klingon".to_owned();
    config.translation.output_language = "en".to_owned();
    let error = config
        .validate()
        .expect_err("unknown input language must be rejected");
    assert!(format!("{error}").contains("translation.input_language"));

    config.translation.input_language = "auto".to_owned();
    config.translation.output_language = "klingon".to_owned();
    let error = config
        .validate()
        .expect_err("unknown output language must be rejected");
    assert!(format!("{error}").contains("translation.output_language"));

    // "auto" only makes sense as an input: the output must be a concrete language.
    config.translation.output_language = "auto".to_owned();
    let error = config.validate().expect_err("auto output must be rejected");
    assert!(format!("{error}").contains("translation.output_language"));
}

#[test]
fn language_catalogue_covers_the_whisper_set_with_display_names() {
    use idiolect_common::languages::{is_supported_language, language_name, LANGUAGES};

    // The catalogue is what both config validation and the tray menus offer, so
    // it must cover the full Whisper language set ("any language").
    assert!(
        LANGUAGES.len() >= 99,
        "expected the full Whisper set, got {}",
        LANGUAGES.len()
    );
    for (code, name) in LANGUAGES {
        assert!(!code.is_empty() && !name.is_empty());
        assert!(is_supported_language(code), "{code} should be supported");
    }
    assert_eq!(language_name("en"), Some("English"));
    assert_eq!(language_name("sv"), Some("Swedish"));
    assert_eq!(language_name("ja"), Some("Japanese"));
    assert_eq!(language_name("klingon"), None);
    assert!(
        !is_supported_language("auto"),
        "auto is a hint, not a language"
    );
}

#[test]
fn training_retention_defaults_to_one_year_when_omitted() {
    // The master-plan TOML has no `history.training_retention_days`, so the serde
    // default must fill in one year.
    let config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config must parse");
    assert_eq!(config.history.training_retention_days, 365);
}

#[test]
fn training_retention_accepts_presets_zero_and_custom_but_rejects_absurd_values() {
    let mut config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");

    // Presets, "keep forever" (0), and an arbitrary custom value all validate.
    for days in [0, 30, 365, 730, 3650, 540, 36_500] {
        config.history.training_retention_days = days;
        config
            .validate()
            .unwrap_or_else(|e| panic!("training_retention_days={days} should validate: {e}"));
    }

    // Beyond the sanity cap is rejected (guards against a fat-fingered custom value).
    config.history.training_retention_days = 36_501;
    let error = config
        .validate()
        .expect_err("absurd retention must be rejected");
    assert!(format!("{error}")
        .to_lowercase()
        .contains("training_retention_days"));
}

#[test]
fn auto_stop_defaults_off_and_must_exceed_the_snippet_pause() {
    // Omitted ⇒ disabled: listening NEVER times out by default — the take ends
    // only when the user toggles (Super+T). Silence auto-stop is strictly
    // opt-in for those who want it.
    let config = IdiolectConfig::from_toml_str("[user]\ndefault_user_id = \"default\"\n")
        .expect("minimal config must parse");
    assert_eq!(config.vad.auto_stop_silence_ms, 0);

    let mut config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");

    // 0 disables auto-stop (manual toggle only).
    config.vad.auto_stop_silence_ms = 0;
    config.validate().expect("0 (disabled) must validate");

    // A nonzero value below the snippet pause threshold could end the take
    // before a single snippet ever completes — reject it.
    config.vad.auto_stop_silence_ms = config.vad.post_roll_ms - 1;
    let error = config
        .validate()
        .expect_err("sub-pause auto-stop must be rejected");
    assert!(format!("{error}").contains("auto_stop_silence_ms"));

    config.vad.auto_stop_silence_ms = config.vad.post_roll_ms;
    config
        .validate()
        .expect("equal to the pause threshold is allowed");
}

#[test]
fn config_rejects_empty_user_id() {
    let mut config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");
    config.user.default_user_id = String::new();
    let validation = config.validate();
    assert!(validation.is_err());
    let error = validation.unwrap_err();
    let message = format!("{error}");
    assert!(message.to_lowercase().contains("user"));
    assert!(message.to_lowercase().contains("default_user_id"));
}

#[test]
fn config_resolves_xdg_paths_without_private_text() {
    let config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");
    let xdg = XdgBaseDirs::default();
    let paths = resolve_xdg_paths(&config, &xdg);
    let paths_text = format!("{paths:?}");

    // Use PathBuf::ends_with for multi-component checks: Debug-format escapes
    // backslashes on Windows, making string-contains unreliable across platforms.
    assert!(paths
        .models_whisper_dir
        .ends_with(Path::new("models").join("whisper")));
    assert!(paths
        .database_path
        .ends_with(Path::new("db").join("idiolect.sqlite")));
    assert!(paths_text.contains("audio"));
    assert!(paths_text.contains("adapters"));
    assert!(paths_text.contains("manifests"));
    assert!(paths_text.contains("decoded"));
    assert!(paths_text.contains("trainer"));

    let unsafe_log_fragment = "raw transcript";
    assert!(!paths_text.to_lowercase().contains(unsafe_log_fragment));
    assert!(!paths_text.to_lowercase().contains("corrected transcript"));
    assert!(!paths_text.to_lowercase().contains("app text"));
}

#[test]
fn notify_command_defaults_to_notify_send_and_is_overridable() {
    // Daemon-side user notifications (e.g. "translation unavailable") shell out
    // to `<notify_command> <summary> <body>`; notify-send is the desktop default.
    let config = IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("config must parse");
    assert_eq!(config.daemon.notify_command, "notify-send");

    let overridden = MASTER_PLAN_TOML.replace(
        "[daemon]\nlog_level = \"info\"",
        "[daemon]\nlog_level = \"info\"\nnotify_command = \"/opt/custom-notifier\"",
    );
    let config = IdiolectConfig::from_toml_str(&overridden).expect("override must parse");
    config.validate().expect("override must validate");
    assert_eq!(config.daemon.notify_command, "/opt/custom-notifier");
}

// ---------------------------------------------------------------------------
// Cross-platform base directories (macOS port — see docs/future/009-macos-port).
//
// `platform_defaults` is pure (no env lookups), so both OS layouts are asserted
// here regardless of which host runs the suite. This is what lets the Linux CI
// prove the macOS layout, and the future macOS runner prove the Linux one.
// ---------------------------------------------------------------------------

#[test]
fn linux_base_dirs_follow_the_xdg_layout() {
    let home = PathBuf::from("/home/ada");
    let tmp = PathBuf::from("/run/user/1000");
    let dirs = XdgBaseDirs::platform_defaults(Platform::Linux, &home, &tmp);

    assert_eq!(dirs.config_home, Path::new("/home/ada/.config"));
    assert_eq!(dirs.data_home, Path::new("/home/ada/.local/share"));
    assert_eq!(dirs.cache_home, Path::new("/home/ada/.cache"));
    // Linux falls back to a home-relative runtime dir; `tmp` is unused here.
    assert_eq!(dirs.runtime_dir, Path::new("/home/ada/.local/run/idiolect"));
}

#[test]
fn macos_base_dirs_follow_the_apple_layout() {
    let home = PathBuf::from("/Users/ada");
    let tmp = PathBuf::from("/var/folders/q5/abc/T");
    let dirs = XdgBaseDirs::platform_defaults(Platform::MacOs, &home, &tmp);

    // Config and data both live under Application Support (macOS has no XDG
    // split); a TOML config belongs there, not in plist-only ~/Library/Preferences.
    assert_eq!(
        dirs.config_home,
        Path::new("/Users/ada/Library/Application Support")
    );
    assert_eq!(
        dirs.data_home,
        Path::new("/Users/ada/Library/Application Support")
    );
    assert_eq!(dirs.cache_home, Path::new("/Users/ada/Library/Caches"));
    // The control socket lives in the per-user temp dir (`$TMPDIR`); macOS has
    // no XDG_RUNTIME_DIR, and TMPDIR is short enough to stay within sun_path.
    assert_eq!(dirs.runtime_dir, Path::new("/var/folders/q5/abc/T"));
}

#[test]
fn macos_resolved_paths_land_under_application_support() {
    let config =
        IdiolectConfig::from_toml_str(MASTER_PLAN_TOML).expect("master-plan config should parse");
    let dirs = XdgBaseDirs::platform_defaults(
        Platform::MacOs,
        Path::new("/Users/ada"),
        Path::new("/var/folders/q5/abc/T"),
    );
    let paths = resolve_xdg_paths(&config, &dirs);

    assert_eq!(
        paths.config_file,
        Path::new("/Users/ada/Library/Application Support/idiolect/config.toml")
    );
    assert_eq!(
        paths.socket_path,
        Path::new("/var/folders/q5/abc/T/idiolect.sock")
    );
    assert!(paths
        .database_path
        .starts_with("/Users/ada/Library/Application Support/idiolect"));
    assert!(paths
        .decoded_cache_dir
        .starts_with("/Users/ada/Library/Caches/idiolect"));
}

// ---------------------------------------------------------------------------
// Socket-path length guard. A `sockaddr_un` holds a fixed `sun_path`; exceed it
// and `bind` fails with a bare EINVAL. macOS' budget (104) is shorter than
// Linux' (108), so a path that binds on Linux can fail on macOS.
// ---------------------------------------------------------------------------

#[test]
fn socket_path_limit_is_shorter_on_macos() {
    assert_eq!(max_socket_path_len(Platform::Linux), 108);
    assert_eq!(max_socket_path_len(Platform::MacOs), 104);
}

#[test]
fn overlong_socket_paths_are_rejected_per_platform() {
    // 103 usable bytes fit on both; 104 fills macOS' budget (no room for NUL)
    // but still fits Linux; 108 overflows both.
    let p103 = PathBuf::from(format!("/{}", "a".repeat(102)));
    assert_eq!(p103.as_os_str().len(), 103);
    let p104 = PathBuf::from(format!("/{}", "a".repeat(103)));
    let p108 = PathBuf::from(format!("/{}", "a".repeat(107)));

    check_socket_path_len(&p103, Platform::MacOs).expect("103 bytes fits macOS");
    check_socket_path_len(&p103, Platform::Linux).expect("103 bytes fits Linux");

    check_socket_path_len(&p104, Platform::MacOs).expect_err("104 bytes overflows macOS sun_path");
    check_socket_path_len(&p104, Platform::Linux).expect("104 bytes still fits Linux");

    check_socket_path_len(&p108, Platform::MacOs).expect_err("108 bytes overflows macOS");
    check_socket_path_len(&p108, Platform::Linux).expect_err("108 bytes overflows Linux");
}

#[test]
fn rejected_socket_path_error_names_the_limit() {
    let too_long = PathBuf::from(format!("/{}", "a".repeat(200)));
    let error = check_socket_path_len(&too_long, Platform::MacOs)
        .expect_err("a 201-byte path must be rejected");
    let message = format!("{error}").to_lowercase();
    assert!(
        message.contains("socket path"),
        "names the offending path: {message}"
    );
    assert!(
        message.contains("104"),
        "names the platform limit: {message}"
    );
}

// ---------------------------------------------------------------------------
// `for_platform` env-resolution layer: HOME/TMPDIR resolution, the per-key
// `XDG_*` overrides layered on the platform defaults, and the `/tmp` fallbacks.
// This is the seam the macOS port introduces (TMPDIR routing the socket, `XDG_*`
// honoured on every OS) — covered here, not just the pure `platform_defaults`.
//
// `for_platform` reads process-global env, so these serialize on a mutex and
// restore the prior values; assertions run *after* restore so a failure can't
// leave the environment dirty for other tests in this binary.
// ---------------------------------------------------------------------------

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with `vars` applied to the process environment (`Some` sets, `None`
/// removes), serialized against other env tests, restoring the prior values
/// before returning `f`'s result.
fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let saved: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(key, _)| ((*key).to_owned(), std::env::var(key).ok()))
        .collect();
    for (key, value) in vars {
        match value {
            Some(val) => std::env::set_var(key, val),
            None => std::env::remove_var(key),
        }
    }
    let result = f();
    for (key, value) in saved {
        match value {
            Some(val) => std::env::set_var(&key, val),
            None => std::env::remove_var(&key),
        }
    }
    result
}

const XDG_VARS: [&str; 4] = [
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
];

fn cleared_xdg() -> Vec<(&'static str, Option<&'static str>)> {
    XDG_VARS.iter().map(|key| (*key, None)).collect()
}

#[test]
fn for_platform_macos_routes_the_socket_through_tmpdir() {
    let mut vars = cleared_xdg();
    vars.push(("HOME", Some("/Users/ada")));
    vars.push(("TMPDIR", Some("/var/folders/q5/abc/T")));
    let dirs = with_env(&vars, || XdgBaseDirs::for_platform(Platform::MacOs));

    // The macOS socket follows $TMPDIR (catches a per-key swap that would route
    // runtime_dir to, say, the Caches default instead).
    assert_eq!(dirs.runtime_dir, Path::new("/var/folders/q5/abc/T"));
    assert_eq!(
        dirs.config_home,
        Path::new("/Users/ada/Library/Application Support")
    );
}

#[test]
fn for_platform_falls_back_to_tmp_when_home_and_tmpdir_are_absent() {
    let mut vars = cleared_xdg();
    vars.push(("HOME", None));
    vars.push(("TMPDIR", None));
    let dirs = with_env(&vars, || XdgBaseDirs::for_platform(Platform::MacOs));

    // Both fall back to /tmp: HOME→/tmp gives /tmp/Library/...; TMPDIR→/tmp
    // gives the socket dir.
    assert_eq!(
        dirs.config_home,
        Path::new("/tmp/Library/Application Support")
    );
    assert_eq!(dirs.runtime_dir, Path::new("/tmp"));
}

#[test]
fn for_platform_lets_each_xdg_override_win_over_its_own_default() {
    // Each override must land on its matching field — not a neighbour's. Setting
    // all four to distinct paths pins the wiring against a copy/paste swap.
    let dirs = with_env(
        &[
            ("HOME", Some("/Users/ada")),
            ("TMPDIR", Some("/var/folders/q5/abc/T")),
            ("XDG_CONFIG_HOME", Some("/over/config")),
            ("XDG_DATA_HOME", Some("/over/data")),
            ("XDG_CACHE_HOME", Some("/over/cache")),
            ("XDG_RUNTIME_DIR", Some("/over/run")),
        ],
        || XdgBaseDirs::for_platform(Platform::MacOs),
    );

    assert_eq!(dirs.config_home, Path::new("/over/config"));
    assert_eq!(dirs.data_home, Path::new("/over/data"));
    assert_eq!(dirs.cache_home, Path::new("/over/cache"));
    // XDG_RUNTIME_DIR wins over TMPDIR for the socket.
    assert_eq!(dirs.runtime_dir, Path::new("/over/run"));
}

#[test]
fn for_platform_linux_keeps_the_xdg_layout_through_env_resolution() {
    let mut vars = cleared_xdg();
    vars.push(("HOME", Some("/home/ada")));
    vars.push(("TMPDIR", Some("/should/be/ignored/on/linux")));
    let dirs = with_env(&vars, || XdgBaseDirs::for_platform(Platform::Linux));

    assert_eq!(dirs.config_home, Path::new("/home/ada/.config"));
    assert_eq!(dirs.data_home, Path::new("/home/ada/.local/share"));
    assert_eq!(dirs.cache_home, Path::new("/home/ada/.cache"));
    // Linux ignores TMPDIR and uses the home-relative runtime dir — proving the
    // pre-port Default behaviour is preserved through the new env wrapper.
    assert_eq!(dirs.runtime_dir, Path::new("/home/ada/.local/run/idiolect"));
}

#[test]
fn path_provider_is_usable_through_a_trait_object() {
    // The daemon and the FFI facade both hold a `&dyn PathProvider`; the desktop
    // and Android impls must be interchangeable behind that seam.
    let android: Box<dyn PathProvider> =
        Box::new(RootedPaths::new("/data/user/0/dev.idiolect/files"));
    let xdg =
        XdgBaseDirs::platform_defaults(Platform::Linux, Path::new("/home/ada"), Path::new("/tmp"));
    let desktop: Box<dyn PathProvider> = Box::new(XdgPaths::new(xdg));

    for provider in [&android, &desktop] {
        // The database always lives directly under the provider's data dir,
        // regardless of how that dir was resolved.
        assert_eq!(
            provider.database_path(),
            provider.data_dir().join("idiolect.db")
        );
        assert_eq!(provider.audio_dir(), provider.data_dir().join("audio"));
    }
    assert_eq!(
        android.data_dir(),
        Path::new("/data/user/0/dev.idiolect/files")
    );
    assert_eq!(
        desktop.data_dir(),
        Path::new("/home/ada/.local/share/idiolect")
    );
}
