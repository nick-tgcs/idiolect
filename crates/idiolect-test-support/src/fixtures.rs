use std::f32::consts::PI;

use idiolect_ports::audio::AudioSegment;

/// Creates a deterministic 16 kHz, one-second, mono sine-wave fixture.
pub fn sine_fixture_16khz_mono() -> AudioSegment {
    let sample_rate_hz = 16_000u32;
    let duration_ms = 1_000u32;
    let channels = 1u16;
    let frequency_hz = 440.0f32;

    let samples_f32_mono = (0..sample_rate_hz)
        .map(|sample_index| {
            let phase = 2.0 * PI * frequency_hz * (sample_index as f32) / (sample_rate_hz as f32);
            phase.sin()
        })
        .collect::<Vec<f32>>();

    AudioSegment {
        sample_rate_hz,
        channels,
        duration_ms,
        samples_f32_mono,
    }
}

#[cfg(test)]
mod tests {
    use super::sine_fixture_16khz_mono;

    #[test]
    fn sine_fixture_has_expected_shape() {
        let segment = sine_fixture_16khz_mono();

        assert_eq!(segment.sample_rate_hz, 16_000);
        assert_eq!(segment.channels, 1);
        assert_eq!(segment.duration_ms, 1_000);
        assert_eq!(segment.sample_count(), 16_000);
        assert_eq!(segment.samples_f32_mono[0], 0.0);
    }

    #[test]
    fn sine_fixture_is_deterministic() {
        let first = sine_fixture_16khz_mono();
        let second = sine_fixture_16khz_mono();

        assert_eq!(first.samples_f32_mono, second.samples_f32_mono);
    }
}
