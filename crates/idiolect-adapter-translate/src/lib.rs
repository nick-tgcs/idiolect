//! Idiolect translation adapter: an external-command translator.
//!
//! Following the repo's subprocess-contract pattern (review/retention dialogs),
//! the translator is any executable invoked as `<command> <input_lang>
//! <output_lang>` with the source text on stdin; it prints the translation to
//! stdout and exits 0. This keeps the daemon free of MT runtime dependencies
//! while supporting any language pair the user has tooling for.

use std::io::Write;
use std::process::{Command, Stdio};

use idiolect_ports::translation::{TranslationPort, TranslationRequest};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandTranslatorError {
    #[error("translator command is not configured")]
    NotConfigured,
    #[error("translator command failed to run: {message}")]
    SpawnFailed { message: String },
    #[error("translator command exited with status {status}: {stderr}")]
    CommandFailed { status: i32, stderr: String },
    #[error("translator command produced no output")]
    EmptyOutput,
    #[error("translator output was not valid UTF-8")]
    InvalidUtf8,
}

/// Translates by piping text through a configured external command.
pub struct CommandTranslator {
    command: String,
}

impl CommandTranslator {
    /// Builds a translator from a configured command string. Returns `None`
    /// when the command is empty/blank ("not configured").
    #[must_use]
    pub fn from_config(command: &str) -> Option<Self> {
        let command = command.trim();
        if command.is_empty() {
            return None;
        }
        Some(Self {
            command: command.to_owned(),
        })
    }
}

impl TranslationPort for CommandTranslator {
    type Error = CommandTranslatorError;

    fn translate(&self, request: &TranslationRequest<'_>) -> Result<String, Self::Error> {
        let mut child = Command::new(&self.command)
            .arg(request.source_language)
            .arg(request.target_language)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| CommandTranslatorError::SpawnFailed {
                message: error.to_string(),
            })?;

        // The child reads stdin to EOF before writing, so send the full text and
        // close the pipe before collecting output. A child that exits without
        // reading stdin (e.g. it failed fast) breaks the pipe mid-write; that is
        // not a spawn failure — the exit status below is the real verdict.
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CommandTranslatorError::SpawnFailed {
                message: "translator stdin was unavailable".to_owned(),
            })?;
        match stdin.write_all(request.text.as_bytes()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(error) => {
                return Err(CommandTranslatorError::SpawnFailed {
                    message: error.to_string(),
                });
            }
        }
        drop(stdin);

        let output =
            child
                .wait_with_output()
                .map_err(|error| CommandTranslatorError::SpawnFailed {
                    message: error.to_string(),
                })?;

        if !output.status.success() {
            return Err(CommandTranslatorError::CommandFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        let translated = String::from_utf8(output.stdout)
            .map_err(|_| CommandTranslatorError::InvalidUtf8)?
            .trim()
            .to_owned();
        if translated.is_empty() {
            return Err(CommandTranslatorError::EmptyOutput);
        }
        Ok(translated)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use idiolect_ports::translation::{TranslationPort, TranslationRequest};

    use super::{CommandTranslator, CommandTranslatorError};

    /// Writes an executable shell script and returns its path.
    fn script(tag: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "idiolect-translate-test-{tag}-{}",
            std::process::id()
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
        path
    }

    fn request<'a>(text: &'a str) -> TranslationRequest<'a> {
        TranslationRequest {
            text,
            source_language: "sv",
            target_language: "en",
        }
    }

    /// Translate, retrying the fork/exec ETXTBSY race: a parallel test thread's
    /// fork can briefly hold this script's write fd open, making exec fail with
    /// "Text file busy". Purely a test-environment artifact (the daemon never
    /// writes the translator binary), so retry until the window closes.
    fn translate_retrying(
        translator: &CommandTranslator,
        req: &TranslationRequest<'_>,
    ) -> Result<String, CommandTranslatorError> {
        for _ in 0..100 {
            match translator.translate(req) {
                Err(CommandTranslatorError::SpawnFailed { message })
                    if message.contains("Text file busy") =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                outcome => return outcome,
            }
        }
        translator.translate(req)
    }

    #[test]
    fn command_receives_language_args_and_stdin_text() {
        // The subprocess contract: argv = [input_lang, output_lang], text on
        // stdin, translation on stdout.
        let path = script("args", r#"printf '%s:%s:' "$1" "$2"; cat"#);
        let translator =
            CommandTranslator::from_config(path.to_str().expect("utf8 path")).expect("configured");

        let translated =
            translate_retrying(&translator, &request("hej världen")).expect("translate");

        assert_eq!(translated, "sv:en:hej världen");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn trailing_newline_is_trimmed() {
        let path = script("newline", r#"cat; echo"#);
        let translator =
            CommandTranslator::from_config(path.to_str().expect("utf8 path")).expect("configured");

        assert_eq!(
            translate_retrying(&translator, &request("hello")).expect("translate"),
            "hello"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nonzero_exit_surfaces_status_and_stderr() {
        let path = script("fail", r#"echo "model missing" >&2; exit 3"#);
        let translator =
            CommandTranslator::from_config(path.to_str().expect("utf8 path")).expect("configured");

        let error = translate_retrying(&translator, &request("hello")).expect_err("must fail");

        assert_eq!(
            error,
            CommandTranslatorError::CommandFailed {
                status: 3,
                stderr: "model missing".to_owned(),
            }
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_command_is_a_typed_spawn_error() {
        let translator =
            CommandTranslator::from_config("/nonexistent/idiolect-translator").expect("configured");

        let error = translate_retrying(&translator, &request("hello")).expect_err("must fail");

        assert!(matches!(error, CommandTranslatorError::SpawnFailed { .. }));
    }

    #[test]
    fn empty_output_is_rejected_not_committed() {
        // An empty "translation" must never silently replace the user's words.
        let path = script("empty", "true");
        let translator =
            CommandTranslator::from_config(path.to_str().expect("utf8 path")).expect("configured");

        let error = translate_retrying(&translator, &request("hello")).expect_err("must fail");

        assert_eq!(error, CommandTranslatorError::EmptyOutput);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn blank_config_means_not_configured() {
        assert!(CommandTranslator::from_config("").is_none());
        assert!(CommandTranslator::from_config("   ").is_none());
    }
}
