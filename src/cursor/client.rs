use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde_json::Value;

use crate::process::{
    CancellationToken, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner,
    SystemProcessRunner,
};

use super::{CursorError, parse_cursor_result};

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(15);
const GENERATION_TIMEOUT: Duration = Duration::from_mins(10);
const COMMIT_MESSAGE_MODEL: &str = "composer-2.5";

/// Estado de autenticación interpretado de forma tolerante.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorAuthentication {
    Authenticated,
    NotAuthenticated,
    Unknown,
}

/// Información cacheable de disponibilidad del CLI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorAvailability {
    pub version: String,
    pub authentication: CursorAuthentication,
}

/// Cliente opcional y aislado para Cursor CLI.
#[derive(Clone)]
pub struct CursorClient {
    executable: PathBuf,
    runner: Arc<dyn ProcessRunner>,
}

impl Default for CursorClient {
    fn default() -> Self {
        Self::new(PathBuf::from("agent"))
    }
}

impl CursorClient {
    /// Crea un cliente de producción desde PATH o una ruta configurada.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            runner: Arc::new(SystemProcessRunner),
        }
    }

    /// Crea un cliente con proceso inyectable para pruebas offline.
    #[must_use]
    pub fn with_runner(executable: PathBuf, runner: Arc<dyn ProcessRunner>) -> Self {
        Self { executable, runner }
    }

    /// Valida versión y consulta autenticación cuando el CLI lo admite.
    pub fn validate(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<CursorAvailability, CursorError> {
        let version_output = self
            .run(
                "cursor-version",
                vec![OsString::from("--version")],
                None,
                None,
                VALIDATION_TIMEOUT,
                cancellation,
            )
            .map_err(|source| CursorError::NotInstalled { source })?;
        let version_output = require_success(version_output)?;
        let version = decode_stdout(&version_output)?.trim().to_owned();

        let status_output = self.run(
            "cursor-status",
            [
                OsString::from("status"),
                OsString::from("--format"),
                OsString::from("json"),
            ]
            .into(),
            None,
            None,
            VALIDATION_TIMEOUT,
            cancellation,
        );
        let authentication = match status_output {
            Ok(output) if output.status.success() => parse_authentication(&output.stdout),
            Ok(_) | Err(_) => CursorAuthentication::Unknown,
        };

        Ok(CursorAvailability {
            version,
            authentication,
        })
    }

    /// Genera una propuesta; nunca ejecuta Git ni crea un commit.
    pub fn generate_commit_message(
        &self,
        repository_root: &Path,
        prompt: String,
        cancellation: &CancellationToken,
    ) -> Result<String, CursorError> {
        let output = self.run(
            "cursor-generate-commit-message",
            vec![
                OsString::from("-p"),
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("--model"),
                OsString::from(COMMIT_MESSAGE_MODEL),
                OsString::from("--workspace"),
                repository_root.as_os_str().to_os_string(),
                OsString::from("--output-format"),
                OsString::from("json"),
            ],
            Some(repository_root.to_path_buf()),
            Some(prompt.into_bytes()),
            GENERATION_TIMEOUT,
            cancellation,
        )?;
        parse_cursor_result(&require_success(output)?.stdout)
    }

    fn run(
        &self,
        label: &'static str,
        arguments: Vec<OsString>,
        current_directory: Option<PathBuf>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        self.runner.run(
            ProcessRequest {
                label,
                program: self.executable.clone(),
                arguments,
                environment: Vec::new(),
                removed_environment: vec![
                    OsString::from("CURSOR_API_KEY"),
                    OsString::from("CURSOR_API_TOKEN"),
                ],
                current_directory,
                stdin,
                timeout,
            },
            cancellation,
        )
    }
}

fn require_success(output: ProcessOutput) -> Result<ProcessOutput, CursorError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(CursorError::CommandFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn decode_stdout(output: &ProcessOutput) -> Result<&str, CursorError> {
    std::str::from_utf8(&output.stdout).map_err(|_| CursorError::InvalidUtf8)
}

fn parse_authentication(output: &[u8]) -> CursorAuthentication {
    let Ok(value) = serde_json::from_slice::<Value>(output) else {
        return CursorAuthentication::Unknown;
    };
    find_authentication(&value).unwrap_or(CursorAuthentication::Unknown)
}

fn find_authentication(value: &Value) -> Option<CursorAuthentication> {
    match value {
        Value::Object(object) => {
            if let Some(authenticated) = object.get("authenticated").and_then(Value::as_bool) {
                return Some(if authenticated {
                    CursorAuthentication::Authenticated
                } else {
                    CursorAuthentication::NotAuthenticated
                });
            }
            if let Some(status) = object.get("status").and_then(Value::as_str) {
                let status = status.to_ascii_lowercase();
                if status.contains("not_authenticated") || status.contains("logged_out") {
                    return Some(CursorAuthentication::NotAuthenticated);
                }
                if status.contains("authenticated") || status.contains("logged_in") {
                    return Some(CursorAuthentication::Authenticated);
                }
            }
            object.values().find_map(find_authentication)
        }
        Value::Array(values) => values.iter().find_map(find_authentication),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorAuthentication, parse_authentication};

    #[test]
    fn parses_nested_authentication_status() {
        let output = br#"{"account":{"authenticated":true},"unknown":"ignored"}"#;

        assert_eq!(
            parse_authentication(output),
            CursorAuthentication::Authenticated
        );
    }
}
