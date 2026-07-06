//! Real-audio, real-Whisper coverage of the stop-time finalize on a LONG take.
//!
//! Every other streaming test drives a mock/scripted decoder or the fixed-string
//! `FixtureAsr`, so none of them can show what the real Whisper adapter does with a
//! take that is several 30 s windows long — the exact case the finalize re-decode
//! exists for. This one feeds the committed ~2 min 11 s LibriSpeech recording (real
//! continuous human speech, with the corpus's own aligned transcript as ground
//! truth) through the true `StreamingTake` fold → finalize path with the real
//! `WhisperAsr`, and pins that a long take finalizes to a FAITHFUL transcript — it
//! does not collapse to a handful of words, and it recovers the bulk of the spoken
//! words.
//!
//! Gated `#[ignore]` because it loads the Whisper model and decodes ~2 min of audio
//! (seconds, not milliseconds). Run it with:
//!
//! ```sh
//! cargo test -p idiolect-adapter-whisper --test real_long_take_finalize -- --ignored
//! ```

use std::collections::HashSet;
use std::path::PathBuf;

use idiolect_adapter_whisper::WhisperAsr;
use idiolect_application::use_cases::streaming::{
    StreamObserver, StreamingConfig, StreamingTake, TakeOutcome, TakeTranscriber,
    TranscribeFailure,
};
use idiolect_ports::asr::AsrPort;
use idiolect_ports::audio::AudioSegment;

/// The real Whisper adapter bound to the take's decode port — the same shape the
/// daemon's `DaemonTranscriber` and the mobile `WhisperTakeTranscriber` use.
struct WhisperTakeTranscriber {
    asr: WhisperAsr,
}

impl TakeTranscriber for WhisperTakeTranscriber {
    fn transcribe(&mut self, samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        self.asr
            .transcribe(&AudioSegment {
                sample_rate_hz: 16_000,
                channels: 1,
                duration_ms: (samples_f32_mono.len() as u64 * 1_000 / 16_000) as u32,
                samples_f32_mono: samples_f32_mono.to_vec(),
            })
            .map(|draft| draft.text)
            .map_err(|error| TranscribeFailure {
                code: "asr-error".to_owned(),
                message: error.to_string(),
            })
    }
}

/// Discards live events — this test asserts on the finalized text, not the stream.
struct NullObserver;

impl StreamObserver for NullObserver {
    type Error = std::convert::Infallible;
    fn snippet_committed(&mut self, _chunk: &str) -> Result<(), Self::Error> {
        Ok(())
    }
    fn snippet_dropped(&mut self, _decoded: &str) -> Result<(), Self::Error> {
        Ok(())
    }
    fn transcribe_failed(&mut self, _code: &str, _message: &str) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/audio")
        .join(name)
}

/// Loads a committed 16 kHz mono PCM-s16le WAV fixture as f32 samples.
fn load_wav_16khz_mono(name: &str) -> Vec<f32> {
    let bytes = std::fs::read(fixture_path(name)).expect("fixture wav present");
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF wav");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE wav");
    let mut cursor = 12;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let start = cursor + 8;
        if id == b"data" {
            return bytes[start..start + size]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0)
                .collect();
        }
        cursor = start + size + (size & 1);
    }
    panic!("wav has no data chunk");
}

/// The set of lower-cased alphanumeric word stems in a transcript — the unit the
/// faithfulness check compares against, so casing and punctuation never matter.
fn word_set(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

#[test]
#[ignore = "loads the Whisper model and decodes ~2 min of audio; run explicitly"]
fn a_real_two_minute_take_finalizes_to_a_faithful_transcript() {
    let audio = load_wav_16khz_mono("librispeech_8555_292519_16khz_mono.wav");
    let reference = std::fs::read_to_string(fixture_path("librispeech_8555_292519.txt"))
        .expect("reference transcript present");
    let reference_words = word_set(&reference);
    let take_secs = audio.len() as f32 / 16_000.0;
    assert!(
        take_secs > 120.0,
        "the fixture must be a genuinely long take (got {take_secs:.1}s)"
    );

    let mut transcriber = WhisperTakeTranscriber {
        asr: WhisperAsr::load_fixture_model().expect("whisper fixture model present"),
    };
    let mut observer = NullObserver;
    let mut take = StreamingTake::new(&StreamingConfig {
        min_speech_ms: 200,
        pre_roll_ms: 300,
        post_roll_ms: 700,
        max_utterance_ms: 30_000,
        auto_stop_silence_ms: 0,
    });

    // Fold the take as a run of pause-delimited snippets — each ≤ the 30 s utterance
    // cap, exactly as the live VAD would hand them over — so finalize has real
    // multi-snippet spans to plan its ≤30 s re-decode chunks over.
    let snippet_len = 16_000 * 25; // 25 s snippets
    for slice in audio.chunks(snippet_len) {
        take.fold_snippet(&mut transcriber, &mut observer, slice.to_vec())
            .expect("fold");
    }

    let TakeOutcome::Speech(finalized) = take.finalize(&mut transcriber) else {
        panic!("a 2 min take of speech must finalize as Speech, not Silent");
    };

    let final_words: Vec<&str> = finalized.final_text.split_whitespace().collect();
    let decoded = word_set(&finalized.final_text);
    let recovered = decoded.intersection(&reference_words).count();
    let recall = recovered as f32 / reference_words.len() as f32;

    // 1) It did NOT collapse. The bug the finalize re-decode guards against turned a
    //    long take into a handful of words; a faithful decode of this take is a few
    //    hundred words. A floor of 150 is far above any collapse, far below the truth.
    assert!(
        final_words.len() >= 150,
        "a 2 min take collapsed to {} words: {:?}",
        final_words.len(),
        finalized.final_text
    );

    // 2) It is FAITHFUL. The finalized text recovers the bulk of the spoken words
    //    (measured against LibriSpeech's own aligned ground truth). A 0.6 floor
    //    leaves headroom for the tiny model's slips and chunk-boundary trims while
    //    still failing hard if finalize ever starts dropping or garbling the take.
    assert!(
        recall >= 0.6,
        "finalized transcript recovered only {recovered}/{} reference words ({:.0}%): {:?}",
        reference_words.len(),
        recall * 100.0,
        finalized.final_text
    );
}
