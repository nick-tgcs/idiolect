pub use crate::audio::{AudioSegment, EncodedAudio};

pub trait AudioCodecPort {
    type Error;

    fn encode(&self, audio: &AudioSegment) -> Result<EncodedAudio, Self::Error>;
    fn decode(&self, encoded: &EncodedAudio) -> Result<AudioSegment, Self::Error>;
}
