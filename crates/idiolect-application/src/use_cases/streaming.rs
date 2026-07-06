//! Shared streaming-take orchestration — the one place a live dictation take is
//! turned, pause by pause, into the single string and single recording it stores.
//!
//! Lifted out of the desktop `run_loop` (M2) so the **same** rules govern the
//! Android path. Two layers live here:
//!
//! * The **pure decisions** ([`is_noise_transcript`], [`snippet_chunk`],
//!   [`choose_final_take_text`], [`merge_tail_correction`]): snippet decodes are
//!   short-context previews that drop words at pause boundaries, so the
//!   whole-recording decode at stop is authoritative — but only when it heard
//!   usable words, else the previewed text wins (a real take lost "I don't want"
//!   to a snippet-only path; this is that guardrail).
//! * The **state machine** ([`StreamingTake`]): it owns the segmenter, the
//!   take-level accumulators, the auto-stop clock, and the once-per-take error
//!   de-duplication. The two edge-specific steps are **injected** — the
//!   decode/translate via [`TakeTranscriber`] (the daemon binds
//!   `transcribe_translated`; Android binds whisper on-device) and the live
//!   feedback via [`StreamObserver`] (the daemon pushes IPC; Android calls the
//!   Kotlin callback). The capture-rate resampling and the per-frame VAD stay at
//!   the edge and feed [`StreamingTake::ingest`] already-16 kHz-mono audio plus a
//!   speech verdict, so this module needs no adapter dependency.

/// Whether a transcription is only non-speech markers. Whisper labels noise with
/// bracketed/parenthesised annotations — `[BLANK_AUDIO]`, `(knocking)`,
/// `[silence]` — and a snippet that contains nothing else (a knock, a cough, a
/// breath that fooled the VAD) must be dropped, not typed into the document.
#[must_use]
pub fn is_noise_transcript(text: &str) -> bool {
    let mut depth = 0_u32;
    for character in text.chars() {
        match character {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if character.is_alphanumeric() => return false,
            _ => {}
        }
    }
    true
}

/// The text a snippet contributes to the take: trimmed (Whisper pads snippet
/// decodes with whitespace, which would double the join space — a real take
/// stored "…  …" this way), carrying its joining space when the take already has
/// text. `None` when the trimmed decode is empty or noise-only markers — nothing
/// worth typing.
#[must_use]
pub fn snippet_chunk(take_so_far: &str, decoded: &str) -> Option<String> {
    let text = decoded.trim();
    if text.is_empty() || is_noise_transcript(text) {
        return None;
    }
    Some(if take_so_far.is_empty() {
        text.to_owned()
    } else {
        format!(" {text}")
    })
}

/// Picks the take's final text at stop: the decode of the WHOLE recording when it
/// produced usable words (short-context snippet decodes drop words at pause
/// boundaries — a real take lost "I don't want" that way), otherwise the glued
/// snippet previews — never lose text the user already saw typed.
#[must_use]
pub fn choose_final_take_text(full_decode: String, previewed: String) -> String {
    let text = full_decode.trim();
    if text.is_empty() || is_noise_transcript(text) {
        previewed
    } else {
        text.to_owned()
    }
}

/// Applies an engine-reported in-place correction to a take's text. The engine's
/// correction window only ever tracks the take's final snippet (`tail`), so when
/// the take's text ends with that snippet, the correction replaces just that
/// suffix. Batch takes (`tail` = None) — where the window held the whole
/// transcript — replace the full text. A tail that no longer suffixes the text
/// (the stored text is the stop-time whole-recording decode, which can differ
/// from the last snippet preview) keeps the take unchanged: one snippet's
/// correction must never overwrite the whole take.
#[must_use]
pub fn merge_tail_correction(current: &str, tail: Option<&str>, corrected: &str) -> String {
    match tail {
        Some(tail) if current.ends_with(tail) => {
            format!("{}{corrected}", &current[..current.len() - tail.len()])
        }
        Some(_) => current.to_owned(),
        None => corrected.to_owned(),
    }
}

use crate::use_cases::segmentation::{FrameBuffer, SegmenterConfig, UtteranceSegmenter};

/// The fixed geometry the live pipeline runs at: 16 kHz mono, 30 ms frames
/// (480 samples). This is the frame size the VAD verdict is computed over, so a
/// caller's per-frame VAD must use the same size; the segmenter and the auto-stop
/// clock derive from it.
const STREAM_SAMPLE_RATE_HZ: u32 = 16_000;
const STREAM_FRAME_MS: u32 = 30;
const STREAM_FRAME_SAMPLES: usize =
    (STREAM_SAMPLE_RATE_HZ as usize * STREAM_FRAME_MS as usize) / 1_000;

/// The window the authoritative stop-time decode is chunked into. Whisper's
/// acoustic context is a fixed 30 s window; handing it one long, silence-stripped
/// block collapses the decode (a 50 s take decoded to 4 words in testing), so the
/// finalize re-decode runs one ≤30 s chunk at a time and stitches the chunks back
/// together. Aligned to snippet (pause) boundaries where possible so a chunk edge
/// never falls mid-word.
const FINAL_CHUNK_SAMPLES: usize = STREAM_SAMPLE_RATE_HZ as usize * 30;

/// The tunable timing rules for a streaming take (the `[vad]` config knobs). The
/// frame geometry is fixed (see the module consts); only these vary by user
/// preference and tray override.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamingConfig {
    /// Speech bursts shorter than this are discarded as noise blips.
    pub min_speech_ms: u32,
    /// Audio kept from just before speech onset so the first phoneme survives.
    pub pre_roll_ms: u32,
    /// The pause threshold: this much contiguous silence ends an utterance.
    pub post_roll_ms: u32,
    /// A snippet is force-emitted at this length even without a pause.
    pub max_utterance_ms: u32,
    /// Silence (after the take's first speech) that ends the whole take by
    /// itself. `0` disables silence auto-stop.
    pub auto_stop_silence_ms: u32,
}

/// The decode/translate step the live pipeline injects: 16 kHz mono samples in,
/// text out. The daemon binds `transcribe_translated`; Android binds whisper
/// on-device. The error carries a stable `code` so the orchestration can
/// de-duplicate the user-facing notification to once per take per cause.
pub trait TakeTranscriber {
    fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure>;
}

/// Lets a boxed transcriber be used wherever a `TakeTranscriber` is expected. The
/// mobile facade holds the decoder behind a `Box<dyn TakeTranscriber + Send>` (the
/// concrete engine is swapped in once a model is loaded), so the take's
/// generic [`StreamingTake::fold_snippet`]/[`StreamingTake::finalize`] must accept
/// the box directly.
impl<T: TakeTranscriber + ?Sized> TakeTranscriber for Box<T> {
    fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        (**self).transcribe(samples_f32_mono)
    }
}

/// A failed decode: a stable `code` (for once-per-take de-duplication) and a
/// human-readable `message` (for the journal / notification body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscribeFailure {
    pub code: String,
    pub message: String,
}

/// Where a live take's events go. The edge turns them into IPC pushes / Kotlin
/// callbacks / desktop notifications. Fallible so an edge whose emit can fail (a
/// socket write to a vanished client) propagates that, exactly as the daemon does.
pub trait StreamObserver {
    type Error;

    /// A pause-completed snippet decoded to usable text and was folded into the
    /// take. `chunk` carries its joining space; push it as a PARTIAL preedit.
    fn snippet_committed(&mut self, chunk: &str) -> Result<(), Self::Error>;

    /// A snippet decoded to nothing, or to noise-only markers: its audio is kept
    /// for the stop-time decode, but it contributes no text. (Diagnostic only.)
    fn snippet_dropped(&mut self, decoded: &str) -> Result<(), Self::Error>;

    /// A snippet failed to decode — surfaced at most once per take per `code`.
    fn transcribe_failed(&mut self, code: &str, message: &str) -> Result<(), Self::Error>;
}

/// One folded snippet held for the stop-time re-decode: its 16 kHz mono audio and
/// the trimmed preview text it contributed (empty when the snippet decoded to
/// nothing or noise — its audio is still kept so a chunk re-decode can recover
/// words the short-context preview missed).
struct TakeSnippet {
    samples: Vec<f32>,
    preview: String,
}

/// One live dictation take: the pause-triggered pipeline's accumulating state.
/// Capture audio (resampled to 16 kHz mono at the edge) is fed via
/// [`Self::ingest`]; each pause-completed snippet is decoded and folded via
/// [`Self::fold_snippet`]; the whole take is re-decoded in ≤30 s chunks and closed
/// out via [`Self::finalize`]. The audio→snippet plumbing (resampler, VAD) stays at the
/// edge; this owns the segmenter, the accumulators, the auto-stop clock, and the
/// once-per-take error de-duplication.
pub struct StreamingTake {
    frames: FrameBuffer,
    segmenter: UtteranceSegmenter,
    /// Every folded snippet's audio + preview, in fold order. The take's stored
    /// recording is these snippets' audio concatenated; the stop-time re-decode
    /// chunks them into ≤30 s windows so a long take never collapses in one pass.
    snippets: Vec<TakeSnippet>,
    /// Every snippet's decoded text, space-joined: the take's previewed string.
    merged_text: String,
    /// The most recent snippet's text (no joining space) — the suffix a post-take
    /// in-place correction replaces.
    last_snippet_text: Option<String>,
    /// Whether any speech frame has been heard this take. Auto-stop only arms
    /// after the first speech: pre-take thinking time never ends the take.
    spoke: bool,
    /// Consecutive silence frames since the last speech frame (audio time).
    silence_frames_since_speech: usize,
    /// The take's auto-stop threshold (0 = never).
    auto_stop_silence_ms: u32,
    /// Error code already surfaced to the user this take — failures repeat per
    /// pause, the notification must not.
    notified_error: Option<String>,
}

impl StreamingTake {
    #[must_use]
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            frames: FrameBuffer::new(),
            segmenter: UtteranceSegmenter::new(SegmenterConfig {
                sample_rate_hz: STREAM_SAMPLE_RATE_HZ,
                frame_ms: STREAM_FRAME_MS,
                min_speech_ms: config.min_speech_ms,
                pre_roll_ms: config.pre_roll_ms,
                post_roll_ms: config.post_roll_ms,
                max_utterance_ms: config.max_utterance_ms,
            }),
            snippets: Vec::new(),
            merged_text: String::new(),
            last_snippet_text: None,
            spoke: false,
            silence_frames_since_speech: 0,
            auto_stop_silence_ms: config.auto_stop_silence_ms,
            notified_error: None,
        }
    }

    /// Re-chunks already-16 kHz-mono `samples` into VAD frames, labels each via
    /// `is_speech` (the caller's VAD), advances the segmenter and the auto-stop
    /// clock, and returns the samples of every snippet a pause just completed.
    pub fn ingest(
        &mut self,
        samples_f32_mono: &[f32],
        mut is_speech: impl FnMut(&[f32]) -> bool,
    ) -> Vec<Vec<f32>> {
        if samples_f32_mono.is_empty() {
            return Vec::new();
        }
        let mut snippets = Vec::new();
        for frame in self.frames.push(samples_f32_mono, STREAM_FRAME_SAMPLES) {
            // A frame the detector rejects is treated as silence: losing one
            // verdict must never abort the whole take.
            let speech = is_speech(&frame);
            if speech {
                self.spoke = true;
                self.silence_frames_since_speech = 0;
            } else {
                self.silence_frames_since_speech += 1;
            }
            if let Some(snippet) = self.segmenter.push_frame(&frame, speech) {
                snippets.push(snippet.samples_f32_mono);
            }
        }
        snippets
    }

    /// Recovers the un-paused tail utterance when recording stops.
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        self.segmenter
            .flush()
            .map(|snippet| snippet.samples_f32_mono)
    }

    /// Whether the take has gone silent past `auto_stop_silence_ms` (audio time,
    /// after its first speech). `0` disables auto-stop.
    #[must_use]
    pub fn auto_stop_due(&self) -> bool {
        self.auto_stop_silence_ms != 0
            && self.spoke
            && self.silence_frames_since_speech * STREAM_FRAME_MS as usize
                >= self.auto_stop_silence_ms as usize
    }

    /// Decodes one pause-completed snippet and folds it into the take: audio
    /// ALWAYS onto the merged recording (the stop-time decode of the whole
    /// recording can recover words a short-context snippet decode missed), text
    /// onto the merged string when the decode produced usable words. Emits the
    /// live partial / drop / failure via `observer`; a decode failure is surfaced
    /// at most once per take per code.
    pub fn fold_snippet<T, O>(
        &mut self,
        transcriber: &mut T,
        observer: &mut O,
        snippet: Vec<f32>,
    ) -> Result<(), O::Error>
    where
        T: TakeTranscriber,
        O: StreamObserver,
    {
        let text = match transcriber.transcribe(&snippet) {
            Ok(text) => text,
            Err(failure) => {
                if self.first_error_this_take(&failure.code) {
                    observer.transcribe_failed(&failure.code, &failure.message)?;
                }
                return Ok(());
            }
        };
        // The snippet's audio is recorded even when its decode is dropped below, so
        // the stop-time chunk re-decode can still recover words the short-context
        // preview missed. `merged_text` stays the glued preview string; the snippet's
        // own trimmed word content rides along for the per-chunk preview fallback.
        let Some(chunk) = snippet_chunk(&self.merged_text, &text) else {
            self.snippets.push(TakeSnippet {
                samples: snippet,
                preview: String::new(),
            });
            return observer.snippet_dropped(&text);
        };
        self.merged_text.push_str(&chunk);
        let trimmed = text.trim().to_owned();
        self.last_snippet_text = Some(trimmed.clone());
        self.snippets.push(TakeSnippet {
            samples: snippet,
            preview: trimmed,
        });
        observer.snippet_committed(&chunk)
    }

    /// Closes the take: decodes the WHOLE merged recording once — the
    /// authoritative text — falling back to the glued snippet previews when that
    /// decode fails or hears nothing. An empty take (no audio), or one that
    /// decodes to nothing, is [`TakeOutcome::Silent`].
    #[must_use]
    pub fn finalize<T: TakeTranscriber>(mut self, transcriber: &mut T) -> TakeOutcome {
        // Ending the take is finalizing its one (and only) phrase.
        self.finalize_phrase(transcriber)
    }

    /// Closes the CURRENT phrase and re-opens the take for the next one: the same
    /// authoritative whole-phrase decode as [`Self::finalize`] (whole-recording decode,
    /// fallback to glued previews, [`TakeOutcome::Silent`] when empty/noise), but the
    /// accumulators and per-phrase error de-dupe reset IN PLACE so dictation continues.
    /// Continuous mode calls this at each pause; one-shot dictation calls [`Self::finalize`]
    /// once at stop. The segmenter and frame buffer are left intact (a pause has already
    /// returned the segmenter to idle), so the next phrase picks up cleanly.
    #[must_use]
    pub fn finalize_phrase<T: TakeTranscriber>(&mut self, transcriber: &mut T) -> TakeOutcome {
        let last_snippet_text = self.last_snippet_text.take();
        // Re-arm per-phrase state: each phrase is independent (own error budget, own
        // auto-stop clock) so one phrase's glitch can't mute or auto-stop the next.
        self.notified_error = None;
        self.spoke = false;
        self.silence_frames_since_speech = 0;

        if self.snippets.is_empty() {
            self.merged_text.clear();
            return TakeOutcome::Silent;
        }

        // Concatenate every snippet's audio ONCE into the take's stored recording,
        // moving each snippet out. `drain` keeps the snippet buffer's capacity for the
        // next phrase (continuous mode). The chunks below are ranges into this single
        // buffer, so no chunk re-copies the audio.
        let mut merged_samples: Vec<f32> = Vec::with_capacity(
            self.snippets
                .iter()
                .map(|snippet| snippet.samples.len())
                .sum(),
        );
        let mut spans: Vec<SnippetSpan> = Vec::with_capacity(self.snippets.len());
        for snippet in self.snippets.drain(..) {
            let start = merged_samples.len();
            merged_samples.extend(snippet.samples);
            spans.push(SnippetSpan {
                range: start..merged_samples.len(),
                preview: snippet.preview,
            });
        }
        self.merged_text.clear();

        // Re-decode one ≤30 s chunk at a time so a long take never hands the model an
        // over-window block (which collapses the decode). Each chunk's decode still
        // wins over its preview, but a chunk whose decode fails or hears only noise
        // falls back to that chunk's preview — so anything properly transcribed stays
        // and only first-pass preview text is replaced.
        let chunks = plan_chunks(&spans);
        let mut final_parts: Vec<String> = Vec::with_capacity(chunks.len());
        let mut fallback_reason: Option<String> = None;
        for chunk in &chunks {
            let chunk_text = match transcriber.transcribe(&merged_samples[chunk.range.clone()]) {
                Ok(text) => choose_final_take_text(text, chunk.preview.clone()),
                Err(failure) => {
                    fallback_reason.get_or_insert(failure.message);
                    chunk.preview.clone()
                }
            };
            final_parts.push(chunk_text);
        }

        let final_text = normalize_take_text(&final_parts.join(" "));
        if final_text.trim().is_empty() {
            return TakeOutcome::Silent;
        }
        TakeOutcome::Speech(FinalizedTake {
            final_text,
            merged_samples,
            last_snippet_text,
            fallback_reason,
        })
    }

    /// True exactly once per take for a given error code: the first failed
    /// snippet notifies the user, the rest only log.
    fn first_error_this_take(&mut self, code: &str) -> bool {
        if self.notified_error.as_deref() == Some(code) {
            return false;
        }
        self.notified_error = Some(code.to_owned());
        true
    }
}

/// One folded snippet's place in the take's merged recording: its sample range and
/// the trimmed preview text it contributed (empty for a dropped/noise snippet).
struct SnippetSpan {
    range: core::ops::Range<usize>,
    preview: String,
}

/// A planned ≤30 s re-decode unit: a range into the take's merged recording plus
/// the glued preview of the snippets in it — the fallback when the chunk's decode
/// fails or hears only noise.
struct FinalChunk {
    range: core::ops::Range<usize>,
    preview: String,
}

/// Plans the ≤30 s ([`FINAL_CHUNK_SAMPLES`]) re-decode chunks over the merged
/// recording, grouping consecutive snippet spans on snippet (pause) boundaries so a
/// chunk edge never lands mid-word. A lone span longer than the window (only
/// possible when `max_utterance_ms` is configured above 30 s) is hard-split into
/// ≤30 s pieces, its preview riding the first piece. Chunks are ranges — the audio
/// is never copied out of the single merged buffer.
fn plan_chunks(spans: &[SnippetSpan]) -> Vec<FinalChunk> {
    let mut chunks: Vec<FinalChunk> = Vec::new();
    // The consecutive snippet spans accumulating into the currently-open chunk.
    let mut group: Vec<&SnippetSpan> = Vec::new();
    let mut group_len = 0usize;

    for span in spans {
        let len = span.range.end - span.range.start;
        if len > FINAL_CHUNK_SAMPLES {
            // An over-window snippet: seal what's pending, then hard-split it.
            if let Some(chunk) = seal_group(&group) {
                chunks.push(chunk);
            }
            group.clear();
            group_len = 0;
            let mut piece = span.range.start;
            let mut first = true;
            while piece < span.range.end {
                let stop = (piece + FINAL_CHUNK_SAMPLES).min(span.range.end);
                chunks.push(FinalChunk {
                    range: piece..stop,
                    preview: if first {
                        span.preview.clone()
                    } else {
                        String::new()
                    },
                });
                piece = stop;
                first = false;
            }
            continue;
        }
        if !group.is_empty() && group_len + len > FINAL_CHUNK_SAMPLES {
            if let Some(chunk) = seal_group(&group) {
                chunks.push(chunk);
            }
            group.clear();
            group_len = 0;
        }
        group.push(span);
        group_len += len;
    }
    if let Some(chunk) = seal_group(&group) {
        chunks.push(chunk);
    }
    chunks
}

/// Builds the chunk covering a group of consecutive snippet spans: their one
/// contiguous range and their glued previews. `None` for an empty group.
fn seal_group(group: &[&SnippetSpan]) -> Option<FinalChunk> {
    let first = group.first()?;
    let last = group.last()?;
    Some(FinalChunk {
        range: first.range.start..last.range.end,
        preview: glue_previews(group.iter().map(|span| span.preview.as_str())),
    })
}

/// Joins the non-empty snippet previews of one chunk with single spaces (dropped
/// snippets contribute no words but their audio is still in the chunk).
fn glue_previews<'a>(previews: impl Iterator<Item = &'a str>) -> String {
    previews
        .filter(|preview| !preview.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapses runs of whitespace to single spaces and trims — the same normalisation
/// the single-pass whole-recording decode used, applied to the stitched chunks.
fn normalize_take_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The result of [`StreamingTake::finalize`].
#[derive(Clone, Debug, PartialEq)]
pub enum TakeOutcome {
    /// Nothing decodable (no audio, or only noise): store nothing.
    Silent,
    /// The take produced usable text: persist it as one session + recording.
    Speech(FinalizedTake),
}

/// A finalized take ready to persist: the one merged recording, its single final
/// string, the correction-window tail, and whether the stop-time decode fell back.
#[derive(Clone, Debug, PartialEq)]
pub struct FinalizedTake {
    /// The take's authoritative text (whole-recording decode, or the preview
    /// fallback).
    pub final_text: String,
    /// The one recording to store for the whole take (16 kHz mono).
    pub merged_samples: Vec<f32>,
    /// The last snippet's decoded text — the suffix a post-take in-place
    /// correction replaces (the engine's window only ever held that snippet).
    pub last_snippet_text: Option<String>,
    /// `Some(message)` ⇒ the stop-time whole-recording decode failed and the
    /// glued snippet previews were kept instead; the edge logs the message.
    pub fallback_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        choose_final_take_text, is_noise_transcript, merge_tail_correction, snippet_chunk,
    };

    #[test]
    fn snippet_chunks_are_trimmed_and_joined_with_one_space() {
        // First snippet: no joining space.
        assert_eq!(
            snippet_chunk("", "restart traffic"),
            Some("restart traffic".to_owned())
        );
        // Later snippets carry exactly one joining space — even when the decode
        // arrives whitespace-padded (a real take stored a double space because a
        // padded decode was glued verbatim).
        assert_eq!(
            snippet_chunk("take", " so far "),
            Some(" so far".to_owned())
        );
        // Empty or whitespace-only decodes contribute nothing — no bare space.
        assert_eq!(snippet_chunk("take", ""), None);
        assert_eq!(snippet_chunk("take", "   "), None);
        // Noise-only markers contribute nothing.
        assert_eq!(snippet_chunk("take", "[BLANK_AUDIO]"), None);
    }

    #[test]
    fn the_stop_time_decode_wins_unless_it_heard_nothing() {
        // Usable whole-recording decode: authoritative (and trimmed).
        assert_eq!(
            choose_final_take_text(" the full take ".to_owned(), "the full".to_owned()),
            "the full take"
        );
        // Empty or noise-only decode: keep the previewed snippet text — never lose
        // words the user already saw typed.
        assert_eq!(
            choose_final_take_text(String::new(), "previewed".to_owned()),
            "previewed"
        );
        assert_eq!(
            choose_final_take_text("[BLANK_AUDIO]".to_owned(), "previewed".to_owned()),
            "previewed"
        );
    }

    #[test]
    fn tail_corrections_replace_only_the_final_snippet() {
        // Streamed take: the engine's correction window held only the last
        // snippet, so the fix lands on that suffix of the merged string.
        assert_eq!(
            merge_tail_correction(
                "restart traffic deploy nginx",
                Some("deploy nginx"),
                "deploy Nginx"
            ),
            "restart traffic deploy Nginx"
        );
        // Batch take: the window held the whole transcript.
        assert_eq!(
            merge_tail_correction("restart traffic", None, "restart Traefik"),
            "restart Traefik"
        );
        // A tail that no longer suffixes the text: the take's text may be the
        // stop-time whole-recording decode, which can legitimately differ from the
        // last snippet preview the engine's window held. Replacing the whole take
        // with one snippet's correction would destroy it — keep the take and drop
        // the unplaceable correction instead.
        assert_eq!(
            merge_tail_correction("something else", Some("deploy nginx"), "deploy Nginx"),
            "something else"
        );
    }

    #[test]
    fn noise_only_transcripts_are_recognised() {
        // Whisper labels non-speech with bracketed/parenthesised markers; a
        // snippet that is ONLY such markers (a knock, a breath) must be dropped,
        // not typed into the user's document.
        assert!(is_noise_transcript("[BLANK_AUDIO]"));
        assert!(is_noise_transcript("(knocking)"));
        assert!(is_noise_transcript(" [silence] (keyboard clacking) "));
        assert!(is_noise_transcript(""));
        assert!(is_noise_transcript("  ."));
        // Real words survive, even alongside a marker.
        assert!(!is_noise_transcript("restart traffic"));
        assert!(!is_noise_transcript("(sighs) restart traffic"));
    }
}

#[cfg(test)]
mod take_tests {
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::rc::Rc;

    use super::{
        plan_chunks, SnippetSpan, StreamObserver, StreamingConfig, StreamingTake, TakeOutcome,
        TakeTranscriber, TranscribeFailure, FINAL_CHUNK_SAMPLES, STREAM_FRAME_SAMPLES,
        STREAM_SAMPLE_RATE_HZ,
    };

    /// A decode port that returns scripted results in call order.
    struct ScriptedTranscriber {
        outputs: VecDeque<Result<String, TranscribeFailure>>,
    }

    impl ScriptedTranscriber {
        fn new(outputs: impl IntoIterator<Item = Result<String, TranscribeFailure>>) -> Self {
            Self {
                outputs: outputs.into_iter().collect(),
            }
        }
    }

    impl TakeTranscriber for ScriptedTranscriber {
        fn transcribe(&mut self, _samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
            self.outputs
                .pop_front()
                .expect("transcriber called more times than scripted")
        }
    }

    /// Records every event the take emits, for assertions.
    #[derive(Default)]
    struct RecordingObserver {
        committed: Vec<String>,
        dropped: Vec<String>,
        failures: Vec<(String, String)>,
    }

    impl StreamObserver for RecordingObserver {
        type Error = Infallible;

        fn snippet_committed(&mut self, chunk: &str) -> Result<(), Infallible> {
            self.committed.push(chunk.to_owned());
            Ok(())
        }
        fn snippet_dropped(&mut self, decoded: &str) -> Result<(), Infallible> {
            self.dropped.push(decoded.to_owned());
            Ok(())
        }
        fn transcribe_failed(&mut self, code: &str, message: &str) -> Result<(), Infallible> {
            self.failures.push((code.to_owned(), message.to_owned()));
            Ok(())
        }
    }

    /// An observer whose emit fails, to prove the fallible plumbing propagates.
    struct FailingObserver;

    impl StreamObserver for FailingObserver {
        type Error = &'static str;

        fn snippet_committed(&mut self, _chunk: &str) -> Result<(), &'static str> {
            Err("client gone")
        }
        fn snippet_dropped(&mut self, _decoded: &str) -> Result<(), &'static str> {
            Ok(())
        }
        fn transcribe_failed(&mut self, _code: &str, _message: &str) -> Result<(), &'static str> {
            Ok(())
        }
    }

    fn config() -> StreamingConfig {
        StreamingConfig {
            min_speech_ms: 250,
            pre_roll_ms: 300,
            post_roll_ms: 700,
            max_utterance_ms: 30_000,
            auto_stop_silence_ms: 0,
        }
    }

    fn failure(code: &str) -> TranscribeFailure {
        TranscribeFailure {
            code: code.to_owned(),
            message: format!("{code} failed"),
        }
    }

    fn ok(text: &str) -> Result<String, TranscribeFailure> {
        Ok(text.to_owned())
    }

    /// A snippet's audio is irrelevant to the text decisions; one non-empty frame
    /// stands in so `merged_samples` is non-empty for `finalize`.
    fn snippet() -> Vec<f32> {
        vec![0.5; STREAM_FRAME_SAMPLES]
    }

    /// A decode port that models Whisper's fixed 30 s acoustic window: any buffer
    /// longer than one window collapses to a single truncated word (the real bug —
    /// a 50 s take decoded to 4 words), while a within-window buffer returns the
    /// next scripted result. Proves the finalize re-decode never hands the model an
    /// over-window block, so the collapse can never reach the committed text.
    struct WindowAwareTranscriber {
        within_window: VecDeque<Result<String, TranscribeFailure>>,
    }

    impl WindowAwareTranscriber {
        fn new(within_window: impl IntoIterator<Item = Result<String, TranscribeFailure>>) -> Self {
            Self {
                within_window: within_window.into_iter().collect(),
            }
        }
    }

    impl TakeTranscriber for WindowAwareTranscriber {
        fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
            if samples_f32_mono.len() > FINAL_CHUNK_SAMPLES {
                return Ok("traffic".to_owned());
            }
            self.within_window
                .pop_front()
                .expect("within-window transcriber called more times than scripted")
        }
    }

    /// `secs` seconds of 16 kHz mono speech audio — enough to span, or exceed, the
    /// finalize re-decode window.
    fn speech_snippet(secs: usize) -> Vec<f32> {
        vec![0.5; STREAM_SAMPLE_RATE_HZ as usize * secs]
    }

    /// Records the longest audio buffer it is ever asked to decode. The finalize
    /// re-decode must chunk a long take into ≤30 s windows, so no single decode may
    /// exceed one window.
    struct MaxLenTranscriber {
        max_len: Rc<Cell<usize>>,
    }

    impl TakeTranscriber for MaxLenTranscriber {
        fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
            self.max_len
                .set(self.max_len.get().max(samples_f32_mono.len()));
            Ok("word".to_owned())
        }
    }

    // Coverage of the ≤30 s chunked stop-time re-decode:
    //   * unit — the four tests below: the chunking, the per-chunk preview fallback,
    //     the boundary/hard-split planning, and the "no decode exceeds one window"
    //     guarantee, all deterministic against `StreamingTake::finalize`. These use a
    //     scripted decoder that MODELS the collapse; they pin the chunk PLUMBING, not
    //     what real Whisper does.
    //   * integration — `idiolect-ffi/tests/seam.rs`
    //     (`a_take_past_one_window_re_decodes_in_chunks_without_an_over_window_block`)
    //     drives a real ~45 s take through the mobile core with the real VAD, proving
    //     the whole edge→application→transcriber path chunks a long take (still a mock
    //     decoder, so still plumbing).
    //   * real audio — `idiolect-adapter-whisper/tests/real_long_take_finalize.rs`
    //     runs this exact `finalize` over a committed ~2 min 11 s LibriSpeech recording
    //     with the REAL Whisper adapter and asserts the finalized text is faithful (no
    //     collapse; the bulk of the ground-truth words recovered). Gated `#[ignore]`.
    //     NOTE: on real continuous speech whisper.cpp already windows a long buffer
    //     internally, so single-pass does not collapse and the chunking is ~neutral —
    //     the observed "50 s → 4 words" collapse reproduces on repeated/synthetic audio
    //     (repetition-penalty early-stop), not on the real recording. The desktop
    //     daemon and IBus engine consume this same pure `finalize` via the same
    //     `TakeTranscriber` seam, so no desktop-specific chunking remains to test.
    #[test]
    fn a_take_longer_than_one_window_is_rechunked_not_truncated() {
        // Two 20 s snippets = a 40 s take, longer than one 30 s window. The live
        // previews decode within-window; at stop the re-decode must run one ≤30 s
        // chunk at a time (here one chunk per snippet), so the over-window collapse
        // that truncated the single-pass whole-take decode can never reach the
        // committed text.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("preview one"), // fold snippet A (20 s ≤ window)
            ok("preview two"), // fold snippet B (20 s ≤ window)
            ok("final one"),   // finalize chunk 1 (snippet A, 20 s)
            ok("final two"),   // finalize chunk 2 (snippet B, 20 s)
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold B");

        match take.finalize(&mut transcriber) {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.final_text, "final one final two");
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn a_chunk_that_re_decodes_to_noise_keeps_its_own_preview() {
        // Two 20 s snippets → two finalize chunks. Chunk 1 re-decodes cleanly; chunk
        // 2 hears only noise at stop, so it keeps its live preview. The fallback is
        // per chunk — a noise-only re-decode never drops the words the user already
        // saw for that chunk, and it replaces only the chunk that did re-decode.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("preview one"),   // fold snippet A
            ok("preview two"),   // fold snippet B
            ok("final one"),     // finalize chunk 1 re-decodes cleanly
            ok("[BLANK_AUDIO]"), // finalize chunk 2 hears only noise → keep its preview
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold B");

        match take.finalize(&mut transcriber) {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.final_text, "final one preview two");
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn no_finalize_decode_ever_exceeds_one_window() {
        // A 60 s take (three 20 s snippets), well over one 30 s window. Every buffer
        // the finalize hands the model — previews and re-decode chunks alike — must
        // be ≤ one window; the over-window block that collapsed the single-pass
        // decode is never formed. The deterministic proof of the cut-off fix.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let max_len = Rc::new(Cell::new(0usize));
        let mut transcriber = MaxLenTranscriber {
            max_len: Rc::clone(&max_len),
        };

        for _ in 0..3 {
            take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
                .expect("fold");
        }
        assert!(matches!(
            take.finalize(&mut transcriber),
            TakeOutcome::Speech(_)
        ));
        assert!(max_len.get() > 0, "the transcriber was never called");
        assert!(
            max_len.get() <= FINAL_CHUNK_SAMPLES,
            "a decode of {} samples exceeded the {FINAL_CHUNK_SAMPLES}-sample window",
            max_len.get(),
        );
    }

    #[test]
    fn plan_chunks_groups_on_boundaries_and_hard_splits_an_over_window_span() {
        // Contiguous spans (in fold order): 20 s, 20 s, then a lone 50 s span. The two
        // 20 s spans can't share a chunk (40 s > window) so they split on the pause
        // boundary; the 50 s span is hard-split into 30 s + 20 s pieces, its preview
        // riding the first piece and every piece within one window.
        let s20 = 20 * STREAM_SAMPLE_RATE_HZ as usize;
        let s50 = 50 * STREAM_SAMPLE_RATE_HZ as usize;
        let spans = vec![
            SnippetSpan {
                range: 0..s20,
                preview: "a".to_owned(),
            },
            SnippetSpan {
                range: s20..(2 * s20),
                preview: "b".to_owned(),
            },
            SnippetSpan {
                range: (2 * s20)..(2 * s20 + s50),
                preview: "c".to_owned(),
            },
        ];

        let chunks = plan_chunks(&spans);

        let ranges: Vec<(usize, usize)> = chunks
            .iter()
            .map(|c| (c.range.start, c.range.end))
            .collect();
        assert_eq!(
            ranges,
            vec![
                (0, s20),
                (s20, 2 * s20),
                (2 * s20, 2 * s20 + FINAL_CHUNK_SAMPLES), // first 30 s of the 50 s span
                (2 * s20 + FINAL_CHUNK_SAMPLES, 2 * s20 + s50), // its remaining 20 s
            ]
        );
        let previews: Vec<&str> = chunks.iter().map(|c| c.preview.as_str()).collect();
        assert_eq!(previews, vec!["a", "b", "c", ""]);
        assert!(
            chunks
                .iter()
                .all(|c| c.range.end - c.range.start <= FINAL_CHUNK_SAMPLES),
            "every chunk must be within one window",
        );
    }

    #[test]
    fn snippets_accumulate_with_joining_spaces_and_emit_partials() {
        let mut take = StreamingTake::new(&config());
        let mut transcriber = ScriptedTranscriber::new([ok("restart traffic"), ok("deploy nginx")]);
        let mut observer = RecordingObserver::default();

        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");

        // The first snippet has no joining space; the second carries exactly one,
        // so what the engine types reads as one flowing sentence.
        assert_eq!(observer.committed, ["restart traffic", " deploy nginx"]);
        assert!(observer.dropped.is_empty());
        assert!(observer.failures.is_empty());
    }

    #[test]
    fn a_noise_only_snippet_is_dropped_but_its_audio_is_kept() {
        let mut take = StreamingTake::new(&config());
        let mut transcriber =
            ScriptedTranscriber::new([ok("restart traffic"), ok("[BLANK_AUDIO]")]);
        let mut observer = RecordingObserver::default();

        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");

        assert_eq!(observer.committed, ["restart traffic"]);
        assert_eq!(observer.dropped, ["[BLANK_AUDIO]"]);
        // Both snippets' audio is in the merged recording (the dropped one too):
        // a stop-time decode of the whole take can still recover its words.
        let outcome = take.finalize(&mut ScriptedTranscriber::new([ok("restart traffic")]));
        match outcome {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.merged_samples.len(), 2 * STREAM_FRAME_SAMPLES);
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn a_decode_failure_notifies_once_per_take_per_code_and_keeps_no_audio() {
        let mut take = StreamingTake::new(&config());
        // Same code twice, then a different code, all failing.
        let mut transcriber = ScriptedTranscriber::new([
            Err(failure("translation-unavailable")),
            Err(failure("translation-unavailable")),
            Err(failure("asr-unavailable")),
        ]);
        let mut observer = RecordingObserver::default();

        for _ in 0..3 {
            take.fold_snippet(&mut transcriber, &mut observer, snippet())
                .expect("fold");
        }

        // One notification for the repeated code, one for the new code.
        assert_eq!(
            observer.failures,
            [
                (
                    "translation-unavailable".to_owned(),
                    "translation-unavailable failed".to_owned()
                ),
                (
                    "asr-unavailable".to_owned(),
                    "asr-unavailable failed".to_owned()
                ),
            ]
        );
        assert!(observer.committed.is_empty());
        // No snippet decoded, so nothing was folded: the take is silent.
        assert_eq!(
            take.finalize(&mut ScriptedTranscriber::new([])),
            TakeOutcome::Silent
        );
    }

    #[test]
    fn a_fresh_take_re_arms_the_error_notification() {
        let mut first = StreamingTake::new(&config());
        let mut transcriber = ScriptedTranscriber::new([Err(failure("translation-unavailable"))]);
        let mut observer = RecordingObserver::default();
        first
            .fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");

        let mut second = StreamingTake::new(&config());
        let mut transcriber = ScriptedTranscriber::new([Err(failure("translation-unavailable"))]);
        second
            .fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");

        assert_eq!(observer.failures.len(), 2, "each take notifies afresh");
    }

    #[test]
    fn an_observer_emit_error_propagates() {
        let mut take = StreamingTake::new(&config());
        let mut transcriber = ScriptedTranscriber::new([ok("restart traffic")]);
        let result = take.fold_snippet(&mut transcriber, &mut FailingObserver, snippet());
        assert_eq!(result, Err("client gone"));
    }

    #[test]
    fn finalize_prefers_the_whole_take_decode_over_the_previews() {
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        // Two short-context previews lost a word; the whole-take decode recovers it.
        take.fold_snippet(
            &mut ScriptedTranscriber::new([ok("I want")]),
            &mut observer,
            snippet(),
        )
        .expect("fold");
        take.fold_snippet(
            &mut ScriptedTranscriber::new([ok("to leave")]),
            &mut observer,
            snippet(),
        )
        .expect("fold");

        let outcome = take.finalize(&mut ScriptedTranscriber::new([ok("I don't want to leave")]));
        match outcome {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.final_text, "I don't want to leave");
                assert_eq!(finalized.last_snippet_text.as_deref(), Some("to leave"));
                assert!(finalized.fallback_reason.is_none());
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn finalize_falls_back_to_previews_when_the_stop_decode_fails() {
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        take.fold_snippet(
            &mut ScriptedTranscriber::new([ok("restart traffic")]),
            &mut observer,
            snippet(),
        )
        .expect("fold");
        take.fold_snippet(
            &mut ScriptedTranscriber::new([ok("deploy nginx")]),
            &mut observer,
            snippet(),
        )
        .expect("fold");

        let outcome = take.finalize(&mut ScriptedTranscriber::new([Err(failure("stop-decode"))]));
        match outcome {
            TakeOutcome::Speech(finalized) => {
                // The glued previews — never lose what the user already saw typed.
                assert_eq!(finalized.final_text, "restart traffic deploy nginx");
                assert_eq!(
                    finalized.fallback_reason.as_deref(),
                    Some("stop-decode failed")
                );
            }
            TakeOutcome::Silent => panic!("expected the previewed fallback"),
        }
    }

    #[test]
    fn a_boxed_transcriber_drives_fold_and_finalize() {
        // The mobile facade holds the decoder as `Box<dyn TakeTranscriber + Send>`;
        // it must drive the take exactly like a concrete one.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber: Box<dyn TakeTranscriber + Send> =
            Box::new(ScriptedTranscriber::new([ok("restart traffic")]));

        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        assert_eq!(observer.committed, ["restart traffic"]);

        let mut finalize: Box<dyn TakeTranscriber + Send> =
            Box::new(ScriptedTranscriber::new([ok("restart traffic")]));
        match take.finalize(&mut finalize) {
            TakeOutcome::Speech(finalized) => assert_eq!(finalized.final_text, "restart traffic"),
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn finalize_of_an_empty_take_is_silent() {
        let take = StreamingTake::new(&config());
        assert_eq!(
            take.finalize(&mut ScriptedTranscriber::new([])),
            TakeOutcome::Silent
        );
    }

    // --- finalize_phrase: continuous mode commits each phrase as the speaker pauses,
    // then keeps the SAME take open for the next phrase (vs finalize, which ends the
    // take). Each phrase is still an authoritative whole-phrase decode. ---

    #[test]
    fn finalize_phrase_decodes_the_phrase_then_keeps_the_take_open_for_the_next() {
        let mut take = StreamingTake::new(&config());
        let mut transcriber = ScriptedTranscriber::new([
            ok("restart"),         // phrase 1 snippet preview
            ok("restart traffic"), // phrase 1 whole-phrase decode
            ok("deploy"),          // phrase 2 snippet preview
            ok("deploy nginx"),    // phrase 2 whole-phrase decode
        ]);
        let mut observer = RecordingObserver::default();

        // Phrase 1: a pause-completed snippet, then the phrase boundary.
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        match take.finalize_phrase(&mut transcriber) {
            TakeOutcome::Speech(f) => assert_eq!(f.final_text, "restart traffic"),
            TakeOutcome::Silent => panic!("expected phrase 1 speech"),
        }

        // The take is reusable: phrase 2's text must NOT carry phrase 1's words
        // (the accumulators reset at the boundary).
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        match take.finalize_phrase(&mut transcriber) {
            TakeOutcome::Speech(f) => assert_eq!(f.final_text, "deploy nginx"),
            TakeOutcome::Silent => panic!("expected phrase 2 speech"),
        }
    }

    #[test]
    fn finalize_phrase_with_no_audio_is_silent_and_decodes_nothing() {
        let mut take = StreamingTake::new(&config());
        // An empty scripted transcriber: if finalize_phrase tried to decode an empty
        // phrase it would panic ("called more times than scripted").
        let mut transcriber = ScriptedTranscriber::new([]);
        assert_eq!(take.finalize_phrase(&mut transcriber), TakeOutcome::Silent);
    }

    #[test]
    fn finalize_phrase_re_arms_the_once_per_phrase_failure_notice() {
        // A decode failure is surfaced at most once per *phrase*: after the boundary
        // the same code may notify again (continuous can't go silent after one glitch).
        let mut take = StreamingTake::new(&config());
        let mut transcriber =
            ScriptedTranscriber::new([Err(failure("decode")), Err(failure("decode"))]);
        let mut observer = RecordingObserver::default();

        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        let _ = take.finalize_phrase(&mut transcriber); // Silent (failed fold kept no audio)
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");

        assert_eq!(observer.failures.len(), 2, "each phrase re-arms the notice");
    }

    #[test]
    fn ingest_emits_a_snippet_at_a_pause_using_the_supplied_verdict() {
        let mut take = StreamingTake::new(&StreamingConfig {
            min_speech_ms: 60, // 2 frames of speech is enough
            pre_roll_ms: 0,    // keep the arithmetic legible
            post_roll_ms: 90,  // 3 silent frames end the utterance
            max_utterance_ms: 30_000,
            auto_stop_silence_ms: 0,
        });
        // Five frames of "speech" then three of "silence", driven by an index so
        // the verdict is deterministic without a real VAD.
        let mut frame_index = 0_usize;
        let mut is_speech = move |_frame: &[f32]| {
            let speech = frame_index < 5;
            frame_index += 1;
            speech
        };
        let audio = vec![0.4; STREAM_FRAME_SAMPLES * 8];

        let snippets = take.ingest(&audio, &mut is_speech);
        assert_eq!(snippets.len(), 1, "the pause completes exactly one snippet");
        // 5 speech + 3 trailing-silence frames = 8 frames of audio.
        assert_eq!(snippets[0].len(), 8 * STREAM_FRAME_SAMPLES);
    }

    #[test]
    fn auto_stop_arms_only_after_speech_and_respects_the_threshold() {
        // A 2 s threshold ⇒ 2000 / 30 ≈ 67 silent frames after speech.
        let mut take = StreamingTake::new(&StreamingConfig {
            auto_stop_silence_ms: 2_000,
            ..config()
        });

        // Pre-speech silence never stops the take.
        take.ingest(&vec![0.0; STREAM_FRAME_SAMPLES * 100], |_| false);
        assert!(!take.auto_stop_due(), "pre-speech silence is thinking time");

        // Speak one frame, then go quiet for well over the threshold.
        take.ingest(&vec![0.5; STREAM_FRAME_SAMPLES], |_| true);
        assert!(!take.auto_stop_due(), "no trailing silence yet");
        take.ingest(&vec![0.0; STREAM_FRAME_SAMPLES * 70], |_| false);
        assert!(take.auto_stop_due(), "threshold crossed after speech");
    }

    #[test]
    fn auto_stop_zero_never_fires() {
        let mut take = StreamingTake::new(&StreamingConfig {
            auto_stop_silence_ms: 0,
            ..config()
        });
        take.ingest(&vec![0.5; STREAM_FRAME_SAMPLES], |_| true);
        take.ingest(&vec![0.0; STREAM_FRAME_SAMPLES * 1_000], |_| false);
        assert!(!take.auto_stop_due(), "0 disables auto-stop entirely");
    }
}
