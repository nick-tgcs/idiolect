//! Log-mel spectrogram, ported op-for-op from whisper.cpp's
//! `log_mel_spectrogram` so the Burn forward pass sees the same features the
//! serving engine computes: periodic Hann 400, hop 160, reflect-pad 200 at the
//! start, 30 s of zeros (+200) at the end, power spectrum bins 0..=200 (no
//! symmetric-half summation), the model's own mel filters, `log10(max(x,1e-10))`,
//! then the global `max(v, max-8); (v+4)/4` normalization.

use crate::ggml::GgmlMelFilters;

pub const SAMPLE_RATE: usize = 16_000;
pub const N_FFT: usize = 400;
pub const HOP_LENGTH: usize = 160;
/// Frames the encoder consumes per 30 s window (2 × n_audio_ctx).
pub const N_FRAMES: usize = 3_000;

/// Computes the first `N_FRAMES` log-mel frames for one 30 s window starting
/// at the beginning of `samples`. Returns `n_mel × N_FRAMES` values, frame
/// index fastest (i.e. `out[mel * N_FRAMES + frame]`), matching the layout the
/// encoder input expects.
#[must_use]
pub fn log_mel_spectrogram(samples: &[f32], filters: &GgmlMelFilters) -> Vec<f32> {
    let n_mel = filters.n_mel as usize;
    let n_fft_bins = filters.n_fft as usize; // 201 = N_FFT/2 + 1

    // whisper.cpp padding: 200 reflected samples in front, then the audio,
    // then 30 s of zeros plus 200 more zeros.
    let mut padded = Vec::with_capacity(samples.len() + N_FFT / 2 + SAMPLE_RATE * 30 + N_FFT / 2);
    for i in (1..=N_FFT / 2).rev() {
        padded.push(*samples.get(i).unwrap_or(&0.0));
    }
    padded.extend_from_slice(samples);
    padded.resize(padded.len() + SAMPLE_RATE * 30 + N_FFT / 2, 0.0);

    // The worker sees n_samples + the front reflect pad as its effective
    // length; frames past it are forced to log10(1e-10) = -10.
    let n_samples_effective = samples.len() + N_FFT / 2;
    let silent_from = n_samples_effective / HOP_LENGTH + 1;

    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos()))
        .collect();

    let mut mel = vec![0.0f32; n_mel * N_FRAMES];
    let mut frame = vec![0.0f32; N_FFT];
    for frame_index in 0..N_FRAMES {
        if frame_index >= silent_from {
            for mel_index in 0..n_mel {
                mel[mel_index * N_FRAMES + frame_index] = -10.0;
            }
            continue;
        }
        let offset = frame_index * HOP_LENGTH;
        for i in 0..N_FFT {
            frame[i] = padded.get(offset + i).copied().unwrap_or(0.0) * window[i];
        }
        let spectrum = fft(&frame);
        for mel_index in 0..n_mel {
            let weights = &filters.data[mel_index * n_fft_bins..(mel_index + 1) * n_fft_bins];
            let mut sum = 0.0f64;
            for (&(re, im), &weight) in spectrum.iter().zip(weights) {
                let power = f64::from(re) * f64::from(re) + f64::from(im) * f64::from(im);
                sum += power * f64::from(weight);
            }
            mel[mel_index * N_FRAMES + frame_index] = (sum.max(1e-10)).log10() as f32;
        }
    }

    let max = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let floor = max - 8.0;
    for value in &mut mel {
        *value = (value.max(floor) + 4.0) / 4.0;
    }
    mel
}

/// Real-input DFT returning bins 0..N_FFT-1 as (re, im) — same recursive
/// even/odd split whisper.cpp uses (radix-2 with a naive DFT at odd sizes).
fn fft(input: &[f32]) -> Vec<(f32, f32)> {
    let n = input.len();
    if n == 1 {
        return vec![(input[0], 0.0)];
    }
    if n % 2 == 1 {
        return naive_dft(input);
    }
    let even: Vec<f32> = input.iter().step_by(2).copied().collect();
    let odd: Vec<f32> = input.iter().skip(1).step_by(2).copied().collect();
    let even_fft = fft(&even);
    let odd_fft = fft(&odd);
    let mut out = vec![(0.0f32, 0.0f32); n];
    for k in 0..n / 2 {
        let theta = -2.0 * std::f32::consts::PI * k as f32 / n as f32;
        let (cos, sin) = (theta.cos(), theta.sin());
        let (ore, oim) = odd_fft[k];
        let twiddled = (cos * ore - sin * oim, cos * oim + sin * ore);
        let (ere, eim) = even_fft[k];
        out[k] = (ere + twiddled.0, eim + twiddled.1);
        out[k + n / 2] = (ere - twiddled.0, eim - twiddled.1);
    }
    out
}

fn naive_dft(input: &[f32]) -> Vec<(f32, f32)> {
    let n = input.len();
    let mut out = vec![(0.0f32, 0.0f32); n];
    for (k, slot) in out.iter_mut().enumerate() {
        let mut re = 0.0f32;
        let mut im = 0.0f32;
        for (i, &sample) in input.iter().enumerate() {
            let theta = -2.0 * std::f32::consts::PI * (k * i) as f32 / n as f32;
            re += sample * theta.cos();
            im += sample * theta.sin();
        }
        *slot = (re, im);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{fft, log_mel_spectrogram, N_FRAMES};
    use crate::ggml::GgmlMelFilters;

    #[test]
    fn the_fft_matches_a_direct_dft() {
        let signal: Vec<f32> = (0..400)
            .map(|i| ((i * 7) % 13) as f32 / 13.0 - 0.5)
            .collect();
        let fast = fft(&signal);
        let slow = super::naive_dft(&signal);
        for (bin, (f, s)) in fast.iter().zip(slow.iter()).enumerate() {
            assert!(
                (f.0 - s.0).abs() < 1e-2 && (f.1 - s.1).abs() < 1e-2,
                "bin {bin}: fft {f:?} vs dft {s:?}"
            );
        }
    }

    #[test]
    fn silence_normalizes_to_a_constant_spectrogram() {
        let filters = GgmlMelFilters {
            n_mel: 2,
            n_fft: 201,
            data: vec![0.5; 2 * 201],
        };
        let mel = log_mel_spectrogram(&vec![0.0f32; 16_000], &filters);
        assert_eq!(mel.len(), 2 * N_FRAMES);
        // All-silence: every value hits the same log floor, so after the
        // (v - (max-8) … +4)/4 normalization everything is (max+4)/4 with
        // max = -10 → -1.5.
        assert!(
            mel.iter().all(|v| (*v - -1.5).abs() < 1e-6),
            "{:?}",
            &mel[..4]
        );
    }
}
