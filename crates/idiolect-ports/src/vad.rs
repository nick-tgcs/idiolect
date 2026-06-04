pub use crate::audio::AudioSegment;

pub trait VadPort {
    type Error;

    fn segment(&mut self, audio: &AudioSegment) -> Result<Vec<AudioSegment>, Self::Error>;
}
