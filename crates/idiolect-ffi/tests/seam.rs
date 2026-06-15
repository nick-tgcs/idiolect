//! Exercises the mobile core **through the UniFFI seam**: every assertion goes
//! via the exported `IdiolectCore` surface and the `IdiolectInputMethod` callback
//! trait — the same objects the Kotlin app drives — against a real on-disk SQLite
//! store. This is the host-runnable proof of the M1 facade; the Kotlin
//! `InputMethodService` *rendering* is the declared GUI seam (M3), but all of its
//! state logic lives here.
//!
//! A "take" follows the desktop's authoritative-full-take shape: `toggle()` to
//! start the mic, `toggle()` again to stop, then the decode of the whole take is
//! delivered (`deliver_transcript`, which M2's streaming worker becomes the real
//! caller of), then `commit()` finalises it into the field.

use std::sync::{Arc, Mutex};

use idiolect_ffi::{IdiolectCore, IdiolectInputMethod};

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
}

/// Build a core over a throwaway data dir, returning the core plus a handle to the
/// callback's recorded event log.
fn new_core() -> (Arc<IdiolectCore>, RecordingCallback) {
    let dir = tempfile::tempdir().expect("tempdir");
    let callback = RecordingCallback::default();
    let core = IdiolectCore::new(
        dir.path().to_string_lossy().into_owned(),
        Box::new(callback.clone()),
    )
    .expect("core should open");
    // Keep the tempdir alive for the core's lifetime by leaking it: the test
    // process is short-lived and the OS reclaims it.
    std::mem::forget(dir);
    (core, callback)
}

/// Dictate one whole take end to end: start, stop, decode-delivers, commit.
fn dictate(core: &IdiolectCore, text: &str) {
    core.toggle().unwrap(); // mic on
    core.toggle().unwrap(); // mic off → the take is decoded
    core.deliver_transcript(text).unwrap();
    core.commit().unwrap();
}

#[test]
fn toggle_pushes_authoritative_recording_status_each_edge() {
    let (core, cb) = new_core();
    assert!(!core.is_recording());

    core.toggle().unwrap();
    assert!(core.is_recording());
    core.toggle().unwrap();
    assert!(!core.is_recording());

    // The UI never toggles optimistically — it mirrors these edge-triggered pushes.
    assert_eq!(
        cb.events(),
        ["recording_status:true", "recording_status:false"]
    );
}

#[test]
fn decoded_take_shows_preedit_and_commit_types_and_persists() {
    let (core, cb) = new_core();
    core.toggle().unwrap();
    core.toggle().unwrap();
    core.deliver_transcript("restart traffic").unwrap();
    core.commit().unwrap();

    assert_eq!(
        cb.events(),
        [
            "recording_status:true",
            "recording_status:false",
            "show_preedit:restart traffic",
            "commit_text:restart traffic",
        ]
    );
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text, "restart traffic");
    assert!(history[0].committed);
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
fn cancel_records_an_empty_take_and_does_not_commit() {
    let (core, cb) = new_core();
    core.toggle().unwrap();
    core.toggle().unwrap();
    core.deliver_transcript("open notes").unwrap();
    core.cancel().unwrap();

    assert_eq!(
        cb.events(),
        [
            "recording_status:true",
            "recording_status:false",
            "show_preedit:open notes",
            "cancel_preedit",
        ]
    );
    // The desktop records a cancelled take as an empty, non-committed history row;
    // the facade mirrors that rather than dropping the row.
    let history = core.recent_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert!(!history[0].committed);
    assert_eq!(history[0].text, "");
}

#[test]
fn pcm_frames_buffer_only_while_recording() {
    let (core, _cb) = new_core();
    // No active take: a stray frame is rejected rather than silently buffered.
    assert!(core.push_pcm_frame(vec![0; 160]).is_err());

    core.toggle().unwrap();
    core.push_pcm_frame(vec![0; 160]).unwrap();
    core.push_pcm_frame(vec![0; 320]).unwrap();
    assert_eq!(core.buffered_frame_count(), 480);

    // Stopping the take releases the buffer (M2's decode worker consumes it here).
    core.toggle().unwrap();
    assert_eq!(core.buffered_frame_count(), 0);
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
