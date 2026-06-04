use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_ports::asr::AsrPort;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::{
    restart_traffic_fixture_16khz_mono, speech_and_silence_fixture_16khz_mono,
};

#[test]
fn real_adapter_matrix_processes_fixture_audio() {
    let restart_traffic = restart_traffic_fixture_16khz_mono();
    let source = speech_and_silence_fixture_16khz_mono();
    assert!(source.duration_ms > restart_traffic.duration_ms);

    let codec = OpusCodec::new();
    let encoded = codec.encode(&source).expect("fixture should encode");
    assert_eq!(encoded.codec_name, "opus");
    assert_eq!(encoded.sample_rate_hz, 16_000);
    assert_eq!(encoded.channels, 1);

    let decoded = codec.decode(&encoded).expect("fixture should decode");
    assert_eq!(decoded.sample_rate_hz, 16_000);
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.duration_ms, source.duration_ms);
    assert_eq!(decoded.sample_count(), source.sample_count());

    let mut vad = VadAdapter::new();
    let segments = vad.segment(&decoded).expect("fixture should segment");
    assert_eq!(segments.len(), 1);

    let whisper = WhisperAsr::load_fixture_model().expect("fixture model should be present");
    let draft = whisper
        .transcribe(&segments[0])
        .expect("speech segment should transcribe");

    let text = draft.text.to_lowercase();
    assert!(text.contains("restart"));
    assert!(text.contains("traffic"));
}
