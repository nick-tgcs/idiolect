use idiolect_adapter_cpal::{CpalAudioInput, CpalAudioInputError};

#[test]
fn cpal_missing_named_device_is_deterministic() {
    let result = CpalAudioInput::open_device_by_name("__idiolect_missing_device__");

    assert!(matches!(result, Err(CpalAudioInputError::DeviceNotFound)));
}
