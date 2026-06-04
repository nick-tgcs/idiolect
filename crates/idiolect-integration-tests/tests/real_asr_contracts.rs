use idiolect_adapter_whisper::WhisperAsr;
use idiolect_ports::asr::AsrPort;
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

#[test]
fn whisper_transcribes_fixture_audio() {
    let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
    let audio = restart_traffic_fixture_16khz_mono();
    let draft = adapter
        .transcribe(&audio)
        .expect("fixture audio should transcribe");
    let text = draft.text.to_lowercase();

    assert!(text.contains("restart"));
    assert!(text.contains("traffic"));
    assert_eq!(draft.metadata.engine_name, "whisper-rs");
    assert_eq!(draft.metadata.engine_version, "0.16.0");
}

#[test]
fn whisper_reports_capabilities_without_backend_type_leakage() {
    let adapter = WhisperAsr::load_fixture_model().expect("fixture model should be present");
    let capabilities = adapter.capabilities();

    assert_eq!(capabilities.name, "whisper-rs");
    assert_eq!(capabilities.version, "0.16.0");
    assert!(!capabilities.supports_streaming);
    assert!(!capabilities.supports_word_timestamps);
    assert!(!capabilities.supports_confidence);
    assert!(!capabilities.supports_gpu);
    assert!(!capabilities.supports_incremental_updates);
    assert!(!std::any::type_name::<WhisperAsr>().contains("whisper_rs"));
}
