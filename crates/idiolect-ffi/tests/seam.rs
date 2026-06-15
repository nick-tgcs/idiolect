//! Exercises the mobile core **through the UniFFI seam**: every assertion goes
//! via the exported `IdiolectCore` surface and the `IdiolectInputMethod` callback
//! trait — the same objects the Kotlin app drives — against a real on-disk SQLite
//! store, the real WebRTC VAD, and the real Opus codec. This is the host-runnable
//! proof of the streaming facade; the Kotlin `InputMethodService` *rendering* is
//! the declared GUI seam (M3), but all of its state logic lives here.
//!
//! A take follows the desktop's authoritative-full-take shape: `toggle()` starts
//! the mic, PCM is pushed via `push_pcm_frame`, each pause-completed snippet
//! previews as a partial preedit, and `toggle()` again decodes the WHOLE
//! recording once and commits it (the *streaming-drops-words* fix). The take's one
//! recording is persisted (the training-pair audio). Most tests drive a
//! deterministic scripted decoder; one drives the real on-device Whisper model.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use idiolect_application::use_cases::streaming::{TakeTranscriber, TranscribeFailure};
use idiolect_ffi::{FfiError, IdiolectCore, IdiolectInputMethod};
use idiolect_ports::audio::AudioSegment;
use idiolect_sync::decode_batch;
use idiolect_test_support::fixtures::{
    speech_and_silence_fixture_16khz_mono, speech_pause_speech_fixture_16khz_mono,
};

/// A test double for the Kotlin-side callback: records every push in order so the
/// tests can assert the exact Rust→front-end event stream.
#[derive(Clone, Default)]
struct RecordingCallback {
    events: Arc<Mutex<Vec<String>>>,
}

impl RecordingCallback {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    fn push(&self, line: String) {
        self.events.lock().unwrap().push(line);
    }
}

impl IdiolectInputMethod for RecordingCallback {
    fn recording_status(&self, recording: bool) {
        self.push(format!("recording_status:{recording}"));
    }
    fn show_preedit(&self, text: String) {
        self.push(format!("show_preedit:{text}"));
    }
    fn update_preedit(&self, text: String) {
        self.push(format!("update_preedit:{text}"));
    }
    fn commit_text(&self, text: String) {
        self.push(format!("commit_text:{text}"));
    }
    fn cancel_preedit(&self) {
        self.push("cancel_preedit".to_owned());
    }
    fn insert_text(&self, text: String) {
        self.push(format!("insert_text:{text}"));
    }
    fn edit_history(&self, id: i64, text: String) {
        self.push(format!("edit_history:{id}:{text}"));
    }
    fn dictation_error(&self, message: String) {
        self.push(format!("dictation_error:{message}"));
    }
}

/// A decoder that returns one fixed transcript for every snippet and the whole
/// take — decouples the streaming-wiring assertions from the count of snippets the
/// VAD happens to segment.
struct FixedTranscriber(String);

impl TakeTranscriber for FixedTranscriber {
    fn transcribe(&mut self, _samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        Ok(self.0.clone())
    }
}

/// A decoder that returns scripted results in call order (each pause-completed
/// snippet, then the whole-take finalize) — lets a test distinguish the per-snippet
/// preview path from the authoritative whole-take decode, and drive a finalize
/// failure independently of the snippet decodes.
struct ScriptedTranscriber(VecDeque<Result<String, TranscribeFailure>>);

impl ScriptedTranscriber {
    fn new(outputs: impl IntoIterator<Item = Result<String, TranscribeFailure>>) -> Self {
        Self(outputs.into_iter().collect())
    }
}

impl TakeTranscriber for ScriptedTranscriber {
    fn transcribe(&mut self, _samples_f32_mono: &[f32]) -> Result<String, TranscribeFailure> {
        self.0
            .pop_front()
            .expect("scripted transcriber called more times than scripted")
    }
}

/// Build a core over a throwaway data dir, returning the core plus a handle to the
/// callback's recorded event log. The dir is leaked so it outlives the core; the
/// test process is short-lived and the OS reclaims it.
fn new_core() -> (Arc<IdiolectCore>, RecordingCallback) {
    let dir = tempfile::tempdir().expect("tempdir");
    let callback = RecordingCallback::default();
    let core = IdiolectCore::new(
        dir.path().to_string_lossy().into_owned(),
        None,
        Box::new(callback.clone()),
    )
    .expect("core should open");
    std::mem::forget(dir);
    (core, callback)
}

/// Push a fixture clip as 16 kHz mono PCM in realistic ~100 ms buffers (the mic
/// FGS → JNI hot path), converting f32 → i16 as `AudioRecord` would deliver it.
fn push_audio(core: &IdiolectCore, audio: &AudioSegment) {
    let pcm: Vec<i16> = audio
        .samples_f32_mono
        .iter()
        .map(|sample| (sample * 32_768.0) as i16)
        .collect();
    for chunk in pcm.chunks(1_600) {
        core.push_pcm_frame(chunk.to_vec()).expect("push frame");
    }
}

/// Dictate one whole take end to end with a scripted decoder: start, stream the
/// speech fixture, stop (decode + commit).
fn dictate(core: &IdiolectCore, text: &str) {
    core.install_transcriber(Box::new(FixedTranscriber(text.to_owned())));
    core.toggle().unwrap(); // mic on
    push_audio(core, &speech_and_silence_fixture_16khz_mono());
    core.toggle().unwrap(); // mic off → the whole take is decoded + committed
}

/// The committed `ggml-tiny.en` fixture, reachable from this crate the same way
/// the Whisper adapter resolves it on the host.
fn fixture_model_path() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/whisper/ggml-tiny.en.bin")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn toggle_pushes_authoritative_recording_status_each_edge() {
    let (core, cb) = new_core();
    assert!(!core.is_recording());

    core.toggle().unwrap();
    assert!(core.is_recording());
    core.toggle().unwrap(); // empty take (no audio) → silent, nothing persisted
    assert!(!core.is_recording());

    // The UI never toggles optimistically — it mirrors these edge-triggered pushes.
    assert_eq!(
        cb.events(),
        ["recording_status:true", "recording_status:false"]
    );
    assert!(core.recent_history(10).unwrap().is_empty());
}

#[test]
fn a_streamed_take_previews_a_partial_then_commits_the_whole_take() {
    let (core, cb) = new_core();
    core.install_transcriber(Box::new(FixedTranscriber("restart traffic".to_owned())));

    core.toggle().unwrap();
    push_audio(&core, &speech_and_silence_fixture_16khz_mono());
    core.toggle().unwrap();

    let events = cb.events();
    assert_eq!(events.first().unwrap(), "recording_status:true");
    // The live preedit appears as the snippet decodes (a partial)…
    assert!(
        events.contains(&"show_preedit:restart traffic".to_owned()),
        "a live partial should appear: {events:?}"
    );
    // …and the authoritative whole-take decode commits it into the field.
    assert!(
        events.contains(&"commit_text:restart traffic".to_owned()),
        "the whole take should commit: {events:?}"
    );
    assert_eq!(events.last().unwrap(), "recording_status:false");

    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart traffic");
    assert!(history[0].committed);
}

#[test]
fn a_committed_take_persists_the_training_pair_audio() {
    // Manage the dir locally so we can inspect the on-disk recording.
    let dir = tempfile::tempdir().expect("tempdir");
    let core = IdiolectCore::new(
        dir.path().to_string_lossy().into_owned(),
        None,
        Box::new(RecordingCallback::default()),
    )
    .expect("core should open");
    core.install_transcriber(Box::new(FixedTranscriber("restart traffic".to_owned())));

    core.toggle().unwrap();
    push_audio(&core, &speech_and_silence_fixture_16khz_mono());
    core.toggle().unwrap();

    // The take's single recording is written to the audio store — the audio half
    // of the raw→corrected training pair the phone later ships to the PC.
    let recordings = ogg_files(&dir.path().join("audio"));
    assert_eq!(
        recordings.len(),
        1,
        "exactly one source recording should persist"
    );
    // …and it is a real, non-empty Opus container (the `IDOPUS1` payload the codec
    // writes), not a zero-byte placeholder — proving `write_source_audio` ran with
    // genuinely encoded audio.
    let payload = std::fs::read(&recordings[0]).expect("read recording");
    assert!(
        payload.starts_with(b"IDOPUS1"),
        "recording is an IDOPUS1 payload"
    );
    assert!(
        payload.len() > b"IDOPUS1".len(),
        "recording carries encoded audio beyond the header"
    );
}

#[test]
fn correction_after_commit_amends_the_persisted_take() {
    let (core, _cb) = new_core();
    dictate(&core, "restart traffic");
    core.report_correction("restart Traefik".to_owned())
        .unwrap();

    // The committed text is rewritten in place; the history projection reflects it.
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart Traefik");
}

#[test]
fn cancel_aborts_the_recording_and_persists_nothing() {
    let (core, cb) = new_core();
    core.install_transcriber(Box::new(FixedTranscriber("hello world".to_owned())));

    core.toggle().unwrap();
    // This clip pauses mid-stream, so a partial preedit is shown before we abort.
    push_audio(&core, &speech_pause_speech_fixture_16khz_mono());
    core.cancel().unwrap();

    let events = cb.events();
    assert!(
        events.iter().any(|e| e.starts_with("show_preedit")),
        "a partial was shown before cancel: {events:?}"
    );
    assert!(
        events.contains(&"cancel_preedit".to_owned()),
        "cancel clears the live preedit: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e.starts_with("commit_text")),
        "an aborted take never commits: {events:?}"
    );
    // The session is created only at finalize, so an aborted take leaves no row.
    assert!(core.recent_history(10).unwrap().is_empty());
    assert!(!core.is_recording());
}

#[test]
fn push_pcm_frame_is_rejected_unless_a_take_is_recording() {
    let (core, _cb) = new_core();
    // No active take: a stray frame is rejected rather than silently buffered.
    assert!(core.push_pcm_frame(vec![0; 160]).is_err());

    core.toggle().unwrap();
    assert!(core.push_pcm_frame(vec![0; 160]).is_ok());

    core.cancel().unwrap();
    assert!(core.push_pcm_frame(vec![0; 160]).is_err());
}

#[test]
fn a_take_with_no_model_warns_once_and_persists_nothing() {
    // No transcriber installed ⇒ the default reports "no model"; every snippet
    // fails to decode.
    let (core, cb) = new_core();

    core.toggle().unwrap();
    push_audio(&core, &speech_pause_speech_fixture_16khz_mono());
    core.toggle().unwrap();

    let events = cb.events();
    let warnings = events
        .iter()
        .filter(|e| e.starts_with("dictation_error"))
        .count();
    // De-duplicated to once per take per cause, not once per failed snippet.
    assert_eq!(warnings, 1, "warned exactly once per take: {events:?}");
    assert!(
        !events.iter().any(|e| e.starts_with("commit_text")),
        "nothing is committed when no model is loaded: {events:?}"
    );
    assert!(core.recent_history(10).unwrap().is_empty());
}

#[test]
fn history_edited_amends_a_past_entry_by_id() {
    let (core, _cb) = new_core();
    dictate(&core, "first take");
    dictate(&core, "second take");

    let history = core.recent_history(10).unwrap();
    // Most-recent first; pick the older entry to prove edit-by-id targets any row.
    let first = history.iter().find(|h| h.text == "first take").unwrap();
    core.history_edited(first.id, "first TAKE".to_owned())
        .unwrap();

    let after = core.recent_history(10).unwrap();
    assert!(after.iter().any(|h| h.text == "first TAKE"));
    assert!(after.iter().any(|h| h.text == "second take"));
    assert!(!after.iter().any(|h| h.text == "first take"));
}

#[test]
fn reinsert_history_types_the_stored_text_at_the_cursor() {
    let (core, cb) = new_core();
    dictate(&core, "paste me");
    let id = core.recent_history(10).unwrap()[0].id;

    core.reinsert_history(id).unwrap();
    assert!(cb.events().contains(&"insert_text:paste me".to_owned()));
}

#[test]
fn open_history_edit_requests_the_review_dialog() {
    let (core, cb) = new_core();
    dictate(&core, "review me");
    let id = core.recent_history(10).unwrap()[0].id;

    core.open_history_edit(id).unwrap();
    assert!(cb
        .events()
        .contains(&format!("edit_history:{id}:review me")));
}

#[test]
fn load_model_rejects_a_missing_file() {
    let (core, _cb) = new_core();
    assert!(core
        .load_model("/no/such/idiolect-model.bin".to_owned())
        .is_err());
}

/// SHA-256 of the committed `ggml-tiny.en.bin` fixture (`sha256sum`).
const FIXTURE_MODEL_SHA256: &str =
    "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f";

#[test]
fn load_model_verified_accepts_a_matching_digest() {
    let (core, _cb) = new_core();
    core.load_model_verified(fixture_model_path(), FIXTURE_MODEL_SHA256.to_owned())
        .expect("a model whose digest matches should load");
}

#[test]
fn load_model_verified_rejects_a_tampered_digest() {
    let (core, _cb) = new_core();
    let err = core
        .load_model_verified(fixture_model_path(), "0".repeat(64))
        .expect_err("a digest mismatch must be rejected, not loaded");
    assert!(
        matches!(err, FfiError::ModelIntegrity { .. }),
        "got {err:?}"
    );
}

#[test]
fn the_on_device_whisper_model_decodes_a_take_end_to_end() {
    // The real on-device path: load the committed fixture model through the FFI,
    // stream real speech, and assert the whole-take decode commits the words.
    let (core, cb) = new_core();
    core.load_model(fixture_model_path())
        .expect("fixture model should load");

    core.toggle().unwrap();
    push_audio(&core, &speech_and_silence_fixture_16khz_mono());
    core.toggle().unwrap();

    let events = cb.events();
    let committed = events
        .iter()
        .find(|e| e.starts_with("commit_text:"))
        .expect("a take should commit");
    let lowered = committed.to_lowercase();
    assert!(lowered.contains("restart"), "decoded: {committed}");
    assert!(lowered.contains("traffic"), "decoded: {committed}");

    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].committed);
}

#[test]
fn a_multi_snippet_take_accumulates_the_full_preedit_then_commits_it() {
    let (core, cb) = new_core();
    // Two pause-separated snippets, then the whole-take decode.
    core.install_transcriber(Box::new(ScriptedTranscriber::new([
        Ok("restart traffic".to_owned()),
        Ok("deploy nginx".to_owned()),
        Ok("restart traffic deploy nginx".to_owned()),
    ])));

    core.toggle().unwrap();
    push_audio(&core, &speech_pause_speech_fixture_16khz_mono());
    core.toggle().unwrap();

    let events = cb.events();
    // The preedit must carry the FULL accumulated text on the second snippet —
    // Android's setComposingText REPLACES the whole region, so a chunk-only push
    // would erase "restart traffic" and leave just "deploy nginx".
    let preedits: Vec<&String> = events
        .iter()
        .filter(|e| e.starts_with("show_preedit:") || e.starts_with("update_preedit:"))
        .collect();
    assert_eq!(
        preedits,
        [
            &"show_preedit:restart traffic".to_owned(),
            &"update_preedit:restart traffic deploy nginx".to_owned(),
        ],
        "the second snippet updates the full composing text: {events:?}"
    );
    assert!(events.contains(&"commit_text:restart traffic deploy nginx".to_owned()));
}

#[test]
fn a_stop_decode_failure_keeps_the_previewed_text_and_still_commits() {
    let (core, cb) = new_core();
    // Both snippets decode; the whole-take (finalize) decode fails → the glued
    // previews are kept and committed (the streaming-drops-words guardrail).
    core.install_transcriber(Box::new(ScriptedTranscriber::new([
        Ok("restart traffic".to_owned()),
        Ok("deploy nginx".to_owned()),
        Err(TranscribeFailure {
            code: "stop-decode".to_owned(),
            message: "stop decode failed".to_owned(),
        }),
    ])));

    core.toggle().unwrap();
    push_audio(&core, &speech_pause_speech_fixture_16khz_mono());
    core.toggle().unwrap();

    let events = cb.events();
    assert!(
        events.contains(&"commit_text:restart traffic deploy nginx".to_owned()),
        "the previewed text should still commit on a stop-decode failure: {events:?}"
    );
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart traffic deploy nginx");
    assert!(history[0].committed);
}

#[test]
fn report_correction_is_a_noop_without_a_commit_or_when_unchanged() {
    let (core, _cb) = new_core();
    // No take committed yet: nothing to amend, no error, no history row.
    core.report_correction("anything".to_owned()).unwrap();
    assert!(core.recent_history(10).unwrap().is_empty());

    // After a commit, re-reporting the SAME text changes nothing.
    dictate(&core, "restart traffic");
    core.report_correction("restart traffic".to_owned())
        .unwrap();
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart traffic");
}

#[test]
fn report_correction_replaces_the_whole_multi_snippet_take() {
    let (core, _cb) = new_core();
    core.install_transcriber(Box::new(ScriptedTranscriber::new([
        Ok("restart traffic".to_owned()),
        Ok("deploy nginx".to_owned()),
        Ok("restart traffic deploy nginx".to_owned()),
    ])));
    core.toggle().unwrap();
    push_audio(&core, &speech_pause_speech_fixture_16khz_mono());
    core.toggle().unwrap();

    // The Android contract: report_correction carries the WHOLE corrected take
    // (read back from the field), not a tail snippet — it replaces the take.
    core.report_correction("restart traffic deploy Nginx".to_owned())
        .unwrap();
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart traffic deploy Nginx");
}

#[test]
fn a_history_edit_of_the_active_take_keeps_a_later_correction_from_stale_skip() {
    let (core, _cb) = new_core();
    dictate(&core, "foo");
    let id = core.recent_history(10).unwrap()[0].id;

    // Edit the just-committed take via the review path to "bar"…
    core.history_edited(id, "bar".to_owned()).unwrap();
    // …then an in-field correction back to "foo". Because the edit refreshed
    // last_commit to "bar", this is seen as a real change and applies. With a stale
    // last_commit ("foo") it would be wrongly skipped as a no-op, leaving "bar".
    core.report_correction("foo".to_owned()).unwrap();

    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "foo");
}

/// With a history key supplied, the `ime_text_history` projection is encrypted at
/// rest: reopening the store *without* the key yields ciphertext (not the plaintext),
/// and reopening *with* it decrypts back. This is the Android analog of the desktop's
/// at-rest history encryption (the key comes from the Keystore on device).
#[test]
fn a_history_key_encrypts_the_projection_at_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().to_string_lossy().into_owned();
    let key = vec![7_u8; 32];

    {
        let core = IdiolectCore::new(data.clone(), Some(key.clone()), boxed_cb())
            .expect("core should open");
        dictate(&core, "secret words");
        assert_eq!(core.recent_history(10).unwrap()[0].text, "secret words");
    }
    // Reopen WITHOUT the key: the stored text is ciphertext, not the plaintext.
    {
        let core = IdiolectCore::new(data.clone(), None, boxed_cb()).expect("core should open");
        assert_ne!(core.recent_history(10).unwrap()[0].text, "secret words");
    }
    // Reopen WITH the key: it decrypts back to the plaintext.
    {
        let core = IdiolectCore::new(data, Some(key), boxed_cb()).expect("core should open");
        assert_eq!(core.recent_history(10).unwrap()[0].text, "secret words");
    }
}

#[test]
fn a_wrong_length_history_key_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().to_string_lossy().into_owned();
    // 16 bytes — not the required 32; must be a typed error, not a panic.
    assert!(IdiolectCore::new(data, Some(vec![0_u8; 16]), boxed_cb()).is_err());
}

#[test]
fn export_sync_batch_is_empty_when_the_outbox_is_empty() {
    let (core, _cb) = new_core();
    // Nothing captured yet: there is nothing to ship, so the pump gets an empty
    // batch (its cheap "skip the network" signal) rather than an encoded no-op.
    let bytes = core
        .export_sync_batch("pixel".to_owned(), "batch-1".to_owned())
        .unwrap();
    assert!(
        bytes.is_empty(),
        "an empty outbox exports no batch: {bytes:?}"
    );
}

#[test]
fn export_then_confirm_ships_a_corrected_take_and_reclaims_it() {
    let (core, _cb) = new_core();
    // A dictated take that the user then corrects is a raw→corrected learning.
    dictate(&core, "restart trafic");
    core.report_correction("restart traffic".to_owned())
        .unwrap();

    // Export encodes it as the on-the-wire sync container the phone POSTs.
    let bytes = core
        .export_sync_batch("pixel".to_owned(), "batch-1".to_owned())
        .unwrap();
    assert!(!bytes.is_empty(), "a corrected take should export");
    let envelope = decode_batch(&bytes).expect("export decodes to a batch");
    assert_eq!(envelope.batch.device_id, "pixel");
    assert_eq!(envelope.batch.batch_id, "batch-1");
    assert_eq!(envelope.batch.learnings.len(), 1);
    let learning = &envelope.batch.learnings[0];
    assert_eq!(learning.raw_transcript, "restart trafic");
    assert_eq!(learning.corrected_transcript, "restart traffic");
    // Its audio rides content-addressed by digest.
    assert!(
        envelope.audio.contains_key(&learning.audio_digest),
        "the learning's audio is carried by digest"
    );

    // Once the PC acks, confirming drains the outbox (delete-after-ACK): a second
    // export now finds nothing pending.
    core.confirm_synced(bytes).unwrap();
    let after = core
        .export_sync_batch("pixel".to_owned(), "batch-2".to_owned())
        .unwrap();
    assert!(
        after.is_empty(),
        "the outbox is drained after confirm: {after:?}"
    );
}

fn boxed_cb() -> Box<RecordingCallback> {
    Box::new(RecordingCallback::default())
}

/// Every `.ogg` source recording beneath `root` (recursively).
fn ogg_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "ogg") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found
}
