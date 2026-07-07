//! IBus engine glue (zbus). Exposes `org.freedesktop.IBus.Factory` +
//! `org.freedesktop.IBus.Engine` on the IBus bus, drives the [`Session`] from
//! `ProcessKeyEvent`, and emits `CommitText`/`UpdatePreeditText` to type into
//! the focused app. Feature-gated (`ibus-engine`).
//!
//! The daemon connection is established ONCE at process startup and shared by
//! all engine instances. `CreateEngine` must be instant — doing a blocking
//! connect inside that async DBus handler makes ibus's `SetGlobalEngine` time
//! out (and the daemon serves one connection at a time, so per-instance
//! connects would also deadlock).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use idiolect_ipc::messages::{EditHistory, HistoryEdited};
use idiolect_ipc::IpcMessage;
use zbus::zvariant::{Array, Dict, OwnedObjectPath, Signature, StructureBuilder, Value};
use zbus::{interface, Connection};

use crate::ipc::{self, DaemonReader, DaemonSender};
use crate::session::{Key, Session, Surface};

const ENGINE_IFACE: &str = "org.freedesktop.IBus.Engine";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";
const TRIGGER_PATH: &str = "/org/idiolect/Trigger";
const BUS_NAME: &str = "org.freedesktop.IBus.Idiolect";

// IBus keyvals / modifier masks.
const KEY_ESCAPE: u32 = 0xff1b;
const KEY_BACKSPACE: u32 = 0xff08;
const KEY_DELETE: u32 = 0xffff;
const KEY_HOME: u32 = 0xff50;
const KEY_LEFT: u32 = 0xff51;
const KEY_RIGHT: u32 = 0xff53;
const KEY_END: u32 = 0xff57;
const RELEASE_MASK: u32 = 1 << 30;
const MOD4_MASK: u32 = 1 << 6; // Super on most layouts
const SUPER_MASK: u32 = 1 << 26;

/// Buffers the surface effects produced by a `Session` call so the async DBus
/// layer can emit them as IBus signals afterwards.
#[derive(Default)]
pub struct PendingSurface {
    ops: Vec<SurfaceOp>,
}

impl PendingSurface {
    fn take_ops(&mut self) -> Vec<SurfaceOp> {
        std::mem::take(&mut self.ops)
    }
}

enum SurfaceOp {
    Commit {
        text: String,
    },
    /// Replace the IME-owned preedit region (the underlined pre-commit preview)
    /// with `text`, emitted as an IBus `UpdatePreeditText`. An empty string clears
    /// it. The live streaming preview lives here — apps never auto-transform
    /// preedit — and only the verified full-take text is ever committed.
    Preedit {
        text: String,
    },
}

impl Surface for PendingSurface {
    fn commit_text(&mut self, text: &str) {
        self.ops.push(SurfaceOp::Commit {
            text: text.to_owned(),
        });
    }

    fn set_preedit(&mut self, text: &str) {
        self.ops.push(SurfaceOp::Preedit {
            text: text.to_owned(),
        });
    }
}

/// Process-wide shared state: the single daemon connection (via the session) and
/// the path of the currently focused engine instance (where signals are sent).
struct Shared {
    session: Mutex<Session<DaemonSender, PendingSurface>>,
    active_path: Mutex<Option<OwnedObjectPath>>,
    /// Review dialog used in "review before insert" mode. Doubles as the live
    /// mid-take surface (snippets stream into it as the user pauses). Behind a
    /// trait so the GUI toolkit is swappable; runs out-of-process so it never
    /// blocks the IME.
    dialog: Box<dyn crate::review::ReviewDialog>,
    /// "Voice is live" overlay shown next to the caret while recording.
    indicator: Box<dyn crate::indicator::RecordingIndicator>,
    /// Latest known caret position (screen pixels) from `set_cursor_location`,
    /// so the indicator can appear right where the user is dictating.
    caret: Mutex<(i32, i32)>,
    connection: Connection,
    /// Restores X11 focus to the app the user was dictating into before a direct
    /// (no-dialog) commit, mirroring what the review dialog already does — the WM
    /// may not have handed focus back yet after the take (indicator, focus churn),
    /// and a commit racing that transition lands nowhere.
    focus: Box<dyn crate::focus::WindowFocus>,
    /// The window that was focused when the current take started recording — the
    /// commit target to re-assert focus on (captured on `recording=true`).
    dictation_target: Mutex<Option<crate::focus::WindowId>>,
}

/// After re-asserting focus, give the WM + app a moment to process the focus-in
/// and re-establish their input context before the engine commits — otherwise the
/// commit races the focus hand-back (the same settle the review dialog uses).
const FOCUS_SETTLE: Duration = Duration::from_millis(120);

type SharedRef = Arc<Shared>;

impl Shared {
    fn set_active(&self, path: &OwnedObjectPath) {
        *self.active_path.lock().expect("active_path mutex") = Some(path.clone());
    }

    /// Forget the active target if it is this (now-gone) context, so a destroyed
    /// or disabled context can never be a stale commit target.
    fn clear_active_if(&self, path: &OwnedObjectPath) {
        let mut active = self.active_path.lock().expect("active_path mutex");
        if active.as_ref() == Some(path) {
            *active = None;
        }
    }

    /// Run a session operation and return the resulting surface ops (lock held
    /// only briefly; never nested with `active_path`).
    fn run_session<F: FnOnce(&mut Session<DaemonSender, PendingSurface>)>(
        &self,
        f: F,
    ) -> Vec<SurfaceOp> {
        let mut session = self.session.lock().expect("session mutex");
        f(&mut session);
        session.surface_mut().take_ops()
    }

    /// Show or hide the recording indicator to match the session state — call
    /// after any state-changing session operation.
    fn sync_indicator(&self) {
        let recording = matches!(
            self.session.lock().expect("session mutex").state(),
            crate::session::State::Recording
        );
        if recording {
            let (x, y) = *self.caret.lock().expect("caret mutex");
            self.indicator.show(x, y);
        } else {
            self.indicator.hide();
        }
    }
}

/// Classify a raw IBus key into the session's [`Key`]. Returns `None` for key
/// releases (ignored).
/// Live-trace of the engine event/correction path to `/tmp/idiolect-edit.log`
/// (ibus swallows the engine's stderr). Compiled in only with the `trace`
/// feature; a no-op otherwise, so production builds do no logging.
#[cfg(feature = "trace")]
pub(crate) fn dbg_edit(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/idiolect-edit.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

#[cfg(not(feature = "trace"))]
#[inline(always)]
pub(crate) fn dbg_edit(_msg: &str) {}

fn classify(keyval: u32, state: u32) -> Option<Key> {
    if state & RELEASE_MASK != 0 {
        return None;
    }
    let is_super = state & (MOD4_MASK | SUPER_MASK) != 0;
    if is_super && (keyval == 0x74 || keyval == 0x54) {
        return Some(Key::Trigger); // Super+T (normally grabbed by the compositor)
    }
    Some(match keyval {
        KEY_ESCAPE => Key::Cancel,
        KEY_BACKSPACE => Key::Backspace,
        KEY_DELETE => Key::Delete,
        KEY_LEFT => Key::Left,
        KEY_RIGHT => Key::Right,
        KEY_HOME => Key::Home,
        KEY_END => Key::End,
        // Printable ASCII and Unicode keyvals become characters so the
        // post-commit correction window can track in-place edits.
        0x20..=0x7e => Key::Char(keyval as u8 as char),
        kv if kv >= 0x0100_0000 => char::from_u32(kv - 0x0100_0000)
            .map(Key::Char)
            .unwrap_or(Key::Passthrough),
        _ => Key::Passthrough,
    })
}

/// Build an `IBusText` D-Bus value: `(sa{sv}sv)` = name, attachments, text,
/// attribute-list. The nested `IBusAttrList` is `(sa{sv}av)` with no attributes.
fn sv_dict() -> Value<'static> {
    Value::from(Dict::new(&Signature::Str, &Signature::Variant))
}

fn ibus_text(text: &str) -> Value<'static> {
    let attr_list = StructureBuilder::new()
        .append_field(Value::from("IBusAttrList"))
        .append_field(sv_dict())
        .append_field(Value::from(Array::new(&Signature::Variant)))
        .build()
        .expect("IBusAttrList structure is well-formed");
    let ibus_text = StructureBuilder::new()
        .append_field(Value::from("IBusText"))
        .append_field(sv_dict())
        .append_field(Value::from(text.to_owned()))
        .append_field(Value::from(attr_list))
        .build()
        .expect("IBusText structure is well-formed");
    Value::from(ibus_text)
}

/// Pull the text out of an inbound IBusText value (the `(sa{sv}sv)` structure,
/// possibly wrapped in a variant). Returns empty string if the shape is
/// unexpected.
fn ibus_text_str(value: &Value<'_>) -> String {
    match value {
        Value::Value(inner) => ibus_text_str(inner),
        Value::Structure(s) => match s.fields().get(2) {
            Some(Value::Str(s)) => s.as_str().to_owned(),
            _ => String::new(),
        },
        Value::Str(s) => s.as_str().to_owned(),
        _ => String::new(),
    }
}

async fn emit_surface_ops(conn: &Connection, engine_path: &OwnedObjectPath, ops: Vec<SurfaceOp>) {
    for op in ops {
        match op {
            SurfaceOp::Commit { text } => {
                dbg_edit(&format!(
                    "emit CommitText -> {} : {:?}",
                    engine_path.as_str(),
                    text
                ));
                let result = conn
                    .emit_signal(
                        None::<&str>,
                        engine_path,
                        ENGINE_IFACE,
                        "CommitText",
                        &(ibus_text(&text),),
                    )
                    .await;
                if let Err(error) = result {
                    eprintln!("idiolect-ibus: failed to emit CommitText: {error}");
                }
            }
            SurfaceOp::Preedit { text } => {
                // `UpdatePreeditText(variant text, uint cursor, bool visible)`.
                // Non-empty shows the underlined preview with the caret at its end;
                // empty hides/clears it. Preedit is IME-owned, so the app renders
                // the preview without ever committing or auto-transforming it.
                let cursor = text.chars().count() as u32;
                let visible = !text.is_empty();
                dbg_edit(&format!(
                    "emit UpdatePreeditText -> {} : {:?} (visible={visible})",
                    engine_path.as_str(),
                    text
                ));
                let result = conn
                    .emit_signal(
                        None::<&str>,
                        engine_path,
                        ENGINE_IFACE,
                        "UpdatePreeditText",
                        &(ibus_text(&text), cursor, visible),
                    )
                    .await;
                if let Err(error) = result {
                    eprintln!("idiolect-ibus: failed to emit UpdatePreeditText: {error}");
                }
            }
        }
    }
}

/// A per-input-context engine object. All instances share one daemon connection.
pub struct IbusEngine {
    shared: SharedRef,
    path: OwnedObjectPath,
}

#[interface(name = "org.freedesktop.IBus.Engine")]
impl IbusEngine {
    async fn process_key_event(&self, keyval: u32, _keycode: u32, state: u32) -> bool {
        self.shared.set_active(&self.path);
        let Some(key) = classify(keyval, state) else {
            return false;
        };
        let mut consumed = false;
        let mut after = crate::session::State::Idle;
        let ops = self.shared.run_session(|session| {
            consumed = session.on_key(key);
            after = session.state();
        });
        self.shared.sync_indicator();
        dbg_edit(&format!("key {key:?} state_after={after:?}"));
        emit_surface_ops(&self.shared.connection, &self.path, ops).await;
        consumed
    }

    async fn focus_in(&self) {
        dbg_edit(&format!("focus_in {}", self.path.as_str()));
        self.shared.set_active(&self.path);
        self.require_surrounding_text().await;
    }
    async fn enable(&self) {
        dbg_edit(&format!("enable {}", self.path.as_str()));
        self.shared.set_active(&self.path);
        self.require_surrounding_text().await;
    }

    async fn focus_out(&self) {
        dbg_edit(&format!("focus_out {}", self.path.as_str()));
        // Leaving the input context ends post-commit correction tracking: the
        // session closes its window and reports any in-place fix to the daemon.
        // Without this call a correction finished by clicking elsewhere was
        // silently lost (the session logic existed but was never driven).
        let ops = self.shared.run_session(|session| session.on_focus_out());
        emit_surface_ops(&self.shared.connection, &self.path, ops).await;
    }
    async fn reset(&self) {
        dbg_edit("reset");
        // IBus resets the context when the app moves the cursor out from under
        // us (mouse click, programmatic change) — edits can no longer be
        // modelled, so close the correction window, reporting what was tracked.
        let ops = self.shared.run_session(|session| session.on_focus_out());
        emit_surface_ops(&self.shared.connection, &self.path, ops).await;
    }
    async fn disable(&self) {
        dbg_edit("disable");
        self.shared.clear_active_if(&self.path);
    }
    async fn destroy(&self) {
        dbg_edit("destroy");
        self.shared.clear_active_if(&self.path);
    }

    async fn set_capabilities(&self, caps: u32) {
        // IBUS_CAP_SURROUNDING_TEXT = 1 << 5. If the app supports surrounding
        // text we can read its real content (capturing mouse/selection edits).
        let surrounding = caps & (1 << 5) != 0;
        dbg_edit(&format!(
            "set_capabilities caps={caps:#x} surrounding_text={surrounding}"
        ));
        if surrounding {
            self.require_surrounding_text().await;
        }
    }

    /// The app reports the text around the cursor (and selection anchor). This
    /// is ground truth for what the user actually edited, regardless of how.
    async fn set_surrounding_text(
        &self,
        text: zbus::zvariant::Value<'_>,
        cursor_index: u32,
        anchor_pos: u32,
    ) {
        let body = ibus_text_str(&text);
        dbg_edit(&format!(
            "set_surrounding_text cursor={cursor_index} anchor={anchor_pos} text={body:?}"
        ));
    }

    async fn set_cursor_location(&self, x: i32, y: i32, w: i32, h: i32) {
        // Apps (notably Electron) interleave spurious 0-height reports (often
        // 0,0) with the real caret rect; only trust those with a real line
        // height, and anchor on the caret's vertical centre.
        if h > 0 {
            *self.shared.caret.lock().expect("caret mutex") = (x, y + h / 2);
            // While recording, stream the moved caret so the indicator follows.
            self.shared.sync_indicator();
        }
        dbg_edit(&format!("set_cursor_location x={x} y={y} w={w} h={h}"));
    }
}

impl IbusEngine {
    /// Ask the application to start sending us surrounding-text updates.
    async fn require_surrounding_text(&self) {
        let result = self
            .shared
            .connection
            .emit_signal(
                None::<&str>,
                &self.path,
                ENGINE_IFACE,
                "RequireSurroundingText",
                &(),
            )
            .await;
        if let Err(error) = result {
            dbg_edit(&format!("require_surrounding_text emit failed: {error}"));
        }
    }
}

/// The factory object IBus calls to create engine instances. Creation is instant
/// (no I/O) — it just registers an object sharing the process-wide connection.
pub struct IbusFactory {
    shared: SharedRef,
    next_id: Mutex<u32>,
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl IbusFactory {
    async fn create_engine(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        _engine_name: &str,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let id = {
            let mut guard = self.next_id.lock().expect("id mutex");
            *guard += 1;
            *guard
        };
        let path = OwnedObjectPath::try_from(format!("/org/freedesktop/IBus/Engine/idiolect/{id}"))
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let engine = IbusEngine {
            shared: Arc::clone(&self.shared),
            path: path.clone(),
        };
        server.at(&path, engine).await?;
        Ok(path)
    }
}

/// Toggle endpoint a GNOME global shortcut (Super+T) calls. The compositor
/// grabs the Super key before any IME can see it, so the trigger can't be a
/// key the engine receives — it comes in here over DBus instead, and acts on
/// the currently-focused engine exactly as an in-engine trigger would.
pub struct Trigger {
    shared: SharedRef,
}

#[interface(name = "org.idiolect.Trigger1")]
impl Trigger {
    async fn toggle(&self) {
        let mut after = crate::session::State::Idle;
        let ops = self.shared.run_session(|session| {
            session.on_key(Key::Trigger);
            after = session.state();
        });
        self.shared.sync_indicator();
        dbg_edit(&format!("toggle (Super+T) state_after={after:?}"));
        let target = self
            .shared
            .active_path
            .lock()
            .expect("active_path mutex")
            .clone();
        match target {
            Some(path) => emit_surface_ops(&self.shared.connection, &path, ops).await,
            None if !ops.is_empty() => dbg_edit(&format!(
                "toggle emit DROPPED: active_path None — {} op(s) NOT typed",
                ops.len()
            )),
            None => {}
        }
    }
}

/// Daemon read loop (one per process): delivers transcripts/errors into the
/// shared session and emits the resulting preedit/commit on the *currently
/// focused* engine object.
fn spawn_reader(shared: SharedRef, mut reader: DaemonReader, mut sender: DaemonSender) {
    let handle = tokio::runtime::Handle::current();
    let socket = ipc::default_socket_path();
    tokio::task::spawn_blocking(move || loop {
        let ops = match reader.read_message() {
            Ok(IpcMessage::PreeditUpdate(update)) => {
                dbg_edit(&format!(
                    "transcript <- daemon: {:?} (review={} partial={} reconcile={})",
                    update.text, update.review, update.partial, update.reconcile
                ));
                if update.partial && update.review {
                    // A display-only snippet of a review-mode take: stream it
                    // into the review dialog (opening it, in its listening
                    // state, on the first snippet) so the user watches the
                    // take grow in the same window they will edit at stop;
                    // nothing touches the document.
                    shared.dialog.append(&update.text);
                    continue;
                }
                if update.partial {
                    // A mid-take snippet of a streamed take: show it in the
                    // IME-owned preedit and keep recording. The daemon finalizes the
                    // whole take at stop. Re-assert focus on the dictation target
                    // first (same dance as the final direct commit) so the preedit
                    // shows on the app the user is dictating into and not wherever
                    // the WM's focus churn left things — streaming dictation hits
                    // THIS arm, not the batch one below.
                    let target = *shared
                        .dictation_target
                        .lock()
                        .expect("dictation_target mutex");
                    restore_dictation_focus(shared.focus.as_ref(), target);
                    shared.run_session(|s| s.on_partial_transcript(update.text))
                } else if update.review {
                    // Review mode: the take is over — the listening dialog
                    // turns editable with the full merged text (blocking —
                    // fine on this dedicated reader thread), then commit the
                    // user's final text into the app and record raw→edited,
                    // or cancel.
                    match shared.dialog.review(&update.text) {
                        Some(edited) => {
                            dbg_edit(&format!("dialog -> insert {edited:?}"));
                            shared.run_session(|s| s.commit_reviewed(&edited))
                        }
                        None => {
                            dbg_edit("dialog -> cancelled");
                            shared.run_session(|s| s.cancel_reviewed())
                        }
                    }
                } else if shared
                    .active_path
                    .lock()
                    .expect("active_path mutex")
                    .is_some()
                {
                    // Direct (review-off) take with a focused context to type into.
                    // Re-assert focus on the window the user started dictating in and
                    // let it settle BEFORE committing — after a take the WM may not
                    // have handed focus back yet (indicator/focus churn), and a commit
                    // racing that transition lands nowhere. This is the focus dance the
                    // review dialog already does; the direct path was missing it.
                    let target = *shared
                        .dictation_target
                        .lock()
                        .expect("dictation_target mutex");
                    restore_dictation_focus(shared.focus.as_ref(), target);
                    if update.reconcile {
                        // Stop of a direct STREAMING take: clear the IME-owned
                        // preedit preview and commit the verified full-take text
                        // once (no backspace guessing — the preview was never in the
                        // document). The daemon already owns and committed the
                        // streamed session, so the engine's commit is display only.
                        shared.run_session(|s| s.on_reconcile(update.text))
                    } else {
                        shared.run_session(|s| s.on_transcript(update.text))
                    }
                } else {
                    // Direct take but NO focused context: typing would lose the
                    // text into nowhere AND the daemon would bank a never-landed
                    // training pair. Discard it instead (and trace it loudly).
                    dbg_edit("direct transcript with no active_path: discarding take (not typed, not recorded)");
                    shared.run_session(|s| s.on_transcript_without_target())
                }
            }
            Ok(IpcMessage::InsertText(insert)) => {
                // History re-insert: commit the stored text straight into the
                // focused app at the cursor, bypassing the dictation session.
                dbg_edit(&format!("insert_text <- daemon: {:?}", insert.text));
                vec![SurfaceOp::Commit { text: insert.text }]
            }
            Ok(IpcMessage::RecordingStatus(status)) => {
                // The daemon is the single authority for recording state; mirror it
                // instead of tracking our own. Drives the "voice is live" indicator
                // (refreshed by sync_indicator below).
                dbg_edit(&format!("recording_status <- daemon: {}", status.recording));
                if status.recording {
                    // Capture the window the user is dictating into NOW, before the
                    // take can disrupt focus, so a direct commit can re-assert it.
                    let target = shared.focus.active_window();
                    dbg_edit(&format!("dictation target captured: {target:?}"));
                    *shared
                        .dictation_target
                        .lock()
                        .expect("dictation_target mutex") = target;
                } else {
                    // A take that ends WITH a final transcript already consumed
                    // its dialog in review() above; this only tears down a
                    // still-listening dialog (cancelled take), where it is the
                    // engine's only signal.
                    shared.dialog.close();
                }
                shared.run_session(|s| s.on_recording_status(status.recording))
            }
            Ok(IpcMessage::Error(_)) => {
                shared.dialog.close();
                shared.run_session(|s| s.on_error())
            }
            Ok(IpcMessage::EditHistory(edit)) => {
                // Retroactive history edit: run the review dialog over the stored
                // take and report the user's correction. Only the record changes —
                // nothing is typed into the app — so this never touches the session
                // or emits surface ops; it always `continue`s.
                dbg_edit(&format!(
                    "edit_history <- daemon: id={} text={:?}",
                    edit.id, edit.text
                ));
                handle_edit_history(&*shared.dialog, &mut sender, edit);
                continue;
            }
            Ok(_) => continue,
            Err(_) => {
                // The daemon connection dropped — e.g. the daemon restarted. Rather
                // than going permanently deaf (the old bug: a dead socket left the
                // engine stuck), reconnect and resync from the daemon's authoritative
                // state push. We retry until the daemon returns, so even a long outage
                // (an update, a slow restart) self-heals.
                reader = reconnect_reader(&socket, &sender);
                shared.dialog.close();
                shared.run_session(|s| s.reset_to_idle());
                shared.sync_indicator();
                continue;
            }
        };
        shared.sync_indicator();
        let target = shared
            .active_path
            .lock()
            .expect("active_path mutex")
            .clone();
        match target {
            Some(path) => handle.block_on(emit_surface_ops(&shared.connection, &path, ops)),
            // No focused IBus context to type into: the transcript is still
            // recorded daemon-side but lands in no app. Trace it — this otherwise
            // silent drop is exactly the "text in history but not in the app" bug.
            None if !ops.is_empty() => dbg_edit(&format!(
                "emit DROPPED: active_path None — {} commit op(s) NOT typed into any app",
                ops.len()
            )),
            None => {}
        }
    });
}

/// Re-assert X11 focus on the window the user was dictating into (if one was
/// captured at record-start) and let it settle, so a direct commit lands there
/// instead of racing the WM's focus hand-back after a take. Returns whether a
/// restore was performed. The same focus dance the review dialog does before it
/// commits — the direct path was missing it, so direct commits could vanish while
/// review-mode commits (which restore + settle) landed fine.
fn restore_dictation_focus(
    focus: &dyn crate::focus::WindowFocus,
    target: Option<crate::focus::WindowId>,
) -> bool {
    let Some(window) = target else { return false };
    dbg_edit(&format!(
        "direct commit: restoring focus to window {window}"
    ));
    focus.restore(window);
    std::thread::sleep(FOCUS_SETTLE);
    true
}

/// Run the review dialog over a stored history entry's text and report the
/// user's correction back to the daemon as `HistoryEdited`; a cancelled dialog
/// reports nothing. Updates only the record — it types nothing into the focused
/// app — so it runs outside the session/surface path.
fn handle_edit_history(
    dialog: &dyn crate::review::ReviewDialog,
    sender: &mut DaemonSender,
    edit: EditHistory,
) {
    match dialog.review(&edit.text) {
        Some(corrected) => {
            dbg_edit(&format!("edit_history -> confirmed id={}", edit.id));
            let _ = sender.send(&IpcMessage::HistoryEdited(HistoryEdited {
                id: edit.id,
                corrected_text: corrected,
            }));
        }
        None => dbg_edit(&format!("edit_history -> cancelled id={}", edit.id)),
    }
}

/// Re-establish the daemon connection after it drops, retrying indefinitely while
/// the daemon is down (a restart, an update). Swaps the live socket inside the
/// shared `sender` so the session keeps working. The reader thread is dedicated, so
/// polling here is harmless; never giving up means the engine always recovers when
/// the daemon returns instead of going permanently deaf.
fn reconnect_reader(socket: &Path, sender: &DaemonSender) -> DaemonReader {
    loop {
        match ipc::reconnect(socket, sender) {
            Ok(reader) => return reader,
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Connect to the daemon, retrying briefly so engine startup tolerates the
/// daemon coming up at roughly the same time.
fn connect_daemon() -> Result<(DaemonSender, DaemonReader), std::io::Error> {
    let socket = ipc::default_socket_path();
    let mut last = None;
    for _ in 0..50 {
        match ipc::connect(&socket) {
            Ok(pair) => return Ok(pair),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("daemon connect failed")))
}

/// Resolve the IBus bus address. ibus does NOT pass `IBUS_ADDRESS` to spawned
/// engines, so when it is unset we read the address ibus-daemon wrote to
/// `$XDG_CONFIG_HOME/ibus/bus/<machine-id>-unix-<display>`. Falls back to the
/// session bus (used by the headless tests, which set `IBUS_ADDRESS` explicitly).
fn resolve_ibus_address() -> Option<String> {
    if let Ok(address) = std::env::var("IBUS_ADDRESS") {
        if !address.is_empty() {
            return Some(address);
        }
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let dir = config_home.join("ibus").join("bus");

    // Prefer the address file matching the current X display (e.g. ":1" ->
    // "unix-1"); otherwise take the most recently written file.
    let want_suffix = std::env::var("DISPLAY").ok().and_then(|display| {
        let num = display
            .trim_start_matches(':')
            .split('.')
            .next()?
            .to_owned();
        Some(format!("unix-{num}"))
    });
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    files.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    });
    files.reverse(); // newest first
    if let Some(suffix) = &want_suffix {
        files.sort_by_key(|path| {
            let matches = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(suffix.as_str()));
            !matches // display-matched files first (false sorts before true)
        });
    }
    for path in files {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            for line in contents.lines() {
                if let Some(address) = line.strip_prefix("IBUS_ADDRESS=") {
                    return Some(address.trim().to_owned());
                }
            }
        }
    }
    None
}

/// Connect to the IBus bus (or the session bus for tests), serve the factory,
/// and run until killed.
pub async fn run() -> zbus::Result<()> {
    // Establish the single daemon connection up front (off the DBus handlers).
    let (sender, reader) = connect_daemon()
        .map_err(|error| zbus::Error::Failure(format!("daemon connect: {error}")))?;
    // A second handle on the same shared socket so the read loop can swap it on
    // reconnect without disturbing the session that owns the sender.
    let reader_sender = sender.clone();

    let builder = match resolve_ibus_address() {
        Some(address) => zbus::connection::Builder::address(address.as_str())?,
        None => zbus::connection::Builder::session()?,
    };
    let connection = builder.build().await?;

    let shared = Arc::new(Shared {
        session: Mutex::new(Session::new(sender, PendingSurface::default())),
        active_path: Mutex::new(None),
        dialog: Box::new(crate::review::SubprocessReviewDialog::discover()),
        indicator: Box::new(crate::indicator::SubprocessIndicator::discover()),
        caret: Mutex::new((400, 400)),
        connection: connection.clone(),
        focus: crate::focus::default_window_focus(),
        dictation_target: Mutex::new(None),
    });
    spawn_reader(Arc::clone(&shared), reader, reader_sender);

    connection
        .object_server()
        .at(
            TRIGGER_PATH,
            Trigger {
                shared: Arc::clone(&shared),
            },
        )
        .await?;
    connection
        .object_server()
        .at(
            FACTORY_PATH,
            IbusFactory {
                shared,
                next_id: Mutex::new(0),
            },
        )
        .await?;
    connection.request_name(BUS_NAME).await?;

    std::future::pending::<()>().await;
    Ok(())
}

#[cfg(all(test, feature = "ibus-engine"))]
mod tests {
    //! Tests for the retroactive history-edit arm: the engine runs the review
    //! dialog over the daemon's stored text and reports the user's correction
    //! back, typing nothing. Driven through `handle_edit_history` over a bare
    //! socket pair — no runtime, no D-Bus, no live reader loop.

    use std::io::{BufRead, BufReader, ErrorKind, Read};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use idiolect_ipc::framing::decode_json_line;
    use idiolect_ipc::messages::EditHistory;
    use idiolect_ipc::IpcMessage;

    use std::sync::Mutex as StdMutex;

    use crate::focus::{WindowFocus, WindowId};
    use crate::ipc::DaemonSender;
    use crate::review::ReviewDialog;

    use super::{handle_edit_history, restore_dictation_focus, PendingSurface, Surface, SurfaceOp};

    #[test]
    fn pending_surface_records_preview_then_clear_then_commit_in_order() {
        // The streaming reconcile path drives the surface as set_preedit(preview)
        // while recording, then set_preedit("") + commit(verified) at stop. The
        // buffered ops must preserve that order so the async layer clears the
        // underlined preview BEFORE committing the verified text (clear-then-commit).
        let mut surface = PendingSurface::default();
        surface.set_preedit("helo world");
        surface.set_preedit("");
        surface.commit_text("hello world");
        let ops = surface.take_ops();
        assert!(
            matches!(
                ops.as_slice(),
                [
                    SurfaceOp::Preedit { text: preview },
                    SurfaceOp::Preedit { text: cleared },
                    SurfaceOp::Commit { text: verified },
                ] if preview == "helo world" && cleared.is_empty() && verified == "hello world"
            ),
            "expected Preedit(preview) -> Preedit(\"\") -> Commit(verified), got a different op sequence"
        );
    }

    /// Records the windows focus was re-asserted on, so we can assert the direct
    /// commit hands focus back to where the user was dictating.
    struct RecordingFocus {
        restored: StdMutex<Vec<WindowId>>,
    }
    impl WindowFocus for RecordingFocus {
        fn active_window(&self) -> Option<WindowId> {
            None
        }
        fn restore(&self, window: WindowId) {
            self.restored.lock().expect("restored mutex").push(window);
        }
    }

    #[test]
    fn direct_commit_restores_focus_to_the_captured_window() {
        let focus = RecordingFocus {
            restored: StdMutex::new(Vec::new()),
        };
        assert!(restore_dictation_focus(&focus, Some(42)));
        assert_eq!(*focus.restored.lock().expect("restored mutex"), vec![42]);
    }

    #[test]
    fn direct_commit_without_a_captured_window_does_not_restore() {
        let focus = RecordingFocus {
            restored: StdMutex::new(Vec::new()),
        };
        assert!(!restore_dictation_focus(&focus, None));
        assert!(focus.restored.lock().expect("restored mutex").is_empty());
    }

    /// A dialog with a fixed verdict: `Some(text)` confirms with that text,
    /// `None` cancels.
    struct FakeDialog {
        reply: Option<String>,
    }
    impl ReviewDialog for FakeDialog {
        fn append(&self, _chunk: &str) {}
        fn review(&self, _transcript: &str) -> Option<String> {
            self.reply.clone()
        }
        fn close(&self) {}
    }

    #[test]
    fn confirmed_edit_reports_history_edited_with_the_corrected_text() {
        let (engine_side, daemon_side) = UnixStream::pair().expect("socketpair");
        let mut sender = DaemonSender::from_stream(daemon_side);
        let dialog = FakeDialog {
            reply: Some("restart Traefik".to_owned()),
        };

        handle_edit_history(
            &dialog,
            &mut sender,
            EditHistory {
                id: 42,
                text: "restart traffic".to_owned(),
            },
        );
        // Close the write end so the read sees EOF after the one message.
        drop(sender);

        let mut line = String::new();
        BufReader::new(engine_side)
            .read_line(&mut line)
            .expect("read");
        match decode_json_line(&line).expect("decode") {
            IpcMessage::HistoryEdited(edited) => {
                assert_eq!(edited.id, 42);
                assert_eq!(edited.corrected_text, "restart Traefik");
            }
            other => panic!("expected HistoryEdited, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_edit_reports_nothing() {
        let (engine_side, daemon_side) = UnixStream::pair().expect("socketpair");
        let mut sender = DaemonSender::from_stream(daemon_side);
        let dialog = FakeDialog { reply: None };

        handle_edit_history(
            &dialog,
            &mut sender,
            EditHistory {
                id: 7,
                text: "original".to_owned(),
            },
        );
        drop(sender);

        // A cancelled review must type nothing back: a short read finds no bytes.
        engine_side
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("timeout");
        let mut buf = [0u8; 1];
        match (&engine_side).read(&mut buf) {
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Ok(0) => {}
            other => panic!("expected no data for a cancelled edit, got {other:?}"),
        }
    }
}
