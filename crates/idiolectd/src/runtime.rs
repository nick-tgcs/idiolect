use std::convert::Infallible;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use idiolect_adapter_sqlite::{SqliteMetadataStore, SqliteStorageError};
use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_common::ids::ImeSessionId;
use idiolect_ipc::messages::PROTOCOL_VERSION;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::MetadataStorePort;
use serde_json::json;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonMode {
    FixtureOnce,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonConfig {
    pub db_path: PathBuf,
    pub socket_path: Option<PathBuf>,
    pub mode: DaemonMode,
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

    let mut store = SqliteMetadataStore::open_path(&db_path)
        .map_err(|error| RuntimeError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| RuntimeError::storage("migrate", error))?;

    let input = RecordingInputMethod;
    let storage = RuntimeMetadataStore::new(store, transcript.clone());
    let mut use_case = DictationUseCase::new(input, storage);

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
