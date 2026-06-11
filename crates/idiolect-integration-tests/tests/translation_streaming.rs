//! Pause-triggered live translation, end to end through the real daemon.
//!
//! The take is ONE conversation: each pause-delimited snippet is translated and
//! pushed immediately as a PARTIAL preedit (the engine types it and keeps
//! going), and the whole take is finalized exactly once at stop — one merged
//! audio recording and one final string per take for training. The final
//! string is ONE decode of the whole merged recording, not the glued snippet
//! previews: short-context snippet decodes drop words at pause boundaries
//! (a real take lost "I don't want" exactly this way), and the words are
//! provably still in the merged audio. In "review before insert" mode nothing
//! is pushed mid-take; the single review dialog gets the full text at stop.
//!
//! Driven via the reserved `fixture-stream` device (a canned
//! speech–pause–speech clip served through the live polling path), the fixture
//! ASR engine, and an uppercase translator stub honouring the external-command
//! contract (`<command> <input_lang> <output_lang>`, text on stdin).

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use idiolect_ipc::framing::{decode_json_line, encode_json_line};
use idiolect_ipc::messages::{ClientHello, CommitPreedit, IpcMessage, RecordingStatus};
use idiolect_ports::storage::{HistoryState, MetadataStorePort};

const SNIPPET: &str = "[sv>ja] RESTART TRAFFIC";

#[test]
fn snippets_stream_as_partials_and_finalize_as_one_session() {
    let fixture = DaemonFixture::new("snippets");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Two pause-delimited snippets arrive while the mic is still recording,
    // each translated and marked PARTIAL. The second carries its joining space
    // so the app-typed stream reads as one sentence flow.
    client.expect_partial_preedit(SNIPPET);
    client.expect_partial_preedit(&format!(" {SNIPPET}"));

    // A stray CommitPreedit mid-take (nothing is finalizable yet) must neither
    // finalize anything nor flip the published recording state.
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: SNIPPET.to_owned(),
    }));

    // Stop: the take finalizes daemon-side (the text was already typed), so the
    // very next push is the stop's recording=false.
    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);

    // Training/storage truth: ONE committed session whose text is the
    // stop-time decode of the WHOLE recording (the fixture ASR decodes any
    // audio — including the merged take — to one "restart traffic"), and ONE
    // stored recording for the whole take. Glued snippet previews are never
    // the stored truth.
    fixture.assert_single_committed_take(SNIPPET);
    assert_eq!(fixture.stored_audio_count(), 1, "one merged recording per take");
}

#[test]
fn streaming_translation_to_english_needs_no_command() {
    // The Whisper-task fast path contract at the daemon boundary: target "en"
    // with no configured command must still stream snippets (the fixture ASR
    // stands in for the engine's internal translate task).
    let fixture = DaemonFixture::new("entarget").with_translation_overrides("auto", "en", None);
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_partial_preedit("restart traffic");
    client.expect_partial_preedit(" restart traffic");

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take("restart traffic");
}

#[test]
fn unworkable_translation_target_notifies_the_user_once_per_take() {
    // Target "fr" with no translator command: every snippet fails. The failure
    // must reach the USER — as a desktop notification through the configured
    // notify command — not just the journal, and once per take, not once per
    // pause. The wire stays clean (no partials, nothing typed) and nothing is
    // stored.
    let fixture = DaemonFixture::new("notify")
        .with_translation_overrides("auto", "fr", Some(""))
        .with_auto_stop_ms(2_000)
        .with_notify_recorder();
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Both snippets fail, so no partials can arrive; the take auto-stops on
    // the trailing silence and the next push is the stop itself.
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);

    let log = fs::read_to_string(fixture.notifications_log())
        .expect("the notify command must have been invoked");
    // One recorder line per invocation starts with the summary (the body may
    // span lines).
    assert_eq!(
        log.matches("Idiolect — dictation is failing|").count(),
        1,
        "one notification per take, not per pause: {log:?}"
    );
    assert!(
        log.contains("translation.command"),
        "the notification must say what to fix: {log:?}"
    );

    // A take where nothing transcribed stores nothing.
    fixture.assert_no_takes();
    assert_eq!(fixture.stored_audio_count(), 0, "nothing worth keeping");
}

#[test]
fn review_mode_holds_the_whole_take_for_one_dialog() {
    // Requirement: with "Review before insert" on, the take is ONE conversation
    // in ONE dialog — nothing is pushed per pause; the full merged text arrives
    // once at stop with the review flag, and the user's edited text becomes the
    // single committed string.
    let fixture = DaemonFixture::new("review");
    fixture.seed_tray_setting("review_mode", "true");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Mid-take the user still SEES progress: each pause pushes a display-only
    // partial (review=true ⇒ the engine streams it into the listening review
    // dialog, types nothing, finalizes nothing).
    client.expect_display_only_partial(SNIPPET);
    client.expect_display_only_partial(&format!(" {SNIPPET}"));

    // Stop: the one dialog payload — the stop-time decode of the whole
    // recording, not the glued previews — then the stop.
    client.send(IpcMessage::ToggleRecording);
    client.expect_final_review_preedit(SNIPPET);
    client.expect_recording_status(false);

    // The user edits in the (single) dialog and confirms.
    let edited = "edited by the user in one dialog";
    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: edited.to_owned(),
    }));

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take(edited);
    assert_eq!(fixture.stored_audio_count(), 1, "one merged recording per take");
}

#[test]
fn plain_dictation_streams_without_translation() {
    // The default behaviour: pause-triggered streaming applies to EVERY live
    // take, translating or not. With translation off the snippets are plain
    // transcriptions, still one merged session per take.
    let fixture = DaemonFixture::new("plain").with_translation_enabled(false);
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_partial_preedit("restart traffic");
    client.expect_partial_preedit(" restart traffic");

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take("restart traffic");
}

#[test]
fn a_stop_time_decode_failure_keeps_the_previewed_snippet_text() {
    // The stop-time whole-recording decode is the take's truth — but when it
    // fails (here: the translator command dies on its third invocation, i.e.
    // exactly the stop-time call), the take must fall back to the glued
    // snippet previews rather than lose what the user already saw typed.
    let fixture = DaemonFixture::new("stopfail").with_translator_failing_from_call(3);
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_partial_preedit(SNIPPET);
    client.expect_partial_preedit(&format!(" {SNIPPET}"));

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take(&format!("{SNIPPET} {SNIPPET}"));
}

#[test]
fn a_long_pause_auto_stops_and_finalizes_the_take() {
    // The user's complaint: "I pause for a really long time and nothing
    // happens." A pause past vad.auto_stop_silence_ms must end the take by
    // itself — no toggle — finalizing the streamed text as one session and
    // announcing recording=false.
    let fixture = DaemonFixture::new("autostop").with_auto_stop_ms(1_000);
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);
    client.expect_partial_preedit(SNIPPET);
    client.expect_partial_preedit(&format!(" {SNIPPET}"));

    // No stop is ever sent: the silence after the clip crosses the threshold
    // and the daemon ends the take itself.
    client.expect_recording_status(false);

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take(SNIPPET);
}

#[test]
fn a_long_pause_pops_the_single_review_dialog() {
    // With "Review before insert" on, the long pause is what pops THE dialog:
    // the full merged conversation arrives as one review payload, unprompted.
    let fixture = DaemonFixture::new("autostop-review").with_auto_stop_ms(1_000);
    fixture.seed_tray_setting("review_mode", "true");
    let daemon = fixture.spawn_daemon();
    let mut client = DaemonClient::connect(&fixture.socket_path());

    client.send_hello_with_status();
    client.expect_recording_status(false);

    client.send(IpcMessage::ToggleRecording);
    client.expect_recording_status(true);

    // Live display-only progress mid-take, then — with no further client
    // input — the dialog payload (one whole-recording decode) and the stop
    // announcement.
    client.expect_display_only_partial(SNIPPET);
    client.expect_display_only_partial(&format!(" {SNIPPET}"));
    client.expect_final_review_preedit(SNIPPET);
    client.expect_recording_status(false);

    client.send(IpcMessage::CommitPreedit(CommitPreedit {
        text: "confirmed in the one dialog".to_owned(),
    }));

    drop(client);
    assert_daemon_exits_successfully(daemon);
    fixture.assert_single_committed_take("confirmed in the one dialog");
}

struct DaemonFixture {
    root: PathBuf,
    input_language: String,
    output_language: String,
    /// `None` writes an uppercase translator stub; `Some("")` means no command.
    command_override: Option<String>,
    /// Most tests drive the stop themselves; auto-stop tests opt in.
    auto_stop_silence_ms: u32,
    translation_enabled: bool,
    /// When set, the translator stub exits non-zero from its Nth invocation
    /// onward (1-based) — used to fail exactly the stop-time decode.
    translator_fails_from_call: Option<u32>,
    /// When true, `[daemon] notify_command` points at a recorder script that
    /// appends "<summary>|<body>" lines to `notifications_log()`.
    record_notifications: bool,
}

impl DaemonFixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock");
        let root = env::temp_dir().join(format!(
            "idiolect-translation-streaming-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root should be created");
        let fixture = Self {
            root,
            input_language: "sv".to_owned(),
            output_language: "ja".to_owned(),
            command_override: None,
            auto_stop_silence_ms: 0,
            translation_enabled: true,
            translator_fails_from_call: None,
            record_notifications: false,
        };
        fixture.write_files();
        fixture
    }

    fn with_notify_recorder(mut self) -> Self {
        self.record_notifications = true;
        self.write_files();
        self
    }

    fn notifications_log(&self) -> PathBuf {
        self.root.join("notifications.log")
    }

    /// A take where nothing transcribed must leave no trace in history.
    fn assert_no_takes(&self) {
        let mut store =
            idiolect_adapter_sqlite::SqliteMetadataStore::open_path(self.database_path())
                .expect("assert store should open");
        store.migrate().expect("assert store should migrate");
        let entries = store.recent_history(50).expect("history should read");
        assert!(entries.is_empty(), "expected no sessions, got {entries:?}");
    }

    fn with_auto_stop_ms(mut self, auto_stop_silence_ms: u32) -> Self {
        self.auto_stop_silence_ms = auto_stop_silence_ms;
        self.write_files();
        self
    }

    fn with_translation_enabled(mut self, enabled: bool) -> Self {
        self.translation_enabled = enabled;
        self.write_files();
        self
    }

    fn with_translator_failing_from_call(mut self, first_failing_call: u32) -> Self {
        self.translator_fails_from_call = Some(first_failing_call);
        self.write_files();
        self
    }

    fn with_translation_overrides(
        mut self,
        input: &str,
        output: &str,
        command: Option<&str>,
    ) -> Self {
        self.input_language = input.to_owned();
        self.output_language = output.to_owned();
        self.command_override = Some(command.unwrap_or("").to_owned());
        self.write_files();
        self
    }

    /// Persists a tray-settings override in the daemon's database before it
    /// starts, standing in for the corresponding tray click.
    fn seed_tray_setting(&self, key: &str, value: &str) {
        fs::create_dir_all(self.database_path().parent().expect("db parent"))
            .expect("db parent should be created");
        let mut store =
            idiolect_adapter_sqlite::SqliteMetadataStore::open_path(self.database_path())
                .expect("seed store should open");
        store.migrate().expect("seed store should migrate");
        store.set_tray_setting(key, value).expect("setting should persist");
    }

    /// One take ⇒ exactly one history entry, committed, with `expected` text.
    fn assert_single_committed_take(&self, expected: &str) {
        let mut store =
            idiolect_adapter_sqlite::SqliteMetadataStore::open_path(self.database_path())
                .expect("assert store should open");
        store.migrate().expect("assert store should migrate");
        let entries = store.recent_history(50).expect("history should read");
        assert_eq!(entries.len(), 1, "one session per take, got {entries:?}");
        assert_eq!(entries[0].state, HistoryState::Committed);
        assert_eq!(entries[0].text, expected, "the take's single merged string");
    }

    /// Counts stored source recordings under the daemon's audio root.
    fn stored_audio_count(&self) -> usize {
        fn walk(dir: &Path, count: &mut usize) {
            let Ok(reader) = fs::read_dir(dir) else { return };
            for entry in reader.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, count);
                } else {
                    *count += 1;
                }
            }
        }
        let mut count = 0;
        walk(&self.data_dir().join("audio"), &mut count);
        count
    }

    fn spawn_daemon(&self) -> JoinHandle<Result<(), String>> {
        let args = vec![
            "run".to_owned(),
            "--config".to_owned(),
            self.config_path().to_string_lossy().into_owned(),
            "--shutdown-after-client".to_owned(),
        ];
        thread::spawn(move || {
            idiolectd::runtime::run_cli(&args)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    }

    fn translator_command(&self) -> String {
        match &self.command_override {
            Some(command) => command.clone(),
            None => {
                let path = self.root.join("uppercase-translator.sh");
                let body = match self.translator_fails_from_call {
                    // The daemon runs the translator sequentially (single run
                    // loop), so a plain counter file is race-free.
                    Some(first_failing_call) => format!(
                        "#!/bin/sh\n\
                         count_file=\"{}\"\n\
                         n=$(cat \"$count_file\" 2>/dev/null || echo 0)\n\
                         n=$((n + 1))\n\
                         printf '%s' \"$n\" > \"$count_file\"\n\
                         [ \"$n\" -ge {first_failing_call} ] && exit 1\n\
                         printf '[%s>%s] ' \"$1\" \"$2\"; tr '[:lower:]' '[:upper:]'\n",
                        self.root.join("translator-calls").display(),
                    ),
                    None => "#!/bin/sh\nprintf '[%s>%s] ' \"$1\" \"$2\"; tr '[:lower:]' '[:upper:]'\n"
                        .to_owned(),
                };
                fs::write(&path, body).expect("translator stub should be written");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                    .expect("translator stub should be executable");
                path.to_string_lossy().into_owned()
            }
        }
    }

    /// Writes the notification recorder stub honouring the notify contract
    /// (`<command> <summary> <body>`), returning its path.
    fn notify_recorder_command(&self) -> String {
        let path = self.root.join("notify-recorder.sh");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"{}\"\n",
                self.notifications_log().display()
            ),
        )
        .expect("notify recorder should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("notify recorder should be executable");
        path.to_string_lossy().into_owned()
    }

    fn write_files(&self) {
        fs::create_dir_all(self.model_path().parent().expect("model parent"))
            .expect("model parent should be created");
        fs::write(self.model_path(), b"dummy model").expect("dummy model should be written");
        let command = self.translator_command();
        // An empty notify command disables notifications — the right default for
        // tests, which must never pop real desktop toasts.
        let notify_command = if self.record_notifications {
            self.notify_recorder_command()
        } else {
            String::new()
        };
        fs::write(
            self.config_path(),
            format!(
                r#"[user]
default_user_id = "default"

[daemon]
socket_path = "{socket_path}"
log_level = "info"
notify_command = "{notify_command}"

[audio]
input_device = "fixture-stream"
capture_sample_rate = 16000
processing_sample_rate = 16000
channels = 1

[vad]
engine = "webrtc"
threshold = 0.5
min_speech_ms = 250
pre_roll_ms = 300
post_roll_ms = 700
max_utterance_ms = 30000
auto_stop_silence_ms = {auto_stop_silence_ms}

[asr]
engine = "fixture"
model = "whisper-medium-en"
language = "en"
use_gpu = false
threads = 1

[storage]
data_dir = "{data_dir}"
database_path = "{database_path}"
audio_codec = "opus"
audio_container = "ogg"
opus_bitrate_bps = 24000
high_value_opus_bitrate_bps = 32000

[training]
min_approved_examples = 50
trainer = "rust-native-lora"
auto_train = false

[privacy]
retain_audio = true

[translation]
enabled = {translation_enabled}
input_language = "{input_language}"
output_language = "{output_language}"
command = "{command}"

[observability]
log_private_text = false
"#,
                socket_path = self.socket_path().display(),
                data_dir = self.data_dir().display(),
                database_path = self.database_path().display(),
                input_language = self.input_language,
                output_language = self.output_language,
                auto_stop_silence_ms = self.auto_stop_silence_ms,
                translation_enabled = self.translation_enabled,
            ),
        )
        .expect("config should be written");
    }

    fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    fn socket_path(&self) -> PathBuf {
        self.root.join("runtime").join("idiolect.sock")
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    fn database_path(&self) -> PathBuf {
        self.root.join("data").join("db").join("idiolect.sqlite")
    }

    fn model_path(&self) -> PathBuf {
        self.root
            .join("data")
            .join("models")
            .join("whisper")
            .join("whisper-medium-en.bin")
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct DaemonClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl DaemonClient {
    fn connect(socket_path: &Path) -> Self {
        let stream = connect_client(socket_path);
        let reader_stream = stream.try_clone().expect("client stream should clone");
        reader_stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .expect("read timeout should set");
        let reader = BufReader::new(reader_stream);
        Self { stream, reader }
    }

    fn send_hello_with_status(&mut self) {
        self.send(IpcMessage::ClientHello(ClientHello {
            client_name: "idiolect-translation-streaming-test".to_owned(),
            protocol_version: 1,
            features: vec![
                "preedit".to_owned(),
                "commit".to_owned(),
                "recording_status".to_owned(),
            ],
        }));
        match self.read() {
            IpcMessage::ServerHello(server) => assert_eq!(server.protocol_version, 1),
            other => panic!("expected ServerHello, got {other:?}"),
        }
    }

    fn send(&mut self, message: IpcMessage) {
        let line = encode_json_line(&message).expect("message should encode");
        self.stream
            .write_all(line.as_bytes())
            .expect("message should write");
        self.stream.flush().expect("message should flush");
    }

    fn read(&mut self) -> IpcMessage {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .expect("message should read");
        assert!(read > 0, "server closed before sending a message");
        decode_json_line(&line).expect("message should decode")
    }

    fn expect_recording_status(&mut self, expected: bool) {
        match self.read() {
            IpcMessage::RecordingStatus(RecordingStatus { recording }) => {
                assert_eq!(recording, expected, "unexpected recording state");
            }
            other => panic!("expected RecordingStatus({expected}), got {other:?}"),
        }
    }

    /// A live mid-take snippet: typed by the engine, finalizing nothing.
    fn expect_partial_preedit(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => {
                assert_eq!(update.text, expected);
                assert!(update.partial, "mid-take snippets must be partial");
                assert!(!update.review, "direct-mode partials are typed, not review-held");
            }
            other => panic!("expected partial PreeditUpdate({expected:?}), got {other:?}"),
        }
    }

    /// A review-mode mid-take snippet: streamed into the listening review
    /// dialog, typed nowhere, finalizing nothing.
    fn expect_display_only_partial(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => {
                assert_eq!(update.text, expected);
                assert!(update.partial, "mid-take snippets must be partial");
                assert!(update.review, "review-mode partials are display-only");
            }
            other => panic!("expected display-only partial({expected:?}), got {other:?}"),
        }
    }

    /// The take-final review payload: the whole conversation, one dialog.
    fn expect_final_review_preedit(&mut self, expected: &str) {
        match self.read() {
            IpcMessage::PreeditUpdate(update) => {
                assert_eq!(update.text, expected, "full merged text in one dialog");
                assert!(update.review, "review mode routes through the dialog");
                assert!(!update.partial, "the take-final payload is not partial");
            }
            other => panic!("expected final review PreeditUpdate, got {other:?}"),
        }
    }
}

fn connect_client(socket_path: &Path) -> UnixStream {
    for _ in 0..500 {
        match UnixStream::connect(socket_path) {
            Ok(stream) => return stream,
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    panic!("client could not connect to {}", socket_path.display());
}

fn assert_daemon_exits_successfully(daemon: JoinHandle<Result<(), String>>) {
    match daemon.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("daemon returned error: {error}"),
        Err(_) => panic!("daemon thread panicked"),
    }
}
