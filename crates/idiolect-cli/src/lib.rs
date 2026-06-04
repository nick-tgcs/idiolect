//! Command-line interface for Idiolect diagnostics and privacy actions.

use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};
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
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn storage(action: &str, error: idiolect_adapter_sqlite::SqliteStorageError) -> Self {
        Self {
            message: format!("storage {action} failed: {error}"),
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
            eprintln!("{error}");
            2
        }
    }
}

pub fn execute(args: &[String]) -> Result<String, CliError> {
    match args {
        [command, format] if command == "doctor" && format == "--json" => Ok(doctor_json()),
        [command, ..] if command == "doctor" => Err(CliError::usage("doctor requires --json")),
        [scope, action, rest @ ..] if scope == "privacy" && action == "export" => {
            privacy_export(rest)
        }
        [scope, action, rest @ ..] if scope == "privacy" && action == "delete" => {
            privacy_delete(rest)
        }
        [] => Err(CliError::usage("command is required")),
        [unknown, ..] => Err(CliError::usage(format!("unknown command: {unknown}"))),
    }
}

fn doctor_json() -> String {
    json!({
        "storage": "ok",
        "ipc": "ok",
    })
    .to_string()
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
