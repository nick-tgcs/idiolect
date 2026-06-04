use idiolect_adapter_vad::VadAdapter;
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;

#[test]
fn vad_contract_returns_one_segment_for_fixture() {
    let mut adapter = VadAdapter::new();
    let segments = adapter
        .segment(&speech_and_silence_fixture_16khz_mono())
        .expect("fixture should segment");

    assert_eq!(segments.len(), 1);
}
