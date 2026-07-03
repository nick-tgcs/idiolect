use std::convert::Infallible;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageError};
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_common::config::{
    resolve_xdg_paths, IdiolectConfig, ResolvedConfigPaths, XdgBaseDirs,
};
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{ErrorMessage, IpcMessage, PreeditUpdate, PROTOCOL_VERSION};
use idiolect_ports::asr::AsrPort;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{HistoryEntry, MetadataStorePort};
use idiolect_ports::vad::VadPort;

use crate::adapters::RuntimeAdapterProfile;
use crate::run_loop::{RunLoopConfig, RunLoopError};
use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;
use serde_json::json;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonMode {
    FixtureOnce,
    ServeFixture,
    ServeRealFixture,
    Run,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonConfig {
    pub db_path: PathBuf,
    pub socket_path: Option<PathBuf>,
    pub mode: DaemonMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureServerConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub transcript: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealFixtureServerConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub audio_fixture_path: PathBuf,
    pub whisper_model_path: PathBuf,
}

#[derive(Debug)]
pub struct RuntimeError {
    message: String,
    source: Option<Box<dyn Error + 'static>>,
}

impl RuntimeError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    fn storage(action: &str, error: SqliteStorageError) -> Self {
        Self {
            message: format!("storage {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn io(action: &str, error: std::io::Error) -> Self {
        Self {
            message: format!("io {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn framing(error: FramingError) -> Self {
        Self {
            message: format!("ipc framing failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn handshake(error: HandshakeError) -> Self {
        Self {
            message: format!("ipc handshake failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn adapter(action: &str, error: impl std::error::Error + 'static) -> Self {
        Self {
            message: format!("adapter {action} failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn run_loop(error: RunLoopError) -> Self {
        Self {
            message: format!("daemon run loop failed: {error}"),
            source: Some(Box::new(error)),
        }
    }

    fn dictation(error: DictationUseCaseError<Infallible, SqliteStorageError>) -> Self {
        match error {
            DictationUseCaseError::Input(input) => match input {},
            DictationUseCaseError::Storage(error) => Self::storage("dictation", error),
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

pub fn run_from_env() -> i32 {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match run_cli(&args) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

pub fn run_cli(args: &[String]) -> Result<String, RuntimeError> {
    match args {
        [version, format] if version == "--version" && format == "--json" => Ok(version_json()),
        [command, subcommand, format]
            if command == "config" && subcommand == "print-default" && format == "--json" =>
        {
            config_print_default()
        }
        [command, rest @ ..] if command == "run" => run_daemon_setup(rest),
        [command, rest @ ..] if command == "fixture-once" => fixture_once(rest),
        [command, rest @ ..] if command == "serve-fixture" => {
            let config = parse_serve_fixture_config(rest)?;
            serve_fixture(config)?;
            Ok(json!({"served": true}).to_string())
        }
        [command, rest @ ..] if command == "serve-real-fixture" => {
            let config = parse_real_fixture_config(rest)?;
            serve_real_fixture(config)?;
            Ok(json!({"served": true}).to_string())
        }
        [] => Err(RuntimeError::usage("command is required")),
        [unknown, ..] => Err(RuntimeError::usage(format!("unknown command: {unknown}"))),
    }
}

pub fn redact_observability_line(line: &str, include_private: bool) -> String {
    if include_private {
        return line.to_owned();
    }

    for marker in [
        "transcript=",
        "raw_transcript=",
        "corrected_transcript=",
        "text=",
        "clipboard=",
    ] {
        if let Some(index) = line.find(marker) {
            let visible_end = index + marker.len();
            return format!("{}[redacted]", &line[..visible_end]);
        }
    }

    line.to_owned()
}

#[must_use]
pub fn redact_observability_line_for_test(line: &str, include_private: bool) -> String {
    redact_observability_line(line, include_private)
}

fn version_json() -> String {
    json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": PROTOCOL_VERSION,
    })
    .to_string()
}

fn config_print_default() -> Result<String, RuntimeError> {
    serde_json::to_string(&IdiolectConfig::default())
        .map_err(|error| RuntimeError::usage(format!("serialize default config failed: {error}")))
}

fn run_daemon_setup(args: &[String]) -> Result<String, RuntimeError> {
    let run_args = parse_run_args(args)?;
    let config_text = fs::read_to_string(&run_args.config_path)
        .map_err(|error| RuntimeError::io("read config", error))?;
    let config = IdiolectConfig::from_toml_str(&config_text)
        .map_err(|error| RuntimeError::usage(format!("config parse failed: {error}")))?;
    config
        .validate()
        .map_err(|error| RuntimeError::usage(format!("config validation failed: {error}")))?;

    let base_dirs = XdgBaseDirs::default();
    let paths = resolve_xdg_paths(&config, &base_dirs);
    prepare_configured_paths(&paths)?;
    // Desktop integration (the GNOME dock mic) is a side effect of a *real,
    // persistent* daemon launch only — never config validation or the ephemeral
    // test daemons, which must not write into the user's real ~/.local/share.
    if crate::desktop_integration::should_install(
        run_args.check_config,
        run_args.shutdown_after_client,
        std::env::var_os("IDIOLECT_DISABLE_TRAY").is_some(),
    ) {
        crate::desktop_integration::ensure(&base_dirs);
    }
    if !paths.model_path.is_file() {
        return Err(RuntimeError::usage(format!(
            "ASR model path does not exist: {}",
            paths.model_path.display()
        )));
    }

    if run_args.check_config {
        return Ok(json!({
            "ready": true,
            "socket_path": paths.socket_path,
            "database_path": paths.database_path,
            "model_path": paths.model_path,
        })
        .to_string());
    }

    run_daemon_with_tray(config, paths, run_args.shutdown_after_client)
        .map_err(RuntimeError::run_loop)?;

    Ok(json!({"shutdown": true}).to_string())
}

fn run_daemon_with_tray(
    config: IdiolectConfig,
    paths: ResolvedConfigPaths,
    shutdown_after_client: bool,
) -> Result<(), RunLoopError> {
    // The run loop owns the store, tray, clipboard, and background maintenance.
    crate::run_loop::run(RunLoopConfig {
        socket_path: paths.socket_path,
        database_path: paths.database_path,
        audio_root: paths.audio_dir,
        decoded_cache_root: paths.decoded_cache_dir,
        user_id: config.user.default_user_id.clone(),
        shutdown_after_client,
        adapter_profile: RuntimeAdapterProfile {
            audio_input_device: config.audio.input_device.clone(),
            vad_engine: config.vad.engine.clone(),
            asr_engine: config.asr.engine.clone(),
            whisper_model_path: paths.model_path.clone(),
            asr_use_gpu: config.asr.use_gpu,
            asr_language: config.asr.language.clone(),
            asr_threads: config.asr.threads,
        },
        history_config: config.history.clone(),
        translation_config: config.translation.clone(),
        vad_config: config.vad.clone(),
        notify_command: config.daemon.notify_command.clone(),
    })
}

#[derive(Debug)]
struct RunArgs {
    config_path: PathBuf,
    check_config: bool,
    shutdown_after_client: bool,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, RuntimeError> {
    let mut config_path = None;
    let mut check_config = false;
    let mut shutdown_after_client = false;
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                config_path = Some(PathBuf::from(flag_value(args, index, "--config")?));
            }
            "--check-config" => {
                check_config = true;
            }
            "--shutdown-after-client" => {
                shutdown_after_client = true;
            }
            unknown => {
                return Err(RuntimeError::usage(format!(
                    "unknown run argument: {unknown}"
                )));
            }
        }
        index += 1;
    }

    Ok(RunArgs {
        config_path: required_value(config_path, "--config")?,
        check_config,
        shutdown_after_client,
    })
}

fn prepare_configured_paths(paths: &ResolvedConfigPaths) -> Result<(), RuntimeError> {
    // Fail fast (before creating any dirs) if the control socket can't fit in the
    // host kernel's `sun_path`; otherwise `bind` later returns an opaque EINVAL.
    // The limit tightens from 108 (Linux) to 104 (macOS) — see docs/future/009.
    paths
        .validate_socket_path()
        .map_err(|error| RuntimeError::usage(format!("socket path invalid: {error}")))?;
    create_parent_dir("socket parent", &paths.socket_path)?;
    create_parent_dir("database parent", &paths.database_path)?;
    create_dir("models whisper", &paths.models_whisper_dir)?;
    create_dir("audio", &paths.audio_dir)?;
    create_dir("adapters", &paths.adapters_dir)?;
    create_dir("manifests", &paths.manifests_dir)?;
    create_dir("decoded cache", &paths.decoded_cache_dir)?;
    create_dir("trainer cache", &paths.trainer_cache_dir)
}

fn create_parent_dir(action: &str, path: &Path) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        create_dir(action, parent)?;
    }
    Ok(())
}

fn create_dir(action: &str, path: &Path) -> Result<(), RuntimeError> {
    fs::create_dir_all(path).map_err(|error| RuntimeError::io(action, error))
}

fn fixture_once(args: &[String]) -> Result<String, RuntimeError> {
    let flags = parse_fixture_flags(args)?;
    if flags.commit == flags.cancel {
        return Err(RuntimeError::usage(
            "fixture-once requires exactly one of --commit or --cancel",
        ));
    }

    let db_path = required_value(flags.db_path, "--db")?;
    let transcript = required_value(flags.transcript, "--transcript")?;
    let final_text = flags.corrected.unwrap_or_else(|| transcript.clone());

    let mut use_case = dictation_use_case(&db_path, &transcript)?;

    let session_id = use_case
        .start_dictation()
        .map_err(RuntimeError::dictation)?;
    use_case
        .transcript_ready(session_id, &transcript)
        .map_err(RuntimeError::dictation)?;

    if final_text != transcript {
        use_case
            .correct_preedit(session_id, &transcript, &final_text, 0)
            .map_err(RuntimeError::dictation)?;
    }

    if flags.commit {
        use_case
            .commit(session_id, &final_text, "fixture-once-commit")
            .map_err(RuntimeError::dictation)?;
    } else {
        use_case
            .cancel(session_id, "fixture-once-cancel")
            .map_err(RuntimeError::dictation)?;
    }

    Ok(json!({
        "session_id": session_id,
        "text": final_text,
        "committed": flags.commit,
        "cancelled": flags.cancel,
    })
    .to_string())
}

pub fn serve_fixture(config: FixtureServerConfig) -> Result<(), RuntimeError> {
    let _ = fs::remove_file(&config.socket_path);
    let listener =
        UnixListener::bind(&config.socket_path).map_err(|error| RuntimeError::io("bind", error))?;
    let (stream, _) = listener
        .accept()
        .map_err(|error| RuntimeError::io("accept", error))?;
    let result = handle_fixture_connection(stream, &config.db_path, &config.transcript);
    let _ = fs::remove_file(&config.socket_path);
    result
}

pub fn serve_real_fixture(config: RealFixtureServerConfig) -> Result<(), RuntimeError> {
    let transcript = transcribe_real_fixture(&config)?;
    serve_fixture(FixtureServerConfig {
        socket_path: config.socket_path,
        db_path: config.db_path,
        transcript,
    })
}

fn transcribe_real_fixture(config: &RealFixtureServerConfig) -> Result<String, RuntimeError> {
    if !config.audio_fixture_path.is_file() {
        return Err(RuntimeError::usage(format!(
            "audio fixture does not exist: {}",
            config.audio_fixture_path.display()
        )));
    }
    if !config.whisper_model_path.is_file() {
        return Err(RuntimeError::usage(format!(
            "whisper fixture model does not exist: {}",
            config.whisper_model_path.display()
        )));
    }

    let source = speech_and_silence_fixture_16khz_mono();
    let codec = OpusCodec::new();
    let encoded = codec
        .encode(&source)
        .map_err(|error| RuntimeError::adapter("opus encode", error))?;
    let decoded = codec
        .decode(&encoded)
        .map_err(|error| RuntimeError::adapter("opus decode", error))?;
    let mut vad = VadAdapter::new();
    let segments = vad
        .segment(&decoded)
        .map_err(|error| RuntimeError::adapter("vad segment", error))?;
    let speech = segments
        .first()
        .ok_or_else(|| RuntimeError::usage("real fixture produced no speech segment"))?;
    let whisper = WhisperAsr::load_fixture_model()
        .map_err(|error| RuntimeError::adapter("whisper load", error))?;
    let draft = whisper
        .transcribe(speech)
        .map_err(|error| RuntimeError::adapter("whisper transcribe", error))?;
    Ok(draft.text)
}

fn handle_fixture_connection(
    mut stream: UnixStream,
    db_path: &PathBuf,
    transcript: &str,
) -> Result<(), RuntimeError> {
    let reader_stream = stream
        .try_clone()
        .map_err(|error| RuntimeError::io("clone unix stream", error))?;
    let mut reader = BufReader::new(reader_stream);
    let mut use_case = dictation_use_case(db_path, transcript)?;
    let mut session_id = None;
    let mut current_text = transcript.to_owned();
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| RuntimeError::io("read ipc line", error))?;
        if read == 0 {
            return Ok(());
        }

        match decode_json_line(&line).map_err(RuntimeError::framing)? {
            IpcMessage::ClientHello(client) => {
                let response = negotiate_protocol(&client).map_err(RuntimeError::handshake)?;
                send_ipc_message(&mut stream, &IpcMessage::ServerHello(response))?;
            }
            IpcMessage::StartRecording | IpcMessage::ToggleRecording => {
                let started_session = use_case
                    .start_dictation()
                    .map_err(RuntimeError::dictation)?;
                use_case
                    .transcript_ready(started_session, transcript)
                    .map_err(RuntimeError::dictation)?;
                session_id = Some(started_session);
                current_text = transcript.to_owned();
                send_ipc_message(
                    &mut stream,
                    &IpcMessage::PreeditUpdate(PreeditUpdate {
                        text: transcript.to_owned(),
                        review: false,
                        partial: false,
                    }),
                )?;
            }
            // The fixture server transcribes immediately on StartRecording, so an
            // explicit stop is a no-op here (the real run loop honours it).
            IpcMessage::StopRecording => {}
            IpcMessage::CommitPreedit(commit) => {
                let active_session = required_session(session_id)?;
                if commit.text != current_text {
                    use_case
                        .correct_preedit(active_session, &current_text, &commit.text, 0)
                        .map_err(RuntimeError::dictation)?;
                    current_text = commit.text.clone();
                }
                use_case
                    .commit(active_session, &commit.text, "fixture-server-commit")
                    .map_err(RuntimeError::dictation)?;
            }
            IpcMessage::ReportCorrection(correction) => {
                let active_session = required_session(session_id)?;
                if correction.corrected_text != current_text {
                    use_case
                        .correct_preedit(
                            active_session,
                            &current_text,
                            &correction.corrected_text,
                            1,
                        )
                        .map_err(RuntimeError::dictation)?;
                    current_text = correction.corrected_text.clone();
                }
            }
            IpcMessage::CancelPreedit => {
                let active_session = required_session(session_id)?;
                use_case
                    .cancel(active_session, "fixture-server-cancel")
                    .map_err(RuntimeError::dictation)?;
            }
            IpcMessage::HistoryReinsert(_)
            | IpcMessage::HistoryCopy(_)
            | IpcMessage::HistoryReinsertResponse(_)
            | IpcMessage::HistoryCopyResponse(_)
            | IpcMessage::HistoryEdited(_) => {
                send_ipc_message(
                    &mut stream,
                    &IpcMessage::Error(ErrorMessage {
                        code: "unexpected-message".to_owned(),
                        message: "history message not expected in this context".to_owned(),
                    }),
                )?;
            }
            IpcMessage::ServerHello(_)
            | IpcMessage::RecordingStatus(_)
            | IpcMessage::PreeditUpdate(_)
            | IpcMessage::ReplaceTake(_)
            | IpcMessage::InsertText(_)
            | IpcMessage::EditHistory(_)
            | IpcMessage::Error(_) => {
                send_ipc_message(
                    &mut stream,
                    &IpcMessage::Error(ErrorMessage {
                        code: "unexpected-message".to_owned(),
                        message: "message is not valid from client".to_owned(),
                    }),
                )?;
            }
        }
    }
}

fn required_session(session_id: Option<ImeSessionId>) -> Result<ImeSessionId, RuntimeError> {
    session_id.ok_or_else(|| RuntimeError::usage("recording has not started"))
}

fn send_ipc_message(stream: &mut UnixStream, message: &IpcMessage) -> Result<(), RuntimeError> {
    let line = encode_json_line(message).map_err(RuntimeError::framing)?;
    stream
        .write_all(line.as_bytes())
        .map_err(|error| RuntimeError::io("write ipc line", error))?;
    stream
        .flush()
        .map_err(|error| RuntimeError::io("flush ipc line", error))
}

fn dictation_use_case(
    db_path: &PathBuf,
    first_raw_text: &str,
) -> Result<DictationUseCase<RecordingInputMethod, RuntimeMetadataStore>, RuntimeError> {
    let mut store = SqliteMetadataStore::open_path(db_path)
        .map_err(|error| RuntimeError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| RuntimeError::storage("migrate", error))?;

    Ok(DictationUseCase::new(
        RecordingInputMethod,
        RuntimeMetadataStore::new(store, first_raw_text.to_owned()),
    ))
}

#[derive(Default)]
struct FixtureFlags {
    db_path: Option<PathBuf>,
    transcript: Option<String>,
    corrected: Option<String>,
    commit: bool,
    cancel: bool,
}

fn parse_fixture_flags(args: &[String]) -> Result<FixtureFlags, RuntimeError> {
    let mut flags = FixtureFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                flags.db_path = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            "--transcript" => {
                index += 1;
                flags.transcript = Some(flag_value(args, index, "--transcript")?.to_owned());
            }
            "--corrected" => {
                index += 1;
                flags.corrected = Some(flag_value(args, index, "--corrected")?.to_owned());
            }
            "--commit" => {
                flags.commit = true;
            }
            "--cancel" => {
                flags.cancel = true;
            }
            unknown => {
                return Err(RuntimeError::usage(format!(
                    "unknown fixture-once argument: {unknown}"
                )));
            }
        }
        index += 1;
    }

    Ok(flags)
}

fn parse_serve_fixture_config(args: &[String]) -> Result<FixtureServerConfig, RuntimeError> {
    let mut db_path = None;
    let mut socket_path = None;
    let mut transcript = None;
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            "--socket" => {
                index += 1;
                socket_path = Some(PathBuf::from(flag_value(args, index, "--socket")?));
            }
            "--transcript" => {
                index += 1;
                transcript = Some(flag_value(args, index, "--transcript")?.to_owned());
            }
            unknown => {
                return Err(RuntimeError::usage(format!(
                    "unknown serve-fixture argument: {unknown}"
                )));
            }
        }
        index += 1;
    }

    Ok(FixtureServerConfig {
        socket_path: required_value(socket_path, "--socket")?,
        db_path: required_value(db_path, "--db")?,
        transcript: required_value(transcript, "--transcript")?,
    })
}

fn parse_real_fixture_config(args: &[String]) -> Result<RealFixtureServerConfig, RuntimeError> {
    let mut db_path = None;
    let mut socket_path = None;
    let mut audio_fixture_path = None;
    let mut whisper_model_path = None;
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            "--socket" => {
                index += 1;
                socket_path = Some(PathBuf::from(flag_value(args, index, "--socket")?));
            }
            "--audio-fixture" => {
                index += 1;
                audio_fixture_path =
                    Some(PathBuf::from(flag_value(args, index, "--audio-fixture")?));
            }
            "--whisper-model" => {
                index += 1;
                whisper_model_path =
                    Some(PathBuf::from(flag_value(args, index, "--whisper-model")?));
            }
            unknown => {
                return Err(RuntimeError::usage(format!(
                    "unknown serve-real-fixture argument: {unknown}"
                )));
            }
        }
        index += 1;
    }

    Ok(RealFixtureServerConfig {
        socket_path: required_value(socket_path, "--socket")?,
        db_path: required_value(db_path, "--db")?,
        audio_fixture_path: required_value(audio_fixture_path, "--audio-fixture")?,
        whisper_model_path: required_value(whisper_model_path, "--whisper-model")?,
    })
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, RuntimeError> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| RuntimeError::usage(format!("{flag} requires a value")))
}

fn required_value<T>(value: Option<T>, flag: &str) -> Result<T, RuntimeError> {
    value.ok_or_else(|| RuntimeError::usage(format!("{flag} is required")))
}

#[derive(Debug, Default)]
struct RecordingInputMethod;

impl InputMethodPort for RecordingInputMethod {
    type Error = Infallible;

    fn show_preedit(&mut self, _session_id: ImeSessionId, _text: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn update_preedit(
        &mut self,
        _session_id: ImeSessionId,
        _text: &str,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn commit_text(&mut self, _session_id: ImeSessionId, _text: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn cancel_preedit(&mut self, _session_id: ImeSessionId) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct RuntimeMetadataStore {
    inner: SqliteMetadataStore,
    next_raw_text: Option<String>,
}

impl RuntimeMetadataStore {
    fn new(inner: SqliteMetadataStore, first_raw_text: String) -> Self {
        Self {
            inner,
            next_raw_text: Some(first_raw_text),
        }
    }
}

impl MetadataStorePort for RuntimeMetadataStore {
    type Error = SqliteStorageError;

    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error> {
        let owned_raw_text = self
            .next_raw_text
            .take()
            .or_else(|| raw_stt_text.map(str::to_owned));
        self.inner.create_session(owned_raw_text.as_deref())
    }

    fn record_preedit_change(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error> {
        self.inner
            .record_preedit_change(session_id, from_text, to_text, event_index)
    }

    fn commit_session(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        self.inner
            .commit_session(session_id, committed_text, idempotency_key)
    }

    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        self.inner.cancel_session(session_id, idempotency_key)
    }

    fn recent_history(&self, limit: u32) -> Result<Vec<HistoryEntry>, Self::Error> {
        self.inner.recent_history(limit)
    }

    fn get_history_entry(&self, id: i64) -> Result<Option<HistoryEntry>, Self::Error> {
        self.inner.get_history_entry(id)
    }

    fn prune_history(&mut self, older_than_days: u32) -> Result<u64, Self::Error> {
        self.inner.prune_history(older_than_days)
    }

    fn delete_history_entry(&mut self, id: i64) -> Result<(), Self::Error> {
        self.inner.delete_history_entry(id)
    }

    fn get_tray_setting(&self, key: &str) -> Result<Option<String>, Self::Error> {
        self.inner.get_tray_setting(key)
    }

    fn set_tray_setting(&mut self, key: &str, value: &str) -> Result<(), Self::Error> {
        self.inner.set_tray_setting(key, value)
    }

    fn get_all_tray_settings(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, Self::Error> {
        self.inner.get_all_tray_settings()
    }
}

#[cfg(test)]
mod socket_guard_tests {
    // This covers the *rejection* path of the socket guard in isolation (no
    // filesystem touch). The *accept* path — a normal short socket passing
    // validation and proceeding to dir creation — is covered end-to-end by
    // `tests/runtime_check_config.rs` (the happy `--check-config` asserts
    // `ready: true`), so it is not duplicated here.
    use super::{prepare_configured_paths, ResolvedConfigPaths};
    use std::path::PathBuf;

    /// A `ResolvedConfigPaths` whose only meaningful field is the socket path.
    /// Validation rejects an overlong socket before any other path is read or
    /// created, so the rest are inert placeholders that are never touched.
    fn paths_with_socket(socket: PathBuf) -> ResolvedConfigPaths {
        let inert = PathBuf::from("/proc/idiolect-never-created");
        ResolvedConfigPaths {
            config_file: inert.clone(),
            socket_path: socket,
            database_path: inert.clone(),
            model_path: inert.clone(),
            models_whisper_dir: inert.clone(),
            audio_dir: inert.clone(),
            adapters_dir: inert.clone(),
            manifests_dir: inert.clone(),
            decoded_cache_dir: inert.clone(),
            trainer_cache_dir: inert,
        }
    }

    #[test]
    fn overlong_socket_path_is_rejected_before_touching_the_filesystem() {
        // A 201-byte path overflows `sun_path` on every platform (max 108). The
        // daemon must reject it with a readable message, not let `bind` later
        // fail with a bare EINVAL — and must not create any directories first
        // (the inert placeholders above would error if it tried).
        let too_long = PathBuf::from(format!("/{}", "a".repeat(200)));
        let error = prepare_configured_paths(&paths_with_socket(too_long))
            .expect_err("a 201-byte socket path overflows sun_path");
        let message = format!("{error}").to_lowercase();
        assert!(
            message.contains("socket path"),
            "explains the cause: {message}"
        );
        assert!(
            message.contains("too long"),
            "explains the cause: {message}"
        );
    }
}
