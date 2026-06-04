use idiolect_adapter_fixture_asr::FixtureAsr;
use idiolect_adapter_fixture_audio::FixtureAudio;
use idiolect_adapter_fixture_codec::FixtureCodec;
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::asr::AsrPort;
use idiolect_ports::audio::AudioInputPort;
use idiolect_ports::codec::AudioCodecPort;

#[test]
fn fixture_pipeline_contract_is_deterministic_end_to_end() {
    let session_id = ImeSessionId::new();
    let mut audio = FixtureAudio::new();
    let asr = FixtureAsr::new("restart traffic");
    let codec = FixtureCodec::new();

    audio
        .start_capture(session_id)
        .expect("fixture audio should start");
    let captured = audio
        .stop_capture(session_id)
        .expect("fixture audio should stop after start");

    let transcript = asr
        .transcribe(&captured)
        .expect("fixture asr should transcribe");
    let encoded = codec
        .encode(&captured)
        .expect("fixture codec should encode");
    let decoded = codec.decode(&encoded).expect("fixture codec should decode");

    assert_eq!(transcript.text, "restart traffic");
    assert_eq!(decoded.samples_f32_mono, captured.samples_f32_mono);
}
