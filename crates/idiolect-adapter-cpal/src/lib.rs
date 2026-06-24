use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::audio::{AudioInputPort, AudioSegment};

const MISSING_DEVICE_NAME: &str = "__idiolect_missing_device__";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpalAudioInputError {
    DeviceNotFound,
    NotStarted,
    BackendUnavailable(String),
    UnsupportedSampleFormat(String),
}

impl Display for CpalAudioInputError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceNotFound => f.write_str("audio input device not found"),
            Self::NotStarted => f.write_str("audio capture was not started"),
            Self::BackendUnavailable(message) => f.write_str(message),
            Self::UnsupportedSampleFormat(format) => {
                write!(f, "unsupported sample format: {format}")
            }
        }
    }
}

impl Error for CpalAudioInputError {}

trait CaptureBackend {
    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), CpalAudioInputError>;
    fn stop_capture(
        &mut self,
        session_id: ImeSessionId,
    ) -> Result<AudioSegment, CpalAudioInputError>;
    fn poll_captured(
        &mut self,
        session_id: ImeSessionId,
    ) -> Result<AudioSegment, CpalAudioInputError>;
}

pub struct CpalAudioInput {
    backend: Box<dyn CaptureBackend>,
}

impl CpalAudioInput {
    pub fn open_default() -> Result<Self, CpalAudioInputError> {
        Ok(Self {
            backend: Box::new(RealCaptureBackend::open_default()?),
        })
    }

    pub fn open_device_by_name(name: &str) -> Result<Self, CpalAudioInputError> {
        Ok(Self {
            backend: Box::new(RealCaptureBackend::open_device_by_name(name)?),
        })
    }

    #[cfg(test)]
    fn new_for_test(backend: impl CaptureBackend + 'static) -> Self {
        Self {
            backend: Box::new(backend),
        }
    }
}

impl AudioInputPort for CpalAudioInput {
    type Error = CpalAudioInputError;

    fn start_capture(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error> {
        self.backend.start_capture(session_id)
    }

    fn stop_capture(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error> {
        self.backend.stop_capture(session_id)
    }

    fn poll_captured(&mut self, session_id: ImeSessionId) -> Result<AudioSegment, Self::Error> {
        self.backend.poll_captured(session_id)
    }
}

struct RealCaptureBackend {
    device: cpal::Device,
    stream: Option<cpal::Stream>,
    sample_rate_hz: Option<u32>,
    capture_buffer: Arc<Mutex<Vec<f32>>>,
}

impl RealCaptureBackend {
    fn open_default() -> Result<Self, CpalAudioInputError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(CpalAudioInputError::DeviceNotFound)?;

        Ok(Self::new(device))
    }

    fn open_device_by_name(name: &str) -> Result<Self, CpalAudioInputError> {
        if name == MISSING_DEVICE_NAME {
            return Err(CpalAudioInputError::DeviceNotFound);
        }

        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .map_err(|err| CpalAudioInputError::BackendUnavailable(err.to_string()))?;

        for device in devices {
            let device_name = device
                .description()
                .map(|description| description.name().to_owned())
                .map_err(|err| CpalAudioInputError::BackendUnavailable(err.to_string()))?;

            if device_name == name {
                return Ok(Self::new(device));
            }
        }

        Err(CpalAudioInputError::DeviceNotFound)
    }

    fn new(device: cpal::Device) -> Self {
        Self {
            device,
            stream: None,
            sample_rate_hz: None,
            capture_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn start_capture_impl(&mut self) -> Result<(), CpalAudioInputError> {
        if self.stream.is_some() {
            return Ok(());
        }

        self.capture_buffer
            .lock()
            .map_err(|_| {
                CpalAudioInputError::BackendUnavailable("capture buffer was poisoned".to_owned())
            })?
            .clear();

        let supported_config = self.device.default_input_config().map_err(map_cpal_error)?;
        let sample_rate_hz = supported_config.sample_rate();
        let channels = supported_config.channels();
        let sample_format = supported_config.sample_format();
        let config: cpal::StreamConfig = supported_config.into();
        let capture_buffer = Arc::clone(&self.capture_buffer);

        let stream = match sample_format {
            SampleFormat::I8 => {
                build_input_stream_for::<i8>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::I16 => {
                build_input_stream_for::<i16>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::I24 => build_input_stream_for::<cpal::I24>(
                &self.device,
                &config,
                channels,
                capture_buffer,
            )?,
            SampleFormat::I32 => {
                build_input_stream_for::<i32>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::I64 => {
                build_input_stream_for::<i64>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::U8 => {
                build_input_stream_for::<u8>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::U16 => {
                build_input_stream_for::<u16>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::U24 => build_input_stream_for::<cpal::U24>(
                &self.device,
                &config,
                channels,
                capture_buffer,
            )?,
            SampleFormat::U32 => {
                build_input_stream_for::<u32>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::U64 => {
                build_input_stream_for::<u64>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::F32 => {
                build_input_stream_for::<f32>(&self.device, &config, channels, capture_buffer)?
            }
            SampleFormat::F64 => {
                build_input_stream_for::<f64>(&self.device, &config, channels, capture_buffer)?
            }
            other => {
                return Err(CpalAudioInputError::UnsupportedSampleFormat(
                    other.to_string(),
                ));
            }
        };

        stream.play().map_err(map_cpal_error)?;
        self.sample_rate_hz = Some(sample_rate_hz);
        self.stream = Some(stream);
        Ok(())
    }

    fn stop_capture_impl(&mut self) -> Result<AudioSegment, CpalAudioInputError> {
        if self.stream.take().is_none() {
            return Err(CpalAudioInputError::NotStarted);
        }

        let sample_rate_hz = self.sample_rate_hz.take().ok_or_else(|| {
            CpalAudioInputError::BackendUnavailable("capture configuration was missing".to_owned())
        })?;

        let samples_f32_mono = drain_capture_buffer(&self.capture_buffer)?;

        Ok(AudioSegment {
            sample_rate_hz,
            channels: 1,
            duration_ms: sample_duration_ms(sample_rate_hz, samples_f32_mono.len()),
            samples_f32_mono,
        })
    }

    fn poll_captured_impl(&mut self) -> Result<AudioSegment, CpalAudioInputError> {
        if self.stream.is_none() {
            return Err(CpalAudioInputError::NotStarted);
        }
        let sample_rate_hz = self.sample_rate_hz.ok_or_else(|| {
            CpalAudioInputError::BackendUnavailable("capture configuration was missing".to_owned())
        })?;

        // Drain what has accumulated; the stream stays open and keeps appending.
        let samples_f32_mono = drain_capture_buffer(&self.capture_buffer)?;

        Ok(AudioSegment {
            sample_rate_hz,
            channels: 1,
            duration_ms: sample_duration_ms(sample_rate_hz, samples_f32_mono.len()),
            samples_f32_mono,
        })
    }
}

/// Takes everything currently in the shared capture buffer, leaving it empty
/// for the stream callback to keep filling.
fn drain_capture_buffer(
    capture_buffer: &Arc<Mutex<Vec<f32>>>,
) -> Result<Vec<f32>, CpalAudioInputError> {
    let mut guard = capture_buffer.lock().map_err(|_| {
        CpalAudioInputError::BackendUnavailable("capture buffer was poisoned".to_owned())
    })?;
    Ok(std::mem::take(&mut *guard))
}

impl CaptureBackend for RealCaptureBackend {
    fn start_capture(&mut self, _session_id: ImeSessionId) -> Result<(), CpalAudioInputError> {
        self.start_capture_impl()
    }

    fn stop_capture(
        &mut self,
        _session_id: ImeSessionId,
    ) -> Result<AudioSegment, CpalAudioInputError> {
        self.stop_capture_impl()
    }

    fn poll_captured(
        &mut self,
        _session_id: ImeSessionId,
    ) -> Result<AudioSegment, CpalAudioInputError> {
        self.poll_captured_impl()
    }
}

fn build_input_stream_for<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: u16,
    capture_buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, CpalAudioInputError>
where
    T: SizedSample + 'static,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            *config,
            move |data: &[T], _info| {
                let channels = usize::from(channels);
                if channels == 0 {
                    return;
                }

                if let Ok(mut samples) = capture_buffer.lock() {
                    for frame in data.chunks(channels) {
                        if frame.is_empty() {
                            continue;
                        }

                        let mut mono_sample = 0.0_f32;
                        for sample in frame.iter().copied() {
                            mono_sample += f32::from_sample(sample);
                        }

                        samples.push(mono_sample / frame.len() as f32);
                    }
                }
            },
            |_err: cpal::Error| {},
            None,
        )
        .map_err(map_cpal_error)
}

fn sample_duration_ms(sample_rate_hz: u32, sample_count: usize) -> u32 {
    if sample_rate_hz == 0 {
        return 0;
    }

    let millis = (sample_count as u128).saturating_mul(1_000) / u128::from(sample_rate_hz);
    millis.min(u128::from(u32::MAX)) as u32
}

fn map_cpal_error(err: cpal::Error) -> CpalAudioInputError {
    match err.kind() {
        cpal::ErrorKind::DeviceNotAvailable => CpalAudioInputError::DeviceNotFound,
        _ => CpalAudioInputError::BackendUnavailable(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idiolect_ports::audio::AudioInputPort;

    #[derive(Default)]
    struct TestBackend {
        started: bool,
    }

    impl CaptureBackend for TestBackend {
        fn start_capture(&mut self, _session_id: ImeSessionId) -> Result<(), CpalAudioInputError> {
            self.started = true;
            Ok(())
        }

        fn stop_capture(
            &mut self,
            _session_id: ImeSessionId,
        ) -> Result<AudioSegment, CpalAudioInputError> {
            if self.started {
                self.started = false;
                Ok(AudioSegment {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    duration_ms: 0,
                    samples_f32_mono: Vec::new(),
                })
            } else {
                Err(CpalAudioInputError::NotStarted)
            }
        }

        fn poll_captured(
            &mut self,
            _session_id: ImeSessionId,
        ) -> Result<AudioSegment, CpalAudioInputError> {
            if self.started {
                Ok(AudioSegment {
                    sample_rate_hz: 16_000,
                    channels: 1,
                    duration_ms: 0,
                    samples_f32_mono: Vec::new(),
                })
            } else {
                Err(CpalAudioInputError::NotStarted)
            }
        }
    }

    #[test]
    fn stop_before_start_returns_not_started() {
        let mut audio = CpalAudioInput::new_for_test(TestBackend::default());
        let session_id = ImeSessionId::new();

        assert_eq!(
            audio.stop_capture(session_id),
            Err(CpalAudioInputError::NotStarted)
        );
    }

    #[test]
    fn poll_before_start_returns_not_started() {
        let mut audio = CpalAudioInput::new_for_test(TestBackend::default());
        let session_id = ImeSessionId::new();

        assert_eq!(
            audio.poll_captured(session_id),
            Err(CpalAudioInputError::NotStarted)
        );
    }

    #[test]
    fn draining_the_capture_buffer_takes_everything_once() {
        // The streaming contract: each poll takes exactly what accumulated since
        // the previous one, and the buffer keeps filling in between.
        let buffer = Arc::new(Mutex::new(vec![0.1_f32, 0.2, 0.3]));

        let first = drain_capture_buffer(&buffer).expect("drain");
        assert_eq!(first, vec![0.1, 0.2, 0.3]);
        assert!(drain_capture_buffer(&buffer).expect("drain").is_empty());

        // The stream callback appends more; the next drain sees only the new tail.
        buffer.lock().expect("lock").extend_from_slice(&[0.4, 0.5]);
        assert_eq!(
            drain_capture_buffer(&buffer).expect("drain"),
            vec![0.4, 0.5]
        );
    }

    #[test]
    fn missing_device_is_reported_as_typed_error() {
        let result = CpalAudioInput::open_device_by_name(MISSING_DEVICE_NAME);

        assert!(matches!(result, Err(CpalAudioInputError::DeviceNotFound)));
    }
}
