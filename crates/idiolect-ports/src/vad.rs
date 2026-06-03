pub use idiolect_core::domain::adapter::AudioSegment;

pub trait VadPort {
    type Error;

    fn segment(&mut self, audio: &AudioSegment) -> Result<Vec<AudioSegment>, Self::Error>;
}
