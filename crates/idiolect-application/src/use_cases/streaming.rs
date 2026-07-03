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
/// finalize re-decode runs one ≤30 s chunk at a time and the chunks are stitched
/// back together. Aligned to snippet (pause) boundaries where possible so a chunk
/// edge never falls mid-word.
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

    /// The authoritative stop-time decode advanced by one ≤30 s chunk. `full_text`
    /// is the WHOLE take as it now stands: the chunks decoded so far replaced by
    /// their authoritative text, followed by the still-preview text of the chunks
    /// not yet re-decoded. The edge replaces the live preedit with this each time
    /// so a long take firms up in place, chunk by chunk, instead of one big swap
    /// (or a truncated one). Default no-op: an edge that only needs the single
    /// final text can ignore the intermediate steps.
    fn finalize_progress(&mut self, full_text: &str) -> Result<(), Self::Error> {
        let _ = full_text;
        Ok(())
    }
}

/// One folded snippet held for the stop-time decode: its 16 kHz mono audio and
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
/// [`Self::fold_snippet`]; the WHOLE take is decoded once and closed out via
/// [`Self::finalize`]. The audio→snippet plumbing (resampler, VAD) stays at the
/// edge; this owns the segmenter, the accumulators, the auto-stop clock, and the
/// once-per-take error de-duplication.
pub struct StreamingTake {
    frames: FrameBuffer,
    segmenter: UtteranceSegmenter,
    /// Every folded snippet, in order: its audio plus the preview text it
    /// contributed (empty for a noise/dropped snippet whose audio is still kept).
    /// The take's stored recording is these snippets' audio concatenated; the
    /// stop-time decode groups them into ≤30 s chunks so each authoritative
    /// re-decode stays inside Whisper's window (the *cut-off-on-long-takes* fix).
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

    /// Closes the take: re-decodes the recording with the proper model — the
    /// authoritative text — one ≤30 s chunk at a time, replacing each chunk's
    /// preview and pushing the whole-take-so-far via
    /// [`StreamObserver::finalize_progress`] as it goes. A chunk whose decode fails
    /// or hears only noise keeps that chunk's preview text (never lose words the
    /// user already saw typed). An empty take, or one that decodes to nothing, is
    /// [`TakeOutcome::Silent`].
    pub fn finalize<T, O>(
        mut self,
        transcriber: &mut T,
        observer: &mut O,
    ) -> Result<TakeOutcome, O::Error>
    where
        T: TakeTranscriber,
        O: StreamObserver,
    {
        // Ending the take is finalizing its one (and only) phrase.
        self.finalize_phrase(transcriber, observer)
    }

    /// Closes the CURRENT phrase and re-opens the take for the next one: the same
    /// authoritative chunked re-decode as [`Self::finalize`] (per ≤30 s chunk,
    /// fallback to that chunk's preview, progressive [`StreamObserver::finalize_progress`],
    /// [`TakeOutcome::Silent`] when empty/noise), but the accumulators and per-phrase
    /// error de-dupe reset IN PLACE so dictation continues. Continuous mode calls this
    /// at each pause; one-shot dictation calls [`Self::finalize`] once at stop. The
    /// segmenter and frame buffer are left intact (a pause has already returned the
    /// segmenter to idle), so the next phrase picks up cleanly.
    pub fn finalize_phrase<T, O>(
        &mut self,
        transcriber: &mut T,
        observer: &mut O,
    ) -> Result<TakeOutcome, O::Error>
    where
        T: TakeTranscriber,
        O: StreamObserver,
    {
        let snippets = core::mem::take(&mut self.snippets);
        let _ = core::mem::take(&mut self.merged_text);
        let last_snippet_text = self.last_snippet_text.take();
        // Re-arm per-phrase state: each phrase is independent (own error budget, own
        // auto-stop clock) so one phrase's glitch can't mute or auto-stop the next.
        self.notified_error = None;
        self.spoke = false;
        self.silence_frames_since_speech = 0;

        if snippets.is_empty() {
            return Ok(TakeOutcome::Silent);
        }

        // The stored recording is every snippet's audio, concatenated in order.
        let merged_samples: Vec<f32> = snippets
            .iter()
            .flat_map(|snippet| snippet.samples.iter().copied())
            .collect();

        // Group consecutive snippets into ≤30 s chunks and re-decode each, so a long
        // take never hands Whisper one over-window block (which collapses the decode).
        let chunks = chunk_snippets(&snippets);
        let mut final_parts: Vec<String> = Vec::with_capacity(chunks.len());
        let mut fallback_reason: Option<String> = None;
        for (index, chunk) in chunks.iter().enumerate() {
            let chunk_text = match transcriber.transcribe(&chunk.samples) {
                Ok(text) => choose_final_take_text(text, chunk.preview.clone()),
                Err(failure) => {
                    fallback_reason.get_or_insert(failure.message);
                    chunk.preview.clone()
                }
            };
            final_parts.push(chunk_text);
            // The whole take as it now stands: chunks decoded so far, then the
            // still-preview text of the chunks not yet re-decoded.
            let mut whole = final_parts.clone();
            whole.extend(chunks[index + 1..].iter().map(|tail| tail.preview.clone()));
            observer.finalize_progress(&normalize_take_text(&whole.join(" ")))?;
        }

        let final_text = normalize_take_text(&final_parts.join(" "));
        if final_text.trim().is_empty() {
            return Ok(TakeOutcome::Silent);
        }
        Ok(TakeOutcome::Speech(FinalizedTake {
            final_text,
            merged_samples,
            last_snippet_text,
            fallback_reason,
        }))
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

/// One ≤30 s re-decode unit: consecutive snippets' audio plus their glued preview
/// text, used as the per-chunk fallback when the chunk's decode fails or is noise.
struct FinalChunk {
    samples: Vec<f32>,
    preview: String,
}

/// Groups snippets into chunks of at most [`FINAL_CHUNK_SAMPLES`] (30 s), on
/// snippet (pause) boundaries so a chunk edge never lands mid-word. A lone snippet
/// longer than the window (only possible when `max_utterance_ms` is configured
/// above 30 s) is hard-split into ≤30 s pieces, its preview riding the first piece.
fn chunk_snippets(snippets: &[TakeSnippet]) -> Vec<FinalChunk> {
    let mut chunks: Vec<FinalChunk> = Vec::new();
    let mut samples: Vec<f32> = Vec::new();
    let mut previews: Vec<String> = Vec::new();

    let seal = |samples: &mut Vec<f32>, previews: &mut Vec<String>, chunks: &mut Vec<FinalChunk>| {
        if !samples.is_empty() {
            chunks.push(FinalChunk {
                samples: core::mem::take(samples),
                preview: glue_previews(&core::mem::take(previews)),
            });
        }
    };

    for snippet in snippets {
        if snippet.samples.len() > FINAL_CHUNK_SAMPLES {
            // Over-window snippet: seal what's pending, then hard-split it.
            seal(&mut samples, &mut previews, &mut chunks);
            for (piece_index, piece) in snippet.samples.chunks(FINAL_CHUNK_SAMPLES).enumerate() {
                chunks.push(FinalChunk {
                    samples: piece.to_vec(),
                    preview: if piece_index == 0 {
                        snippet.preview.clone()
                    } else {
                        String::new()
                    },
                });
            }
            continue;
        }
        if !samples.is_empty() && samples.len() + snippet.samples.len() > FINAL_CHUNK_SAMPLES {
            seal(&mut samples, &mut previews, &mut chunks);
        }
        samples.extend_from_slice(&snippet.samples);
        previews.push(snippet.preview.clone());
    }
    seal(&mut samples, &mut previews, &mut chunks);
    chunks
}

/// Joins the non-empty snippet previews of one chunk with single spaces (dropped
/// snippets contribute no words but their audio is still in the chunk).
fn glue_previews(previews: &[String]) -> String {
    previews
        .iter()
        .filter(|preview| !preview.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapses runs of whitespace to single spaces and trims — the same
/// normalisation the whole-recording decode used, applied to the stitched chunks.
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
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use super::{
        StreamObserver, StreamingConfig, StreamingTake, TakeOutcome, TakeTranscriber,
        TranscribeFailure, FINAL_CHUNK_SAMPLES, STREAM_FRAME_SAMPLES,
    };

    /// A decode port that mimics Whisper's long-audio collapse: any buffer longer
    /// than one 30 s window returns a single truncated word (the real bug — a 50 s
    /// take decoded to 4 words), while a within-window buffer returns the next
    /// scripted result. Proves the finalize re-decode never hands the engine an
    /// over-window block, so the collapse can't reach the committed text.
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
                return Ok("collapsed".to_owned());
            }
            self.within_window
                .pop_front()
                .expect("transcriber called more times than scripted")
        }
    }

    /// One snippet's worth of speech audio, `secs` long (non-zero so it is never
    /// mistaken for silence). Fed straight to `fold_snippet`, bypassing the segmenter.
    fn speech_snippet(secs: usize) -> Vec<f32> {
        vec![0.4; super::STREAM_SAMPLE_RATE_HZ as usize * secs]
    }

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
        /// The whole-take text pushed after each finalize chunk, in order.
        progress: Vec<String>,
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
        fn finalize_progress(&mut self, full_text: &str) -> Result<(), Infallible> {
            self.progress.push(full_text.to_owned());
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
        let outcome = take
            .finalize(
                &mut ScriptedTranscriber::new([ok("restart traffic")]),
                &mut observer,
            )
            .expect("finalize");
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
            take.finalize(&mut ScriptedTranscriber::new([]), &mut observer)
                .expect("finalize"),
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

        let outcome = take
            .finalize(
                &mut ScriptedTranscriber::new([ok("I don't want to leave")]),
                &mut observer,
            )
            .expect("finalize");
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

        let outcome = take
            .finalize(
                &mut ScriptedTranscriber::new([Err(failure("stop-decode"))]),
                &mut observer,
            )
            .expect("finalize");
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
        match take.finalize(&mut finalize, &mut observer).expect("finalize") {
            TakeOutcome::Speech(finalized) => assert_eq!(finalized.final_text, "restart traffic"),
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn finalize_of_an_empty_take_is_silent() {
        let take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        assert_eq!(
            take.finalize(&mut ScriptedTranscriber::new([]), &mut observer)
                .expect("finalize"),
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
        match take
            .finalize_phrase(&mut transcriber, &mut observer)
            .expect("finalize")
        {
            TakeOutcome::Speech(f) => assert_eq!(f.final_text, "restart traffic"),
            TakeOutcome::Silent => panic!("expected phrase 1 speech"),
        }

        // The take is reusable: phrase 2's text must NOT carry phrase 1's words
        // (the accumulators reset at the boundary).
        take.fold_snippet(&mut transcriber, &mut observer, snippet())
            .expect("fold");
        match take
            .finalize_phrase(&mut transcriber, &mut observer)
            .expect("finalize")
        {
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
        let mut observer = RecordingObserver::default();
        assert_eq!(
            take.finalize_phrase(&mut transcriber, &mut observer)
                .expect("finalize"),
            TakeOutcome::Silent
        );
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
        // Silent (failed fold kept no audio).
        let _ = take
            .finalize_phrase(&mut transcriber, &mut observer)
            .expect("finalize");
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

    // --- The cut-off-on-long-takes fix: the finalize re-decode is chunked into
    // ≤30 s windows so a long take never hands Whisper one over-window block (which
    // collapses the decode), and the chunks firm up the preedit progressively. ---

    #[test]
    fn a_long_take_re_decodes_per_chunk_instead_of_collapsing_the_whole_recording() {
        // Two 20 s snippets ⇒ 40 s total ⇒ two ≤30 s chunks. A whole-recording decode
        // (>30 s) would collapse to "collapsed"; the chunked decode must keep both.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("preview one"),  // fold snippet A (20 s ≤ window)
            ok("preview two"),  // fold snippet B (20 s ≤ window)
            ok("final one"),    // finalize chunk 1 (snippet A, 20 s)
            ok("final two"),    // finalize chunk 2 (snippet B, 20 s)
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold B");

        let outcome = take
            .finalize(&mut transcriber, &mut observer)
            .expect("finalize");
        match outcome {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(
                    finalized.final_text, "final one final two",
                    "both chunks survive; the over-window collapse never reaches the take"
                );
                assert!(finalized.fallback_reason.is_none());
                // The whole 40 s of audio is still stored for training.
                assert_eq!(
                    finalized.merged_samples.len(),
                    super::STREAM_SAMPLE_RATE_HZ as usize * 40
                );
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn finalize_replaces_the_preview_chunk_by_chunk_as_it_goes() {
        // The preedit firms up in place: after chunk 1 the first 20 s is authoritative
        // while the second is still its preview; after chunk 2 the whole take is final.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("preview one"),
            ok("preview two"),
            ok("final one"),
            ok("final two"),
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold B");
        take.finalize(&mut transcriber, &mut observer)
            .expect("finalize");

        assert_eq!(
            observer.progress,
            [
                "final one preview two", // chunk 1 final + chunk 2 still preview
                "final one final two",   // both chunks final
            ]
        );
    }

    #[test]
    fn a_chunk_whose_re_decode_is_noise_keeps_that_chunks_preview() {
        // Per-chunk fallback: only the first-pass preview of a chunk that fails to
        // re-decode is kept; the other chunk's authoritative text still lands.
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("preview one"),
            ok("preview two"),
            ok("final one"),      // chunk 1 re-decodes cleanly
            ok("[BLANK_AUDIO]"),  // chunk 2 re-decodes to noise → keep its preview
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(20))
            .expect("fold B");

        match take
            .finalize(&mut transcriber, &mut observer)
            .expect("finalize")
        {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.final_text, "final one preview two");
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }

    #[test]
    fn a_short_take_still_re_decodes_the_whole_recording_once() {
        // A ≤30 s take is a single chunk: exactly one re-decode, and the whole-take
        // decode still wins over the previews (the streaming-drops-words guarantee).
        let mut take = StreamingTake::new(&config());
        let mut observer = RecordingObserver::default();
        let mut transcriber = WindowAwareTranscriber::new([
            ok("I want"),              // fold snippet A
            ok("to leave"),           // fold snippet B
            ok("I don't want to leave"), // single finalize chunk
        ]);

        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(5))
            .expect("fold A");
        take.fold_snippet(&mut transcriber, &mut observer, speech_snippet(5))
            .expect("fold B");

        match take
            .finalize(&mut transcriber, &mut observer)
            .expect("finalize")
        {
            TakeOutcome::Speech(finalized) => {
                assert_eq!(finalized.final_text, "I don't want to leave");
                assert_eq!(observer.progress, ["I don't want to leave"]);
            }
            TakeOutcome::Silent => panic!("expected a take"),
        }
    }
}
