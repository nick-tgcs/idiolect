//! Command-line interface for Idiolect diagnostics and privacy actions.

use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use idiolect_adapter_sqlite::SqliteMetadataStore;

use idiolect_ipc::framing::{decode_json_line, encode_json_line, FramingError};
use idiolect_ipc::messages::{HistoryCopy, HistoryReinsert, IpcMessage};
use idiolect_ports::storage::MetadataStorePort;
use serde_json::json;

/// Returns this crate's package name for smoke tests.
#[must_use]
pub fn crate_name() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[derive(Debug)]
pub struct CliError {
    message: String,
    stdout_json: Option<String>,
    exit_code: i32,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stdout_json: None,
            exit_code: 2,
        }
    }

    fn not_implemented(command: &str) -> Self {
        let message = format!("{command} is not implemented yet");
        Self {
            stdout_json: Some(
                json!({
                    "code": "not-implemented",
                    "command": command,
                    "message": message,
                })
                .to_string(),
            ),
            message,
            exit_code: 3,
        }
    }

    fn storage(action: &str, error: idiolect_adapter_sqlite::SqliteStorageError) -> Self {
        Self {
            message: format!("storage {action} failed: {error}"),
            stdout_json: None,
            exit_code: 2,
        }
    }

    fn io(action: &str, error: std::io::Error) -> Self {
        Self {
            message: format!("io {action} failed: {error}"),
            stdout_json: None,
            exit_code: 2,
        }
    }

    fn framing(action: &str, error: FramingError) -> Self {
        Self {
            message: format!("framing {action} failed: {error}"),
            stdout_json: None,
            exit_code: 2,
        }
    }
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for CliError {}

pub fn run_from_env() -> i32 {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match execute(&args) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => {
            if let Some(stdout_json) = &error.stdout_json {
                println!("{stdout_json}");
            } else {
                eprintln!("{error}");
            }
            error.exit_code
        }
    }
}

pub fn execute(args: &[String]) -> Result<String, CliError> {
    match args {
        [command, rest @ ..] if command == "doctor" => doctor(rest),
        [scope, action, rest @ ..] if scope == "privacy" && action == "export" => {
            privacy_export(rest)
        }
        [scope, action, rest @ ..] if scope == "privacy" && action == "delete" => {
            privacy_delete(rest)
        }
        [scope, action, rest @ ..] if scope == "logs" && action == "show" => logs_show(rest),
        [scope, action, ..] if scope == "privacy" && action == "delete-all" => {
            Err(CliError::not_implemented("privacy delete-all"))
        }
        [scope, action, ..] if scope == "service" && action == "status" => {
            Err(CliError::not_implemented("service status"))
        }
        [scope, action, ..] if scope == "service" && action == "restart" => {
            Err(CliError::not_implemented("service restart"))
        }
        [scope, action, ..] if scope == "models" && action == "list" => {
            Err(CliError::not_implemented("models list"))
        }
        [scope, action, ..] if scope == "models" && action == "install" => {
            Err(CliError::not_implemented("models install"))
        }
        [scope, action, ..] if scope == "sessions" && action == "list" => {
            Err(CliError::not_implemented("sessions list"))
        }
        [scope, action, ..] if scope == "sessions" && action == "show" => {
            Err(CliError::not_implemented("sessions show"))
        }
        [scope, action, ..] if scope == "sessions" && action == "delete" => {
            Err(CliError::not_implemented("sessions delete"))
        }
        [scope, action, ..] if scope == "memory" && action == "list" => {
            Err(CliError::not_implemented("memory list"))
        }
        [scope, action, ..] if scope == "memory" && action == "delete" => {
            Err(CliError::not_implemented("memory delete"))
        }
        [scope, action, ..] if scope == "candidates" && action == "list" => {
            Err(CliError::not_implemented("candidates list"))
        }
        [scope, action, ..] if scope == "train" && action == "export-manifest" => {
            Err(CliError::not_implemented("train export-manifest"))
        }
        [scope, action, ..] if scope == "train" && action == "classify" => {
            Err(CliError::not_implemented("train classify"))
        }
        [scope, action, ..] if scope == "train" && action == "run" => {
            Err(CliError::not_implemented("train run"))
        }
        [scope, action, ..] if scope == "adapters" && action == "list" => {
            Err(CliError::not_implemented("adapters list"))
        }
        [scope, action, ..] if scope == "adapters" && action == "promote" => {
            Err(CliError::not_implemented("adapters promote"))
        }
        [scope, action, ..] if scope == "adapters" && action == "rollback" => {
            Err(CliError::not_implemented("adapters rollback"))
        }
        [scope, action, rest @ ..] if scope == "history" => history_command(action, rest),
        [scope, action, rest @ ..] if scope == "tray" => tray_command(action, rest),
        [] => Err(CliError::usage("command is required")),
        [unknown, ..] => Err(CliError::usage(format!("unknown command: {unknown}"))),
    }
}

fn doctor(args: &[String]) -> Result<String, CliError> {
    let flags = parse_doctor_flags(args)?;
    if !flags.json {
        return Err(CliError::usage("doctor requires --json"));
    }

    let paths = DoctorPaths::resolve(&flags);
    let sqlite_migrations = sqlite_migration_status(&paths.database_path);
    let socket = socket_status(&paths.socket_path);
    let model_file = file_status(&paths.model_path);
    let fcitx5_metadata = fcitx5_metadata_status(&paths.fcitx5_data_dir);

    Ok(json!({
        "paths": {
            "database_path": paths.database_path,
            "socket_path": paths.socket_path,
            "model_path": paths.model_path,
            "fcitx5_data_dir": paths.fcitx5_data_dir,
        },
        "sqlite_migrations": sqlite_migrations,
        "socket": socket,
        "model_file": model_file,
        "fcitx5_metadata": fcitx5_metadata.clone(),
        "audio": { "status": "checked" },
        "fcitx5": fcitx5_metadata,
        "storage": sqlite_migrations,
        "ipc": socket,
    })
    .to_string())
}

#[derive(Default)]
struct DoctorFlags {
    json: bool,
    database_path: Option<PathBuf>,
    socket_path: Option<PathBuf>,
    model_path: Option<PathBuf>,
    fcitx5_data_dir: Option<PathBuf>,
}

fn parse_doctor_flags(args: &[String]) -> Result<DoctorFlags, CliError> {
    let mut flags = DoctorFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--json" => flags.json = true,
            "--db" => {
                index += 1;
                flags.database_path = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            "--socket" => {
                index += 1;
                flags.socket_path = Some(PathBuf::from(flag_value(args, index, "--socket")?));
            }
            "--model" => {
                index += 1;
                flags.model_path = Some(PathBuf::from(flag_value(args, index, "--model")?));
            }
            "--fcitx5-data-dir" => {
                index += 1;
                flags.fcitx5_data_dir =
                    Some(PathBuf::from(flag_value(args, index, "--fcitx5-data-dir")?));
            }
            "--audio" | "--fcitx5" | "--models" | "--storage" => {}
            unknown => {
                return Err(CliError::usage(format!(
                    "unknown doctor argument: {unknown}"
                )))
            }
        }
        index += 1;
    }

    Ok(flags)
}

struct DoctorPaths {
    database_path: PathBuf,
    socket_path: PathBuf,
    model_path: PathBuf,
    fcitx5_data_dir: PathBuf,
}

impl DoctorPaths {
    fn resolve(flags: &DoctorFlags) -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let data_home = env_path_or("XDG_DATA_HOME", home.join(".local").join("share"));
        let runtime_dir = env_path_or(
            "XDG_RUNTIME_DIR",
            home.join(".local").join("run").join("idiolect"),
        );
        let data_root = data_home.join("idiolect");

        Self {
            database_path: flags
                .database_path
                .clone()
                .unwrap_or_else(|| data_root.join("db").join("idiolect.sqlite")),
            socket_path: flags
                .socket_path
                .clone()
                .unwrap_or_else(|| runtime_dir.join("idiolect.sock")),
            model_path: flags.model_path.clone().unwrap_or_else(|| {
                data_root
                    .join("models")
                    .join("whisper")
                    .join("whisper-medium-en.bin")
            }),
            fcitx5_data_dir: flags.fcitx5_data_dir.clone().unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fcitx5/idiolect-fcitx5/data")
            }),
        }
    }
}

fn env_path_or(name: &str, fallback: PathBuf) -> PathBuf {
    env::var_os(name).map(PathBuf::from).unwrap_or(fallback)
}

fn sqlite_migration_status(path: &Path) -> serde_json::Value {
    if !path.is_file() {
        return json!({"status": "missing"});
    }

    match open_store(path) {
        Ok(_) => json!({"status": "ok"}),
        Err(error) => json!({"status": "error", "message": error.to_string()}),
    }
}

fn socket_status(path: &Path) -> serde_json::Value {
    if UnixStream::connect(path).is_ok() {
        json!({"status": "reachable"})
    } else {
        json!({"status": "unreachable"})
    }
}

fn file_status(path: &Path) -> serde_json::Value {
    if path.is_file() {
        json!({"status": "present"})
    } else {
        json!({"status": "missing"})
    }
}

fn fcitx5_metadata_status(data_dir: &Path) -> serde_json::Value {
    let required = [
        "idiolect-addon.conf",
        "idiolect.conf",
        "org.fcitx.Fcitx5.Addon.Idiolect.metainfo.xml",
    ];
    let missing = required
        .iter()
        .filter(|file| !data_dir.join(file).is_file())
        .copied()
        .collect::<Vec<_>>();

    if missing.is_empty() {
        json!({"status": "present"})
    } else {
        json!({"status": "missing", "missing": missing})
    }
}

fn logs_show(args: &[String]) -> Result<String, CliError> {
    let flags = parse_logs_flags(args)?;
    let log_file = required_value(flags.log_file, "--log-file")?;
    let contents =
        fs::read_to_string(&log_file).map_err(|error| CliError::io("read log", error))?;
    let rendered = contents
        .lines()
        .map(|line| redact_observability_line(line, flags.include_private))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(if rendered.is_empty() {
        String::new()
    } else {
        format!("{rendered}\n")
    })
}

#[derive(Default)]
struct LogsFlags {
    log_file: Option<PathBuf>,
    include_private: bool,
}

fn parse_logs_flags(args: &[String]) -> Result<LogsFlags, CliError> {
    let mut flags = LogsFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--log-file" => {
                index += 1;
                flags.log_file = Some(PathBuf::from(flag_value(args, index, "--log-file")?));
            }
            "--include-private" => flags.include_private = true,
            unknown => return Err(CliError::usage(format!("unknown logs argument: {unknown}"))),
        }
        index += 1;
    }

    Ok(flags)
}

fn redact_observability_line(line: &str, include_private: bool) -> String {
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

fn privacy_export(args: &[String]) -> Result<String, CliError> {
    let flags = parse_privacy_flags(args)?;
    let user = required_value(flags.user, "--user")?;
    let db = required_value(flags.db, "--db")?;
    let store = open_store(&db)?;
    let summary = store
        .privacy_export_summary(&user)
        .map_err(|error| CliError::storage("export", error))?;

    Ok(json!({
        "user": summary.user_id,
        "training_candidates": summary.training_candidates,
        "user_data_deleted_events": summary.user_data_deleted_events,
    })
    .to_string())
}

fn privacy_delete(args: &[String]) -> Result<String, CliError> {
    let flags = parse_privacy_flags(args)?;
    if !flags.confirm_delete {
        return Err(CliError::usage("privacy delete requires --confirm-delete"));
    }

    let user = required_value(flags.user, "--user")?;
    let db = required_value(flags.db, "--db")?;
    let mut store = open_store(&db)?;
    store
        .delete_user_data(&user)
        .map_err(|error| CliError::storage("delete", error))?;

    Ok(json!({
        "user": user,
        "deleted": true,
    })
    .to_string())
}

#[derive(Default)]
struct PrivacyFlags {
    user: Option<String>,
    db: Option<PathBuf>,
    confirm_delete: bool,
}

fn parse_privacy_flags(args: &[String]) -> Result<PrivacyFlags, CliError> {
    let mut flags = PrivacyFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--user" => {
                index += 1;
                flags.user = Some(flag_value(args, index, "--user")?.to_owned());
            }
            "--db" => {
                index += 1;
                flags.db = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            "--confirm-delete" => {
                flags.confirm_delete = true;
            }
            unknown => {
                return Err(CliError::usage(format!(
                    "unknown privacy argument: {unknown}"
                )));
            }
        }
        index += 1;
    }

    Ok(flags)
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, CliError> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| CliError::usage(format!("{flag} requires a value")))
}

fn required_value<T>(value: Option<T>, flag: &str) -> Result<T, CliError> {
    value.ok_or_else(|| CliError::usage(format!("{flag} is required")))
}

fn open_store(path: &Path) -> Result<SqliteMetadataStore, CliError> {
    let mut store =
        SqliteMetadataStore::open_path(path).map_err(|error| CliError::storage("open", error))?;
    store
        .migrate()
        .map_err(|error| CliError::storage("migrate", error))?;
    Ok(store)
}

// History command implementation
fn history_command(action: &str, args: &[String]) -> Result<String, CliError> {
    match action {
        "list" => history_list(args),
        "show" => history_show(args),
        "delete" => history_delete(args),
        "prune" => history_prune(args),
        "reinsert" => history_reinsert(args),
        "copy" => history_copy(args),
        _ => Err(CliError::usage(format!("unknown history action: {action}"))),
    }
}

#[derive(Default)]
struct HistoryFlags {
    limit: Option<u32>,
    json: bool,
    id: Option<i64>,
    confirm_delete: bool,
    days: Option<u32>,
    socket: Option<PathBuf>,
    db: Option<PathBuf>,
}

fn parse_history_flags(args: &[String]) -> Result<HistoryFlags, CliError> {
    let mut flags = HistoryFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--limit" => {
                index += 1;
                flags.limit = Some(flag_value(args, index, "--limit")?.parse().map_err(|_| CliError::usage("--limit requires a number"))?);
            }
            "--json" => flags.json = true,
            "--id" => {
                index += 1;
                flags.id = Some(flag_value(args, index, "--id")?.parse().map_err(|_| CliError::usage("--id requires a number"))?);
            }
            "--confirm-delete" => flags.confirm_delete = true,
            "--days" => {
                index += 1;
                flags.days = Some(flag_value(args, index, "--days")?.parse().map_err(|_| CliError::usage("--days requires a number"))?);
            }
            "--socket" => {
                index += 1;
                flags.socket = Some(PathBuf::from(flag_value(args, index, "--socket")?));
            }
            "--db" => {
                index += 1;
                flags.db = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            unknown => return Err(CliError::usage(format!("unknown history argument: {unknown}"))),
        }
        index += 1;
    }

    Ok(flags)
}

fn history_list(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    let db = required_value(flags.db, "--db")?;
    let store = open_store(&db)?;
    let limit = flags.limit.unwrap_or(10);
    let entries = store.recent_history(limit).map_err(|e| CliError::storage("list", e))?;
    
    if flags.json {
        Ok(json!({
            "entries": entries.iter().map(|e| json!({
                "id": e.id,
                "session_id": e.session_id,
                "text": e.text,
                "state": format!("{:?}", e.state),
                "created_at": e.created_at,
            })).collect::<Vec<_>>(),
        }).to_string())
    } else {
        let mut output = String::new();
        for entry in entries {
            output.push_str(&format!("{} [{}] {}\n", entry.id, entry.created_at, entry.text));
        }
        Ok(output)
    }
}

fn history_show(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    let id = required_value(flags.id, "--id")?;
    let db = required_value(flags.db, "--db")?;
    let store = open_store(&db)?;
    let entry = store
        .get_history_entry(id)
        .map_err(|e| CliError::storage("show", e))?
        .ok_or_else(|| CliError::usage("history entry not found"))?;

    if flags.json {
        Ok(json!({
            "id": entry.id,
            "session_id": entry.session_id,
            "text": entry.text,
            "state": format!("{:?}", entry.state),
            "created_at": entry.created_at,
        }).to_string())
    } else {
        Ok(format!("{} [{}] {}\n", entry.id, entry.created_at, entry.text))
    }
}

fn history_delete(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    if !flags.confirm_delete {
        return Err(CliError::usage("history delete requires --confirm-delete"));
    }
    let id = required_value(flags.id, "--id")?;
    let db = required_value(flags.db, "--db")?;
    let mut store = open_store(&db)?;
    store.delete_history_entry(id).map_err(|e| CliError::storage("delete", e))?;
    
    Ok(json!({
        "id": id,
        "deleted": true,
    }).to_string())
}

fn history_prune(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    if !flags.confirm_delete {
        return Err(CliError::usage("history prune requires --confirm-delete"));
    }
    let days = required_value(flags.days, "--days")?;
    let db = required_value(flags.db, "--db")?;
    let mut store = open_store(&db)?;
    let count = store.prune_history(days).map_err(|e| CliError::storage("prune", e))?;
    
    Ok(json!({
        "days": days,
        "deleted_count": count,
    }).to_string())
}

fn history_reinsert(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    let id = required_value(flags.id, "--id")?;
    let socket_path = flags.socket.unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".local").join("run").join("idiolect"));
        runtime_dir.join("idiolect.sock")
    });
    
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| CliError::io("connect", e))?;
    let message = IpcMessage::HistoryReinsert(HistoryReinsert { id });
    let line = encode_json_line(&message).map_err(|e| CliError::framing("encode", e))?;
    stream.write_all(line.as_bytes()).map_err(|e| CliError::io("write", e))?;
    stream.flush().map_err(|e| CliError::io("flush", e))?;
    
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| CliError::io("clone", e))?);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| CliError::io("read", e))?;
    let response = decode_json_line(&line).map_err(|e| CliError::framing("decode", e))?;
    let response = match response {
        IpcMessage::HistoryReinsertResponse(r) => r,
        _ => return Err(CliError::usage("unexpected response type")),
    };
    
    if flags.json {
        Ok(json!({
            "id": id,
            "success": response.success,
            "error": response.error,
        }).to_string())
    } else if response.success {
        Ok(format!("Reinserted history entry {}\n", id))
    } else {
        Err(CliError::usage(response.error.unwrap_or_else(|| "Unknown error".to_owned())))
    }
}

fn history_copy(args: &[String]) -> Result<String, CliError> {
    let flags = parse_history_flags(args)?;
    let id = required_value(flags.id, "--id")?;
    let socket_path = flags.socket.unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".local").join("run").join("idiolect"));
        runtime_dir.join("idiolect.sock")
    });
    
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| CliError::io("connect", e))?;
    let message = IpcMessage::HistoryCopy(HistoryCopy { id });
    let line = encode_json_line(&message).map_err(|e| CliError::framing("encode", e))?;
    stream.write_all(line.as_bytes()).map_err(|e| CliError::io("write", e))?;
    stream.flush().map_err(|e| CliError::io("flush", e))?;
    
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| CliError::io("clone", e))?);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| CliError::io("read", e))?;
    let response = decode_json_line(&line).map_err(|e| CliError::framing("decode", e))?;
    let response = match response {
        IpcMessage::HistoryCopyResponse(r) => r,
        _ => return Err(CliError::usage("unexpected response type")),
    };
    
    if flags.json {
        Ok(json!({
            "id": id,
            "success": response.success,
            "error": response.error,
        }).to_string())
    } else if response.success {
        Ok(format!("Copied history entry {} to clipboard\n", id))
    } else {
        Err(CliError::usage(response.error.unwrap_or_else(|| "Unknown error".to_owned())))
    }
}

// Tray command implementation
fn tray_command(action: &str, args: &[String]) -> Result<String, CliError> {
    match action {
        "status" => tray_status(args),
        "config" => tray_config(args),
        "menu" => tray_menu(args),
        _ => Err(CliError::usage(format!("unknown tray action: {action}"))),
    }
}

#[derive(Default)]
struct TrayFlags {
    json: bool,
    retention_days: Option<u32>,
    max_entries: Option<u32>,
    db: Option<PathBuf>,
}

fn parse_tray_flags(args: &[String]) -> Result<TrayFlags, CliError> {
    let mut flags = TrayFlags::default();
    let mut index = 0_usize;

    while index < args.len() {
        match args[index].as_str() {
            "--json" => flags.json = true,
            "--retention-days" => {
                index += 1;
                flags.retention_days = Some(flag_value(args, index, "--retention-days")?.parse().map_err(|_| CliError::usage("--retention-days requires a number"))?);
            }
            "--max-entries" => {
                index += 1;
                flags.max_entries = Some(flag_value(args, index, "--max-entries")?.parse().map_err(|_| CliError::usage("--max-entries requires a number"))?);
            }
            "--db" => {
                index += 1;
                flags.db = Some(PathBuf::from(flag_value(args, index, "--db")?));
            }
            unknown => return Err(CliError::usage(format!("unknown tray argument: {unknown}"))),
        }
        index += 1;
    }

    Ok(flags)
}

fn tray_status(args: &[String]) -> Result<String, CliError> {
    let flags = parse_tray_flags(args)?;
    let db = required_value(flags.db, "--db")?;
    let store = open_store(&db)?;
    let settings = store.get_all_tray_settings().map_err(|e| CliError::storage("get settings", e))?;
    
    let retention = settings.get("retention_days").cloned().unwrap_or_else(|| "1".to_string());
    let max_entries = settings.get("max_entries").cloned().unwrap_or_else(|| "10".to_string());
    
    if flags.json {
        Ok(json!({
            "retention_days": retention.parse::<u32>().unwrap_or(1),
            "max_entries": max_entries.parse::<u32>().unwrap_or(10),
        }).to_string())
    } else {
        Ok(format!("Retention: {} days\nMax Entries: {}\n", retention, max_entries))
    }
}

fn tray_config(args: &[String]) -> Result<String, CliError> {
    let flags = parse_tray_flags(args)?;
    let db = required_value(flags.db, "--db")?;
    let mut store = open_store(&db)?;
    
    if let Some(days) = flags.retention_days {
        idiolect_application::use_cases::menu::validate_retention_days(days)
            .map_err(|error| CliError::usage(error.to_string()))?;
        store.set_tray_setting("retention_days", &days.to_string()).map_err(|e| CliError::storage("set retention", e))?;
    }

    if let Some(max) = flags.max_entries {
        idiolect_application::use_cases::menu::validate_max_entries(max)
            .map_err(|error| CliError::usage(error.to_string()))?;
        store.set_tray_setting("max_entries", &max.to_string()).map_err(|e| CliError::storage("set max_entries", e))?;
    }
    
    let settings = store.get_all_tray_settings().map_err(|e| CliError::storage("get settings", e))?;
    let retention = settings.get("retention_days").cloned().unwrap_or_else(|| "1".to_string());
    let max_entries = settings.get("max_entries").cloned().unwrap_or_else(|| "10".to_string());
    
    if flags.json {
        Ok(json!({
            "retention_days": retention.parse::<u32>().unwrap_or(1),
            "max_entries": max_entries.parse::<u32>().unwrap_or(10),
        }).to_string())
    } else {
        Ok(format!("Updated: Retention: {} days, Max Entries: {}\n", retention, max_entries))
    }
}

fn tray_menu(args: &[String]) -> Result<String, CliError> {
    let flags = parse_tray_flags(args)?;
    let db = required_value(flags.db, "--db")?;
    let store = open_store(&db)?;
    
    let settings = store.get_all_tray_settings().map_err(|e| CliError::storage("get settings", e))?;
    let retention = settings.get("retention_days").cloned().unwrap_or_else(|| "1".to_string()).parse::<u32>().unwrap_or(1);
    let max_entries = settings.get("max_entries").cloned().unwrap_or_else(|| "10".to_string()).parse::<u32>().unwrap_or(10);
    
    let history_config = idiolect_common::config::HistoryConfig {
        retention_days: retention,
        max_entries,
        ..idiolect_common::config::HistoryConfig::default()
    };
    
    let entries = store.recent_history(max_entries).map_err(|e| CliError::storage("list", e))?;
    let menu = idiolect_application::use_cases::menu::MenuUseCase::new().get_menu(
        idiolect_application::use_cases::menu::RecordingState::Idle,
        &entries,
        &history_config,
        &idiolect_common::config::TranslationConfig::default(),
    );
    
    if flags.json {
        Ok(json!({
            "menu": menu.iter().map(|item| json!({
                "id": item.id,
                "label": item.label,
                "enabled": item.enabled,
                "kind": format!("{:?}", item.kind),
            })).collect::<Vec<_>>(),
        }).to_string())
    } else {
        let mut output = String::new();
        for item in menu {
            output.push_str(&format!("{} ({}): {}\n", item.id, item.enabled, item.label));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
