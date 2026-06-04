use idiolect_adapter_fixture_audio::FixtureAudio;
use idiolect_adapter_opus::OpusCodec;
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::audio::AudioInputPort;
use idiolect_ports::codec::AudioCodecPort;

#[test]
fn opus_codec_round_trips_fixture_metadata() {
    let session_id = ImeSessionId::new();
    let mut audio = FixtureAudio::new();
    let codec = OpusCodec::new();

    audio
        .start_capture(session_id)
        .expect("fixture audio should start");
    let captured = audio
        .stop_capture(session_id)
        .expect("fixture audio should stop after start");

    let encoded = codec
        .encode(&captured)
        .expect("fixture audio should encode");
    let decoded = codec.decode(&encoded).expect("fixture audio should decode");

    assert_eq!(encoded.codec_name, "opus");
    assert_eq!(decoded.sample_rate_hz, captured.sample_rate_hz);
    assert_eq!(decoded.channels, captured.channels);
    assert_eq!(decoded.duration_ms, captured.duration_ms);
    assert_eq!(decoded.sample_count(), captured.sample_count());
}
