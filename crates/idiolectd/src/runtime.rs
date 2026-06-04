use std::convert::Infallible;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageError};
use idiolect_adapter_vad::VadAdapter;
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::handshake::{negotiate_protocol, HandshakeError};
use idiolect_ipc::messages::{ErrorMessage, IpcMessage, PreeditUpdate, PROTOCOL_VERSION};
use idiolect_ports::asr::AsrPort;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::MetadataStorePort;
use idiolect_ports::vad::VadPort;
use idiolect_test_support::fixtures::speech_and_silence_fixture_16khz_mono;
use serde_json::json;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonMode {
    FixtureOnce,
    ServeFixture,
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

fn version_json() -> String {
    json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": PROTOCOL_VERSION,
    })
    .to_string()
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
            IpcMessage::StartRecording => {
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
                    }),
                )?;
            }
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
            IpcMessage::CancelPreedit => {
                let active_session = required_session(session_id)?;
                use_case
                    .cancel(active_session, "fixture-server-cancel")
                    .map_err(RuntimeError::dictation)?;
            }
            IpcMessage::ServerHello(_) | IpcMessage::PreeditUpdate(_) | IpcMessage::Error(_) => {
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
}
