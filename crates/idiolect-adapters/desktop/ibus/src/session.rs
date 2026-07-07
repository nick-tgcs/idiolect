//! Pure dictation state machine for the IBus engine.
//!
//! Knows nothing about IBus, DBus, sockets, or audio. Toggle model: a trigger
//! starts recording; the next trigger stops it. A batch take is committed
//! straight into the focused app at stop (no Enter step). A streamed take shows
//! its live preview in the IME-owned **preedit** as it grows (never committed, so
//! no app can auto-transform it), and at stop the engine clears the preedit and
//! commits the single verified full-take text — see
//! [`on_partial_transcript`](Session::on_partial_transcript) and
//! [`on_reconcile`](Session::on_reconcile). Immediately after that commit the
//! engine opens a *correction window* and models the user's keyboard edits
//! (cursor movement, insert, delete) against a shadow buffer, so a fix made
//! anywhere in the just-dictated text is reported to the daemon as a
//! raw→corrected training signal. Unit-testable with fakes.
//!
//! Limitation: edits are reconstructed from keystrokes, so mouse-click cursor
//! repositioning (which sends no key event) is not modeled. The window only
//! reports when the user actually changed existing dictated text — pure forward
//! typing is treated as continuation, not a correction.

/// The idiolect daemon, as the session needs it. The daemon owns the microphone
/// and is the single authority for recording state, so the session never decides
/// start-vs-stop: it sends one direction-free [`toggle`](DaemonClient::toggle)
/// intent and learns the resulting state from the daemon's pushed `RecordingStatus`
/// (delivered into [`Session::on_recording_status`]). `commit` finalizes the take
/// into a training candidate; `report_correction` amends it when the user fixes the
/// text in place right after it lands.
pub trait DaemonClient {
    /// "The user pressed the toggle key." The daemon decides whether this starts or
    /// stops a recording and announces the result via `RecordingStatus`.
    fn toggle(&mut self);
    /// Finalize the session with the committed text (the daemon records it).
    fn commit(&mut self, final_text: &str);
    /// Amend the just-committed session with the user's in-place correction.
    fn report_correction(&mut self, corrected_text: &str);
    fn cancel(&mut self);
}

/// The text destination — in production an IBus input context. `commit_text`
/// types the verified text into the focused application. `set_preedit` shows the
/// live streaming preview as IME-owned **preedit** (the underlined pre-commit
/// region); the whole region is replaced each call and an empty string clears it.
///
/// The preview lives in preedit, not committed text, on purpose: preedit belongs
/// to the input method, so applications never auto-transform it the way they
/// mangle committed text (bracket auto-close, autocompletion, smart quotes). At
/// stop the engine clears the preedit and commits the verified full-take text
/// exactly once — so there is no live-typed preview in the document to reconcile
/// with backspaces, and nothing an auto-closing editor can corrupt. idiolect owns
/// the input; the preview is ours until we commit.
pub trait Surface {
    fn commit_text(&mut self, text: &str);
    fn set_preedit(&mut self, text: &str);
}

/// A key, already classified from the raw IBus keyval/state by the engine layer,
/// or delivered via the DBus toggle endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    /// The dictation toggle (default Super+T, delivered via the toggle endpoint
    /// because the compositor grabs the Super key).
    Trigger,
    /// Escape — abort an in-progress take / end the correction window.
    Cancel,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    /// A printable character.
    Char(char),
    /// Anything else (Enter, Tab, Up/Down, …) — passes through and closes any
    /// open correction window.
    Passthrough,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Not recording. `Idle` and `Recording` mirror the daemon's authoritative
    /// recording state — the session never sets them optimistically; they change
    /// only when [`Session::on_recording_status`] arrives.
    Idle,
    /// The daemon reports the microphone is open.
    Recording,
    /// Just committed a transcript; modeling the user's in-place edits. This is a
    /// purely local overlay — only the IME sees the keystrokes — so a stop's
    /// `recording = false` does not close it.
    Reviewing,
}

/// The post-commit correction buffer: a shadow copy of the dictated line plus a
/// cursor, mirroring the user's keyboard edits. `corrected` becomes true only
/// when existing dictated text is actually changed (a delete, or an insert that
/// is not a pure append at the end) — distinguishing a fix from just typing on.
struct Review {
    committed: String,
    chars: Vec<char>,
    cursor: usize,
    corrected: bool,
}

impl Review {
    fn new(text: String) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Self {
            committed: text,
            chars,
            cursor,
            corrected: false,
        }
    }

    fn text(&self) -> String {
        self.chars.iter().collect()
    }
}

pub struct Session<D, S> {
    daemon: D,
    surface: S,
    state: State,
    review: Option<Review>,
    /// The daemon's authoritative mic state, mirrored independently of `state`
    /// (which may be `Reviewing` while the mic is still open in streaming mode).
    recording: bool,
    /// The cumulative live preview this take: the concatenation of every streamed
    /// partial, held in the IME-owned **preedit** (never committed to the app). At
    /// stop [`Session::on_reconcile`] clears the preedit and commits the verified
    /// full-take text once; abandoning the take (cancel/error/silent-stop/reset)
    /// clears it via [`Session::clear_preview`]. Empty means no preview is showing.
    preview: String,
}

impl<D, S> Session<D, S>
where
    D: DaemonClient,
    S: Surface,
{
    pub fn new(daemon: D, surface: S) -> Self {
        Self {
            daemon,
            surface,
            state: State::Idle,
            review: None,
            recording: false,
            preview: String::new(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Mutable access to the surface (the engine drains buffered commit effects
    /// through this after each session call).
    pub fn surface_mut(&mut self) -> &mut S {
        &mut self.surface
    }

    /// Handle a classified key. Returns `true` if consumed (the IBus layer
    /// swallows it), `false` to pass it through to the application.
    pub fn on_key(&mut self, key: Key) -> bool {
        match self.state {
            // In both Idle and Recording a trigger is the same direction-free intent:
            // tell the daemon, and let its `RecordingStatus` push move our state. We
            // never flip the recording phase optimistically, so we can never disagree
            // with the daemon.
            State::Idle => match key {
                Key::Trigger => {
                    self.daemon.toggle();
                    true
                }
                _ => false, // transparent passthrough — normal typing is unaffected
            },
            State::Recording => match key {
                Key::Trigger => {
                    self.daemon.toggle();
                    true
                }
                Key::Cancel => {
                    self.daemon.cancel();
                    // Snappy local feedback; the daemon's `recording = false` push
                    // reconciles to the same state. A discarded take must drop its
                    // live preedit preview (it was never committed).
                    self.clear_preview();
                    self.state = State::Idle;
                    true
                }
                _ => false,
            },
            State::Reviewing => self.on_key_reviewing(key),
        }
    }

    fn on_key_reviewing(&mut self, key: Key) -> bool {
        let review = self.review.as_mut().expect("review buffer in Reviewing");
        match key {
            // Cursor movement: mirror the app so inserts/deletes land correctly.
            Key::Left => {
                review.cursor = review.cursor.saturating_sub(1);
                false
            }
            Key::Right => {
                if review.cursor < review.chars.len() {
                    review.cursor += 1;
                }
                false
            }
            Key::Home => {
                review.cursor = 0;
                false
            }
            Key::End => {
                review.cursor = review.chars.len();
                false
            }
            Key::Backspace => {
                if review.cursor > 0 {
                    review.cursor -= 1;
                    review.chars.remove(review.cursor);
                    review.corrected = true;
                }
                false
            }
            Key::Delete => {
                if review.cursor < review.chars.len() {
                    review.chars.remove(review.cursor);
                    review.corrected = true;
                }
                false
            }
            Key::Char(c) => {
                if review.cursor < review.chars.len() {
                    // Insert into existing text — a real edit.
                    review.chars.insert(review.cursor, c);
                    review.cursor += 1;
                    review.corrected = true;
                    false
                } else if review.corrected {
                    // Retyping at the end during an ongoing fix.
                    review.chars.push(c);
                    review.cursor += 1;
                    false
                } else {
                    // Pure forward typing with no edit yet: continuation, not a
                    // correction. Close the window and let it pass through.
                    self.end_review();
                    false
                }
            }
            Key::Trigger => {
                // Close the correction window (reporting any fix), then start a new
                // take. The daemon's `recording = true` push will move us to Recording.
                self.end_review();
                self.daemon.toggle();
                true
            }
            // Escape, Enter, Tab, Up/Down, …: close the window, pass the key on.
            Key::Cancel | Key::Passthrough => {
                self.end_review();
                false
            }
        }
    }

    /// The daemon delivered a transcript: type it straight into the app, tell
    /// the daemon to finalize, and open the correction window.
    ///
    /// In streaming (pause-triggered translation) mode the daemon delivers one
    /// transcript per pause while the mic stays open, so a transcript landing on
    /// an open correction window is the next snippet, not an anomaly: the
    /// previous window closes (reporting any in-place fix) and the new snippet
    /// commits. Once the daemon has announced the mic closed, a stray transcript
    /// is still ignored.
    pub fn on_transcript(&mut self, text: String) {
        match self.state {
            State::Recording => {}
            State::Reviewing if self.recording => self.end_review(),
            _ => return, // no take in progress; ignore an unsolicited/late transcript
        }
        // A take-final transcript supersedes any streamed preview: drop the
        // preedit before committing so the correction window below covers only the
        // full committed text.
        self.clear_preview();
        if text.is_empty() {
            self.state = State::Idle;
            return;
        }
        self.surface.commit_text(&text);
        self.daemon.commit(&text);
        self.review = Some(Review::new(text));
        self.state = State::Reviewing;
    }

    /// The daemon delivered a mid-take snippet of a streamed take (a PARTIAL): show
    /// it in the IME-owned **preedit** and keep recording. Nothing is finalized —
    /// the daemon owns the take's single session and commits it at stop — and no
    /// correction window opens mid-take. The snippet is appended to the running
    /// preview and the whole preview is set as the preedit region (preedit
    /// replaces, so we resend the full text), so the stop-time
    /// [`on_reconcile`](Self::on_reconcile) can clear it and commit the verified
    /// text without any live-typed preview in the document to reconcile.
    pub fn on_partial_transcript(&mut self, text: String) {
        if self.state != State::Recording || text.is_empty() {
            return; // no live take; ignore a stray/late partial
        }
        self.preview.push_str(&text);
        self.surface.set_preedit(&self.preview);
    }

    /// The daemon delivered the verified full-take text at the stop of a direct
    /// (review-off) streaming take. The live preview was IME-owned preedit, never
    /// committed, so we simply clear the preedit and commit the verified text once
    /// — no divergence math, no synthesised backspaces that an auto-closing editor
    /// could corrupt. This commit is display-only: the daemon already owns and
    /// committed the streamed session, so we do NOT send our own commit. The full
    /// verified text then becomes the in-place correction window so a later fix
    /// still reports a raw→corrected training pair.
    ///
    /// Self-adjusting w.r.t. the "preview typing" toggle: with preview off no
    /// preedit was ever shown, so [`clear_preview`](Self::clear_preview) is a
    /// no-op and the whole verified text is committed fresh.
    pub fn on_reconcile(&mut self, final_text: String) {
        match self.state {
            State::Recording => {}
            // A reconcile landing on an open window while the mic is still open is
            // the next streamed take's stop: close the prior window (reporting any
            // fix) and reconcile the new one.
            State::Reviewing if self.recording => self.end_review(),
            _ => return, // no live take; ignore an unsolicited/late reconcile
        }
        // Drop the ephemeral preedit preview before committing (clear-then-commit),
        // so the app replaces the underlined preview with the verified text.
        self.clear_preview();
        if final_text.is_empty() {
            // The whole-take decode came back empty: the preview is gone and there
            // is nothing to commit or correct.
            self.state = State::Idle;
            return;
        }
        self.surface.commit_text(&final_text);
        self.review = Some(Review::new(final_text));
        self.state = State::Reviewing;
    }

    /// The daemon's authoritative recording state changed. This is the single
    /// source of truth for the `Idle`/`Recording` phase, so the session mirrors it.
    /// A `Reviewing` correction window is a local overlay: a stop's `false` is
    /// expected and leaves it open, while a fresh `true` (a new take) closes it.
    ///
    /// For a streamed take the correction window is opened by the stop-time
    /// [`on_reconcile`](Self::on_reconcile) (which the daemon sends *before* this
    /// status), so a `recording = false` arriving on the already-open `Reviewing`
    /// state simply leaves it open. A plain `Recording → false` with no transcript
    /// (a silent take) just returns to idle.
    pub fn on_recording_status(&mut self, recording: bool) {
        self.recording = recording;
        match self.state {
            State::Reviewing => {
                if recording {
                    self.end_review();
                    self.state = State::Recording;
                }
            }
            State::Recording if !recording => {
                // Mic closed while still Recording means no reconcile arrived (a
                // silent/aborted streamed take): clear any preedit preview.
                self.clear_preview();
                self.state = State::Idle;
            }
            _ => {
                self.state = if recording {
                    State::Recording
                } else {
                    State::Idle
                };
            }
        }
    }

    /// Drop any in-flight take and return to idle. Used after a daemon reconnect:
    /// the daemon re-pushes its authoritative `RecordingStatus`, so the session
    /// resyncs from a clean slate rather than a stale guess.
    pub fn reset_to_idle(&mut self) {
        self.review = None;
        self.clear_preview();
        self.state = State::Idle;
        self.recording = false;
    }

    /// Commit the user's reviewed (possibly edited) text straight into the app
    /// and record it with the daemon. Used by the review-dialog flow, where the
    /// editing happens in a window the engine owns rather than in the app — so
    /// no post-commit tracking is needed. An empty result cancels the take.
    pub fn commit_reviewed(&mut self, text: &str) {
        if text.is_empty() {
            self.daemon.cancel();
        } else {
            self.surface.commit_text(text);
            self.daemon.commit(text);
        }
        self.review = None;
        self.state = State::Idle;
    }

    /// The user cancelled the review dialog: discard the take.
    pub fn cancel_reviewed(&mut self) {
        self.daemon.cancel();
        self.review = None;
        self.state = State::Idle;
    }

    /// A finished direct (review-off) take arrived but the engine has no focused
    /// context to type it into. Rather than silently lose the text into nowhere —
    /// and let the daemon bank a "you accepted this" training pair that never
    /// landed — discard the take (cancel it daemon-side); the user re-dictates
    /// into a focused field. Like [`on_transcript`](Self::on_transcript) it acts
    /// only on a live take; a stray/late transcript is ignored.
    pub fn on_transcript_without_target(&mut self) {
        match self.state {
            State::Recording => {}
            State::Reviewing if self.recording => {}
            _ => return,
        }
        self.daemon.cancel();
        self.clear_preview();
        self.review = None;
        self.state = State::Idle;
    }

    /// Focus left the input context — close any open correction window.
    pub fn on_focus_out(&mut self) {
        if self.state == State::Reviewing {
            self.end_review();
        }
    }

    /// The daemon reported an error mid-take: return to idle.
    pub fn on_error(&mut self) {
        self.review = None;
        self.clear_preview();
        self.state = State::Idle;
    }

    /// Drop the live preedit preview, if any. Preedit is IME-owned and ephemeral,
    /// so abandoning a take (cancel, error, silent stop, supersede, reconnect) must
    /// visibly clear it. No-op when no preview is showing, so callers stay clean
    /// and no spurious `set_preedit("")` op is emitted.
    fn clear_preview(&mut self) {
        if !self.preview.is_empty() {
            self.preview.clear();
            self.surface.set_preedit("");
        }
    }

    /// Close the correction window: if the user actually edited the dictated
    /// text in place (and the result differs), report the correction.
    fn end_review(&mut self) {
        self.state = State::Idle;
        if let Some(review) = self.review.take() {
            let text = review.text();
            if review.corrected && text != review.committed {
                self.daemon.report_correction(&text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDaemon {
        events: Vec<String>,
    }
    impl DaemonClient for FakeDaemon {
        fn toggle(&mut self) {
            self.events.push("toggle".to_owned());
        }
        fn commit(&mut self, final_text: &str) {
            self.events.push(format!("commit:{final_text}"));
        }
        fn report_correction(&mut self, corrected_text: &str) {
            self.events.push(format!("correct:{corrected_text}"));
        }
        fn cancel(&mut self) {
            self.events.push("cancel".to_owned());
        }
    }

    #[derive(Default)]
    struct FakeSurface {
        committed: Vec<String>,
        /// Each `set_preedit(text)` call's text, in order — lets a test assert the
        /// live preview grew in the IME-owned preedit and was cleared (`""`) at
        /// stop, all without ever touching committed document text.
        preedits: Vec<String>,
    }
    impl Surface for FakeSurface {
        fn commit_text(&mut self, text: &str) {
            self.committed.push(text.to_owned());
        }
        fn set_preedit(&mut self, text: &str) {
            self.preedits.push(text.to_owned());
        }
    }

    fn session() -> Session<FakeDaemon, FakeSurface> {
        Session::new(FakeDaemon::default(), FakeSurface::default())
    }

    /// Dictate `transcript` so the correction window is open, mirroring the real
    /// daemon flow: toggle (start) → daemon announces recording → toggle (stop) →
    /// daemon delivers the transcript → daemon announces recording stopped. The
    /// engine's recording phase is driven entirely by the daemon's pushes.
    fn dictate(s: &mut Session<FakeDaemon, FakeSurface>, transcript: &str) {
        s.on_key(Key::Trigger); // toggle: start intent
        s.on_recording_status(true); // daemon: recording started
        s.on_key(Key::Trigger); // toggle: stop intent
        s.on_transcript(transcript.to_owned()); // daemon: transcript (-> Reviewing)
        s.on_recording_status(false); // daemon: mic closed (does not close review)
    }

    fn type_str(s: &mut Session<FakeDaemon, FakeSurface>, text: &str) {
        for c in text.chars() {
            s.on_key(Key::Char(c));
        }
    }

    fn last_correction(s: &Session<FakeDaemon, FakeSurface>) -> Option<String> {
        s.daemon
            .events
            .iter()
            .rev()
            .find_map(|e| e.strip_prefix("correct:").map(str::to_owned))
    }

    #[test]
    fn passthrough_keys_are_not_consumed_when_idle() {
        let mut s = session();
        assert!(!s.on_key(Key::Char('a')));
        assert!(!s.on_key(Key::Backspace));
        assert!(!s.on_key(Key::Left));
        assert!(!s.on_key(Key::Passthrough));
        assert_eq!(s.state(), State::Idle);
        assert!(s.daemon.events.is_empty());
    }

    #[test]
    fn direct_transcript_without_target_discards_the_take() {
        // A finished direct take with no focused context to type into: nothing is
        // typed, and the take is cancelled daemon-side (so no never-landed training
        // pair is recorded), returning to idle.
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_key(Key::Trigger);
        s.on_transcript_without_target();

        assert!(s.surface.committed.is_empty(), "nothing is typed");
        assert_eq!(
            s.daemon.events,
            ["toggle", "toggle", "cancel"],
            "the take is cancelled daemon-side, not committed"
        );
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn discard_without_target_ignores_a_stray_transcript_when_idle() {
        // No take in progress: the call must not cancel anything.
        let mut s = session();
        s.on_transcript_without_target();
        assert!(s.daemon.events.is_empty(), "no live take -> no cancel");
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn trigger_sends_toggle_and_mirrors_daemon_status() {
        let mut s = session();
        // A trigger sends one direction-free intent and does NOT flip the phase:
        // the engine stays put until the daemon announces the new state.
        assert!(s.on_key(Key::Trigger));
        assert_eq!(s.state(), State::Idle, "no optimistic flip");
        s.on_recording_status(true);
        assert_eq!(s.state(), State::Recording);
        // A second trigger is again just an intent — still no optimistic flip.
        assert!(s.on_key(Key::Trigger));
        assert_eq!(s.state(), State::Recording);
        s.on_recording_status(false);
        assert_eq!(s.state(), State::Idle);
        assert_eq!(s.daemon.events, ["toggle", "toggle"]);
    }

    #[test]
    fn transcript_auto_commits_and_opens_review() {
        let mut s = session();
        dictate(&mut s, "restart traffic");
        assert_eq!(s.surface.committed, ["restart traffic"]);
        assert_eq!(
            s.daemon.events,
            ["toggle", "toggle", "commit:restart traffic"]
        );
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn recording_false_does_not_close_an_open_review() {
        // The mic closes when a take stops, but the post-commit correction window is
        // a local concern that must stay open for the user to fix the text.
        let mut s = session();
        dictate(&mut s, "restart traffic");
        assert_eq!(s.state(), State::Reviewing);
        s.on_recording_status(false);
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn tail_correction_via_backspace_and_retype() {
        let mut s = session();
        dictate(&mut s, "restart traffic");
        for _ in 0.."traffic".len() {
            assert!(!s.on_key(Key::Backspace));
        }
        type_str(&mut s, "Traefik");
        s.on_key(Key::Trigger); // next dictation closes the window (sends a toggle)
        assert_eq!(last_correction(&s), Some("restart Traefik".to_owned()));
        // The window is closed and a toggle was sent; the new take's Recording state
        // arrives via the daemon's push.
        assert_eq!(s.state(), State::Idle);
        s.on_recording_status(true);
        assert_eq!(s.state(), State::Recording);
    }

    #[test]
    fn mid_sentence_edit_with_arrow_navigation() {
        let mut s = session();
        // Whisper heard "deploy traffic and engine X"; fix "traffic" -> "Traefik"
        // and "engine X" -> "nginx" by navigating with arrows.
        dictate(&mut s, "deploy engine X");
        // Cursor is at end. Walk left to delete "engine X" (8 chars) and retype.
        for _ in 0.."engine X".len() {
            s.on_key(Key::Backspace);
        }
        type_str(&mut s, "nginx");
        s.on_focus_out();
        assert_eq!(last_correction(&s), Some("deploy nginx".to_owned()));
    }

    #[test]
    fn insert_in_the_middle_is_captured() {
        let mut s = session();
        dictate(&mut s, "the cat sat");
        // Move to just after "the " and insert "big ".
        s.on_key(Key::Home);
        for _ in 0..4 {
            s.on_key(Key::Right); // cursor after "the "
        }
        type_str(&mut s, "big ");
        s.on_focus_out();
        assert_eq!(last_correction(&s), Some("the big cat sat".to_owned()));
    }

    #[test]
    fn delete_key_removes_forward() {
        let mut s = session();
        dictate(&mut s, "hello world");
        s.on_key(Key::Home);
        s.on_key(Key::Delete); // remove 'h'
        s.on_focus_out();
        assert_eq!(last_correction(&s), Some("ello world".to_owned()));
    }

    #[test]
    fn forward_typing_without_an_edit_is_not_a_correction() {
        let mut s = session();
        dictate(&mut s, "good morning");
        type_str(&mut s, " everyone"); // continuation
        assert!(last_correction(&s).is_none());
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn editing_back_to_the_original_reports_nothing() {
        let mut s = session();
        dictate(&mut s, "alpha");
        s.on_key(Key::Backspace); // "alph"
        s.on_key(Key::Char('a')); // "alpha" again
        s.on_focus_out();
        assert!(last_correction(&s).is_none());
    }

    #[test]
    fn focus_out_reports_an_open_correction() {
        let mut s = session();
        dictate(&mut s, "hello world");
        for _ in 0.."world".len() {
            s.on_key(Key::Backspace);
        }
        type_str(&mut s, "World");
        s.on_focus_out();
        assert_eq!(last_correction(&s), Some("hello World".to_owned()));
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn reviewed_commit_types_and_records_edited_text() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_key(Key::Trigger);
        // (review mode: the dialog returns the edited text instead of auto-commit)
        s.commit_reviewed("deploy Traefik");
        assert_eq!(s.surface.committed, ["deploy Traefik"]);
        assert_eq!(
            s.daemon.events,
            ["toggle", "toggle", "commit:deploy Traefik"]
        );
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn reviewed_cancel_discards_the_take() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_key(Key::Trigger);
        s.cancel_reviewed();
        assert!(s.surface.committed.is_empty());
        assert_eq!(s.daemon.events, ["toggle", "toggle", "cancel"]);
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn escape_during_recording_cancels() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true); // daemon: recording started
        assert_eq!(s.state(), State::Recording);
        assert!(s.on_key(Key::Cancel));
        assert_eq!(s.daemon.events, ["toggle", "cancel"]);
        assert_eq!(s.state(), State::Idle);
    }

    /// Stream `parts` as live partials within an open take (start + recording).
    fn stream_partials(s: &mut Session<FakeDaemon, FakeSurface>, parts: &[&str]) {
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        for p in parts {
            s.on_partial_transcript((*p).to_owned());
        }
    }

    #[test]
    fn reconcile_never_commits_daemon_side() {
        // The daemon already owns (and committed) the streamed session; the engine
        // reconcile is display-only and must NOT send its own commit.
        let mut s = session();
        stream_partials(&mut s, &["helo world"]);
        s.on_reconcile("hello world".to_owned());

        let commits = s
            .daemon
            .events
            .iter()
            .filter(|e| e.starts_with("commit:"))
            .count();
        assert_eq!(commits, 0, "reconcile finalizes daemon-side only");
    }

    #[test]
    fn recording_false_after_reconcile_keeps_review_open() {
        // Stop send-order is reconcile THEN RecordingStatus(false); the trailing
        // status must leave the freshly-opened full-text correction window open.
        let mut s = session();
        stream_partials(&mut s, &["helo world"]);
        s.on_reconcile("hello world".to_owned());
        s.on_recording_status(false);
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn editing_after_reconcile_reports_the_full_corrected_text() {
        // The correction window now covers the FULL final (not just the tail), so a
        // fix anywhere reports the whole corrected take to the daemon.
        let mut s = session();
        stream_partials(&mut s, &["hello world", " deploy nginx"]);
        s.on_reconcile("hello world deploy nginx".to_owned());
        s.on_recording_status(false);
        assert_eq!(s.state(), State::Reviewing);

        for _ in 0.."nginx".len() {
            s.on_key(Key::Backspace);
        }
        type_str(&mut s, "Nginx");
        s.on_focus_out();

        assert_eq!(
            last_correction(&s),
            Some("hello world deploy Nginx".to_owned())
        );
    }

    // --- IME-owned preedit preview (new contract) ---------------------------
    //
    // The live streaming preview must live in preedit, not committed text, so no
    // app can auto-transform it and no backspace-reconcile can corrupt the doc.

    #[test]
    fn partial_snippets_show_in_preedit_not_committed_text() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);

        s.on_partial_transcript("hello world".to_owned());
        s.on_partial_transcript(" second snippet".to_owned());

        // The growing preview is the WHOLE preedit region each time (preedit
        // replaces, not appends), and nothing is committed to the document.
        assert_eq!(
            s.surface.preedits,
            ["hello world", "hello world second snippet"]
        );
        assert!(
            s.surface.committed.is_empty(),
            "the preview is preedit, never committed text"
        );
        assert_eq!(s.daemon.events, ["toggle"], "no per-snippet finalize");
        assert_eq!(s.state(), State::Recording, "still mid-take");
    }

    #[test]
    fn reconcile_clears_preedit_and_commits_only_the_verified_final() {
        // Direct streaming stop: the lossy preview was preedit-only, so at stop the
        // engine clears the preedit and commits the verified text exactly once — no
        // divergence math, no backspaces an auto-closing editor could corrupt.
        let mut s = session();
        stream_partials(&mut s, &["helo world"]);
        assert_eq!(s.surface.preedits, ["helo world"]);
        assert!(s.surface.committed.is_empty(), "preview not committed yet");

        s.on_reconcile("hello world".to_owned());

        assert_eq!(
            s.surface.preedits,
            ["helo world", ""],
            "the preedit preview is cleared at stop"
        );
        assert_eq!(
            s.surface.committed,
            ["hello world"],
            "only the verified full text is committed, once"
        );
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn reconcile_preview_accumulates_across_partials_then_commits_once() {
        let mut s = session();
        stream_partials(&mut s, &["hello ", "wrld"]);
        assert_eq!(s.surface.preedits, ["hello ", "hello wrld"]);

        s.on_reconcile("hello world".to_owned());

        assert_eq!(s.surface.preedits, ["hello ", "hello wrld", ""]);
        assert_eq!(s.surface.committed, ["hello world"]);
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn reconcile_with_no_preview_shows_no_preedit_and_commits_full_final() {
        // Preview typing OFF: no partials, so no preedit was ever shown; the engine
        // just commits the whole verified final. No preedit op, no backspaces.
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);

        s.on_reconcile("the verified text".to_owned());

        assert!(
            s.surface.preedits.is_empty(),
            "no preview was ever shown, so no preedit op"
        );
        assert_eq!(s.surface.committed, ["the verified text"]);
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn reconcile_with_empty_final_clears_preedit_and_idles() {
        // The whole-take decode came back empty (silence/noise after previews):
        // clear the preedit preview and return to idle, committing nothing.
        let mut s = session();
        stream_partials(&mut s, &["ghost text"]);

        s.on_reconcile(String::new());

        assert_eq!(
            s.surface.preedits,
            ["ghost text", ""],
            "preview shown, then cleared"
        );
        assert!(s.surface.committed.is_empty(), "nothing committed");
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn cancel_during_streaming_clears_the_preedit_preview() {
        // Escape mid-take aborts: the ephemeral preedit preview must visibly clear.
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_partial_transcript("draft preview".to_owned());
        assert_eq!(s.surface.preedits, ["draft preview"]);

        assert!(s.on_key(Key::Cancel));

        assert_eq!(s.surface.preedits, ["draft preview", ""]);
        assert!(s.surface.committed.is_empty(), "nothing committed");
        assert_eq!(s.daemon.events, ["toggle", "cancel"]);
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn silent_stop_after_a_preview_clears_the_preedit() {
        // A streamed take showed a live preview but produced no verified text (the
        // mic closed with no reconcile): the ephemeral preedit must be cleared.
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_partial_transcript("half a thought".to_owned());

        s.on_recording_status(false); // mic closed, no reconcile

        assert_eq!(s.surface.preedits, ["half a thought", ""]);
        assert!(s.surface.committed.is_empty());
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn error_during_streaming_clears_the_preedit_preview() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_partial_transcript("partial".to_owned());

        s.on_error();

        assert_eq!(s.surface.preedits, ["partial", ""]);
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn reset_to_idle_clears_a_streaming_preedit_preview() {
        let mut s = session();
        s.on_key(Key::Trigger);
        s.on_recording_status(true);
        s.on_partial_transcript("streaming".to_owned());

        s.reset_to_idle();

        assert_eq!(s.surface.preedits, ["streaming", ""]);
        assert_eq!(s.state(), State::Idle);
    }

    #[test]
    fn partials_outside_a_live_take_are_ignored() {
        let mut s = session();
        s.on_partial_transcript("ghost".to_owned());
        assert!(s.surface.preedits.is_empty(), "idle: no preedit shown");
        assert!(s.surface.committed.is_empty(), "idle: nothing typed");

        // After a finished batch take (review window open, mic closed) a stray
        // partial must not show a preview either.
        dictate(&mut s, "restart traffic");
        s.on_partial_transcript("ghost".to_owned());
        assert!(
            s.surface.preedits.is_empty(),
            "no preedit for a stray partial"
        );
        assert_eq!(s.surface.committed, ["restart traffic"]);
    }

    #[test]
    fn a_new_take_reports_the_previous_correction_first() {
        // Correction window open from a reconciled streamed take; the next take's
        // recording=true must close it (reporting the fix) before the new take.
        let mut s = session();
        stream_partials(&mut s, &["deploy nginx"]);
        s.on_reconcile("deploy nginx".to_owned());
        s.on_recording_status(false);
        for _ in 0.."nginx".len() {
            s.on_key(Key::Backspace);
        }
        type_str(&mut s, "Traefik");

        s.on_key(Key::Trigger);
        s.on_recording_status(true);

        assert_eq!(last_correction(&s), Some("deploy Traefik".to_owned()));
        assert_eq!(s.state(), State::Recording);
    }

    #[test]
    fn late_transcript_after_the_mic_closed_is_still_ignored() {
        // The streaming acceptance must not weaken the unsolicited-transcript
        // guard: once the daemon announced the mic closed, a stray transcript
        // landing on an open review window commits nothing.
        let mut s = session();
        dictate(&mut s, "restart traffic"); // ends Reviewing, recording=false
        s.on_transcript("ghost".to_owned());
        assert_eq!(s.surface.committed, ["restart traffic"]);
        assert_eq!(s.state(), State::Reviewing);
    }

    #[test]
    fn reset_to_idle_clears_an_in_flight_take() {
        // After a daemon reconnect the session resyncs from a clean slate.
        let mut s = session();
        dictate(&mut s, "restart traffic");
        assert_eq!(s.state(), State::Reviewing);
        s.reset_to_idle();
        assert_eq!(s.state(), State::Idle);
        // A late status push after reset is mirrored normally.
        s.on_recording_status(true);
        assert_eq!(s.state(), State::Recording);
    }
}
