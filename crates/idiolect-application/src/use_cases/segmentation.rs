//! Pause-triggered utterance segmentation.
//!
//! A pure state machine that turns a live stream of fixed-size audio frames —
//! each pre-labelled speech/non-speech by a VAD — into utterance "snippets":
//! when the speaker pauses for `post_roll_ms`, everything they said since the
//! last pause (plus a `pre_roll_ms` lead-in so the first phoneme is never
//! clipped) is emitted as one snippet. This is what lets the daemon transcribe
//! and translate *as the user pauses* instead of only when recording stops.
//!
//! No I/O and no VAD dependency: callers supply the per-frame speech verdict,
//! which keeps every timing rule unit-testable.

use std::collections::VecDeque;

/// Timing rules for the segmenter, all in milliseconds of audio (not wall
/// clock). Mirrors the `[vad]` config section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmenterConfig {
    pub sample_rate_hz: u32,
    /// Fixed frame size pushed by the caller. Must match the VAD frame size.
    pub frame_ms: u32,
    /// Speech bursts shorter than this are discarded as noise blips.
    pub min_speech_ms: u32,
    /// Audio kept from just before speech onset so the first phoneme survives.
    pub pre_roll_ms: u32,
    /// The pause threshold: this much contiguous silence ends the utterance.
    pub post_roll_ms: u32,
    /// A snippet is force-emitted at this length even if the speaker never
    /// pauses, so one long monologue cannot defer feedback forever.
    pub max_utterance_ms: u32,
}

impl SegmenterConfig {
    /// Samples per frame at the configured rate.
    #[must_use]
    pub fn frame_samples(&self) -> usize {
        (self.sample_rate_hz as usize * self.frame_ms as usize) / 1_000
    }

    fn frames_for(&self, duration_ms: u32) -> usize {
        (duration_ms as usize).div_ceil(self.frame_ms.max(1) as usize)
    }
}

/// An emitted utterance: the samples from pre-roll through the pause.
#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub samples_f32_mono: Vec<f32>,
}

#[derive(Debug, PartialEq, Eq)]
enum SegmenterState {
    Idle,
    Active,
}

/// See the module docs. Feed fixed-size frames via [`Self::push_frame`]; call
/// [`Self::flush`] when recording stops to recover the un-paused tail.
#[derive(Debug)]
pub struct UtteranceSegmenter {
    config: SegmenterConfig,
    state: SegmenterState,
    /// Ring of recent silence frames kept while idle, prepended on speech onset.
    pre_roll: VecDeque<Vec<f32>>,
    /// Samples of the in-progress utterance (pre-roll included).
    active: Vec<f32>,
    speech_frames: usize,
    trailing_silence_frames: usize,
    active_frames: usize,
}

impl UtteranceSegmenter {
    #[must_use]
    pub fn new(config: SegmenterConfig) -> Self {
        Self {
            config,
            state: SegmenterState::Idle,
            pre_roll: VecDeque::new(),
            active: Vec::new(),
            speech_frames: 0,
            trailing_silence_frames: 0,
            active_frames: 0,
        }
    }

    /// Whether an utterance is currently in progress.
    #[must_use]
    pub fn is_speaking(&self) -> bool {
        self.state == SegmenterState::Active
    }

    /// Pushes one frame (exactly [`SegmenterConfig::frame_samples`] samples)
    /// with its VAD verdict. Returns a snippet when this frame completes one:
    /// either the pause threshold was just crossed, or the utterance hit
    /// `max_utterance_ms`.
    pub fn push_frame(&mut self, samples: &[f32], is_speech: bool) -> Option<Snippet> {
        debug_assert_eq!(samples.len(), self.config.frame_samples());

        match self.state {
            SegmenterState::Idle => {
                if is_speech {
                    self.begin_utterance(samples);
                    None
                } else {
                    self.remember_pre_roll(samples);
                    None
                }
            }
            SegmenterState::Active => {
                self.active.extend_from_slice(samples);
                self.active_frames += 1;
                if is_speech {
                    self.speech_frames += 1;
                    self.trailing_silence_frames = 0;
                } else {
                    self.trailing_silence_frames += 1;
                }

                let paused = self.trailing_silence_frames
                    >= self.config.frames_for(self.config.post_roll_ms);
                let over_long =
                    self.active_frames >= self.config.frames_for(self.config.max_utterance_ms);
                if paused || over_long {
                    self.finish_utterance()
                } else {
                    None
                }
            }
        }
    }

    /// Ends the stream (recording stopped): emits the in-progress utterance if
    /// it contains enough speech, and resets.
    pub fn flush(&mut self) -> Option<Snippet> {
        if self.state != SegmenterState::Active {
            return None;
        }
        self.finish_utterance()
    }

    fn begin_utterance(&mut self, samples: &[f32]) {
        self.active.clear();
        for frame in &self.pre_roll {
            self.active.extend_from_slice(frame);
        }
        self.pre_roll.clear();
        self.active.extend_from_slice(samples);
        self.active_frames = 1;
        self.speech_frames = 1;
        self.trailing_silence_frames = 0;
        self.state = SegmenterState::Active;
    }

    fn remember_pre_roll(&mut self, samples: &[f32]) {
        let cap = self.config.frames_for(self.config.pre_roll_ms);
        if cap == 0 {
            return;
        }
        if self.pre_roll.len() == cap {
            self.pre_roll.pop_front();
        }
        self.pre_roll.push_back(samples.to_vec());
    }

    fn finish_utterance(&mut self) -> Option<Snippet> {
        let speech_ms = self.speech_frames as u32 * self.config.frame_ms;
        let samples = std::mem::take(&mut self.active);
        self.state = SegmenterState::Idle;
        self.speech_frames = 0;
        self.trailing_silence_frames = 0;
        self.active_frames = 0;

        if speech_ms >= self.config.min_speech_ms {
            Some(Snippet {
                samples_f32_mono: samples,
            })
        } else {
            // A blip too short to be speech: drop it rather than emit noise.
            None
        }
    }
}

/// Re-chunks arbitrarily-sized capture drains into the segmenter's fixed frame
/// size, carrying the remainder between pushes.
#[derive(Debug, Default)]
pub struct FrameBuffer {
    pending: Vec<f32>,
}

impl FrameBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends drained samples and returns every now-complete frame.
    pub fn push(&mut self, samples: &[f32], frame_samples: usize) -> Vec<Vec<f32>> {
        if frame_samples == 0 {
            return Vec::new();
        }
        self.pending.extend_from_slice(samples);
        let complete = self.pending.len() / frame_samples;
        let mut frames = Vec::with_capacity(complete);
        for index in 0..complete {
            frames.push(self.pending[index * frame_samples..(index + 1) * frame_samples].to_vec());
        }
        self.pending.drain(..complete * frame_samples);
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameBuffer, SegmenterConfig, UtteranceSegmenter};

    /// Tiny frames keep the arithmetic legible: 10 samples per 10 ms frame,
    /// pause after 3 silent frames, blips under 2 frames discarded, pre-roll of
    /// 2 frames, force-flush at 100 frames.
    fn config() -> SegmenterConfig {
        SegmenterConfig {
            sample_rate_hz: 1_000,
            frame_ms: 10,
            min_speech_ms: 20,
            pre_roll_ms: 20,
            post_roll_ms: 30,
            max_utterance_ms: 1_000,
        }
    }

    fn frame(value: f32) -> Vec<f32> {
        vec![value; 10]
    }

    /// Pushes `count` frames of `value`, asserting no snippet is emitted before
    /// the last push; returns the last push's result.
    fn push_run(
        segmenter: &mut UtteranceSegmenter,
        value: f32,
        is_speech: bool,
        count: usize,
    ) -> Option<super::Snippet> {
        let mut last = None;
        for index in 0..count {
            let emitted = segmenter.push_frame(&frame(value), is_speech);
            if index + 1 < count {
                assert!(
                    emitted.is_none(),
                    "snippet emitted mid-run at frame {index}"
                );
            }
            last = emitted;
        }
        last
    }

    #[test]
    fn pause_emits_the_snippet_with_pre_roll() {
        let mut segmenter = UtteranceSegmenter::new(config());

        // Lead-in silence beyond the pre-roll window: only the last 2 frames
        // (the pre-roll cap) may survive into the snippet.
        push_run(&mut segmenter, 0.1, false, 5);
        // 4 frames of speech, then exactly the pause threshold of silence.
        push_run(&mut segmenter, 0.9, true, 4);
        assert!(segmenter.is_speaking());
        let snippet = push_run(&mut segmenter, 0.0, false, 3).expect("pause must emit a snippet");

        // pre-roll (2) + speech (4) + trailing silence (3) = 9 frames.
        assert_eq!(snippet.samples_f32_mono.len(), 9 * 10);
        assert!(
            snippet.samples_f32_mono.starts_with(&[0.1; 20]),
            "pre-roll retained"
        );
        assert_eq!(
            &snippet.samples_f32_mono[20..60],
            &[0.9; 40],
            "speech retained"
        );
        assert!(!segmenter.is_speaking(), "back to idle after the pause");
    }

    #[test]
    fn short_blip_is_discarded_as_noise() {
        let mut segmenter = UtteranceSegmenter::new(config());

        // One 10 ms speech frame is under min_speech_ms (20 ms).
        segmenter.push_frame(&frame(0.9), true);
        let emitted = push_run(&mut segmenter, 0.0, false, 3);

        assert!(emitted.is_none(), "a blip must not become a snippet");
        assert!(!segmenter.is_speaking());
    }

    #[test]
    fn a_short_pause_does_not_split_the_utterance() {
        let mut segmenter = UtteranceSegmenter::new(config());

        push_run(&mut segmenter, 0.9, true, 3);
        // 2 silent frames: under the 3-frame pause threshold.
        push_run(&mut segmenter, 0.0, false, 2);
        push_run(&mut segmenter, 0.8, true, 3);
        let snippet =
            push_run(&mut segmenter, 0.0, false, 3).expect("the real pause emits one snippet");

        // Both bursts plus the mid-gap and the final pause are one utterance:
        // 3 + 2 + 3 + 3 = 11 frames.
        assert_eq!(snippet.samples_f32_mono.len(), 11 * 10);
    }

    #[test]
    fn max_utterance_forces_a_snippet_mid_speech() {
        let mut segmenter = UtteranceSegmenter::new(SegmenterConfig {
            max_utterance_ms: 50,
            ..config()
        });

        // Continuous speech with no pause: the cap (5 frames) must emit anyway.
        let snippet = push_run(&mut segmenter, 0.9, true, 5).expect("cap must force a snippet");

        assert_eq!(snippet.samples_f32_mono.len(), 5 * 10);
        assert!(!segmenter.is_speaking());
        // Speech continuing after the forced flush starts a fresh utterance.
        segmenter.push_frame(&frame(0.9), true);
        assert!(segmenter.is_speaking());
    }

    #[test]
    fn flush_recovers_the_tail_on_stop() {
        let mut segmenter = UtteranceSegmenter::new(config());

        push_run(&mut segmenter, 0.9, true, 4);
        let snippet = segmenter.flush().expect("stop must flush the tail");

        assert_eq!(snippet.samples_f32_mono.len(), 4 * 10);
        assert!(segmenter.flush().is_none(), "flush is idempotent");
    }

    #[test]
    fn flush_discards_a_tail_blip() {
        let mut segmenter = UtteranceSegmenter::new(config());
        segmenter.push_frame(&frame(0.9), true);
        assert!(segmenter.flush().is_none(), "sub-min tail is noise");
    }

    #[test]
    fn consecutive_utterances_emit_separate_snippets() {
        let mut segmenter = UtteranceSegmenter::new(config());

        push_run(&mut segmenter, 0.9, true, 3);
        let first = push_run(&mut segmenter, 0.0, false, 3).expect("first snippet");
        push_run(&mut segmenter, 0.7, true, 3);
        let second = push_run(&mut segmenter, 0.0, false, 3).expect("second snippet");

        assert_eq!(first.samples_f32_mono.len(), 6 * 10);
        // No pre-roll accumulated between utterances (silence went into the
        // first snippet's tail, not the second's head).
        assert_eq!(second.samples_f32_mono.len(), 6 * 10);
        assert_eq!(&second.samples_f32_mono[..30], &[0.7; 30]);
    }

    #[test]
    fn frame_buffer_rechunks_with_remainder_carry() {
        let mut buffer = FrameBuffer::new();

        assert!(
            buffer.push(&[1.0; 7], 10).is_empty(),
            "incomplete frame waits"
        );
        let frames = buffer.push(&[2.0; 18], 10);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0][..7], [1.0; 7][..]);
        assert_eq!(frames[0][7..], [2.0; 3][..]);
        assert_eq!(frames[1], vec![2.0; 10]);
        // 5 samples remain pending.
        let frames = buffer.push(&[3.0; 5], 10);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0][..5], [2.0; 5][..]);
        assert_eq!(frames[0][5..], [3.0; 5][..]);
    }
}
