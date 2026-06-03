use idiolect_common::ids::ImeSessionId;

pub use idiolect_core::domain::adapter::AudioSegment;

pub trait AudioInputPort {
    type Error;

    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error>;
    fn stop_capture(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error>;
}
