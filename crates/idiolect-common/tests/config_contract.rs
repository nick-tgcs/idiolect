use idiolect_common::config::{resolve_xdg_paths, IdiolectConfig, XdgBaseDirs};

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
    let error = config.validate().expect_err("unknown input language must be rejected");
    assert!(format!("{error}").contains("translation.input_language"));

    config.translation.input_language = "auto".to_owned();
    config.translation.output_language = "klingon".to_owned();
    let error = config.validate().expect_err("unknown output language must be rejected");
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
    assert!(LANGUAGES.len() >= 99, "expected the full Whisper set, got {}", LANGUAGES.len());
    for (code, name) in LANGUAGES {
        assert!(!code.is_empty() && !name.is_empty());
        assert!(is_supported_language(code), "{code} should be supported");
    }
    assert_eq!(language_name("en"), Some("English"));
    assert_eq!(language_name("sv"), Some("Swedish"));
    assert_eq!(language_name("ja"), Some("Japanese"));
    assert_eq!(language_name("klingon"), None);
    assert!(!is_supported_language("auto"), "auto is a hint, not a language");
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
    let error = config.validate().expect_err("absurd retention must be rejected");
    assert!(format!("{error}").to_lowercase().contains("training_retention_days"));
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

    assert!(paths_text.contains("models/whisper"));
    assert!(paths_text.contains("audio"));
    assert!(paths_text.contains("adapters"));
    assert!(paths_text.contains("manifests"));
    assert!(paths_text.contains("db/idiolect.sqlite"));
    assert!(paths_text.contains("decoded"));
    assert!(paths_text.contains("trainer"));

    let unsafe_log_fragment = "raw transcript";
    assert!(!paths_text.to_lowercase().contains(unsafe_log_fragment));
    assert!(!paths_text.to_lowercase().contains("corrected transcript"));
    assert!(!paths_text.to_lowercase().contains("app text"));
}
