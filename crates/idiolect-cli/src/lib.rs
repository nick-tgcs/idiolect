//! Command-line interface for Idiolect diagnostics and privacy actions.

use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use idiolect_adapter_sqlite::SqliteMetadataStore;
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
        "fcitx5_metadata": fcitx5_metadata,
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

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_available() {
        assert!(!super::crate_name().is_empty());
    }
}
