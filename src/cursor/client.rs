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
const GENERATION_TIMEOUT: Duration = Duration::from_mins(1);
const COMMIT_MESSAGE_MODEL: &str = "composer-2.5-fast";

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
    argument_prefix: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
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
        let (executable, argument_prefix) = prepare_executable(executable);
        let environment = cursor_environment(&executable);
        Self {
            executable,
            argument_prefix,
            environment,
            runner: Arc::new(SystemProcessRunner),
        }
    }

    /// Crea un cliente con proceso inyectable para pruebas offline.
    #[must_use]
    pub fn with_runner(executable: PathBuf, runner: Arc<dyn ProcessRunner>) -> Self {
        let (executable, argument_prefix) = prepare_executable(executable);
        let environment = cursor_environment(&executable);
        Self {
            executable,
            argument_prefix,
            environment,
            runner,
        }
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
        _repository_root: &Path,
        prompt: String,
        cancellation: &CancellationToken,
    ) -> Result<String, CursorError> {
        // El prompt ya contiene todo el contexto staged. Ejecutar el agente en un
        // directorio vacío evita que cargue reglas o ficheros adicionales del repo;
        // `--trust` solo se aplica a este workspace efímero y permite el modo headless.
        let isolated_workspace = tempfile::tempdir().map_err(CursorError::IsolatedWorkspace)?;
        let output = self.run(
            "cursor-generate-commit-message",
            vec![
                OsString::from("-p"),
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("--sandbox"),
                OsString::from("disabled"),
                OsString::from("--trust"),
                OsString::from("--disable-project-configs"),
                OsString::from("--model"),
                OsString::from(COMMIT_MESSAGE_MODEL),
                OsString::from("--output-format"),
                OsString::from("json"),
            ],
            Some(isolated_workspace.path().to_path_buf()),
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
        let mut full_arguments = self.argument_prefix.clone();
        full_arguments.extend(arguments);
        self.runner.run(
            ProcessRequest {
                label,
                program: self.executable.clone(),
                arguments: full_arguments,
                environment: self.environment.clone(),
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

fn prepare_executable(executable: PathBuf) -> (PathBuf, Vec<OsString>) {
    let Some(directory) = executable.parent() else {
        return (executable, Vec::new());
    };
    if executable
        .extension()
        .is_none_or(|extension| extension != "cmd")
    {
        return (executable, Vec::new());
    }
    let node = directory.join("node.exe");
    let entrypoint = directory.join("index.js");
    if node.is_file() && entrypoint.is_file() {
        (node, vec![entrypoint.into_os_string()])
    } else if let Some(versioned_launcher) = super::executable::latest_version_entrypoint(directory)
    {
        prepare_executable(versioned_launcher)
    } else {
        (executable, Vec::new())
    }
}

fn cursor_environment(executable: &Path) -> Vec<(OsString, OsString)> {
    if executable.file_name().is_none_or(|name| name != "node.exe")
        || std::env::var_os("NODE_COMPILE_CACHE").is_some()
    {
        return Vec::new();
    }
    std::env::var_os("LOCALAPPDATA").map_or_else(Vec::new, |local_app_data| {
        vec![(
            OsString::from("NODE_COMPILE_CACHE"),
            PathBuf::from(local_app_data)
                .join("cursor-compile-cache")
                .into_os_string(),
        )]
    })
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
    use super::{CursorAuthentication, parse_authentication, prepare_executable};

    #[test]
    fn invokes_the_versioned_node_process_directly() {
        let installation = tempfile::tempdir().expect("debe crear la instalación simulada");
        let version = installation.path().join("versions").join("2026.09.02-bbbb");
        std::fs::create_dir_all(&version).expect("debe crear la versión simulada");
        let launcher = installation.path().join("agent.cmd");
        let node = version.join("node.exe");
        let entrypoint = version.join("index.js");
        std::fs::write(&launcher, b"").expect("debe crear el launcher");
        std::fs::write(version.join("cursor-agent.cmd"), b"")
            .expect("debe crear el launcher versionado");
        std::fs::write(&node, b"").expect("debe crear node");
        std::fs::write(&entrypoint, b"").expect("debe crear el entrypoint");

        let (program, prefix) = prepare_executable(launcher);

        assert_eq!(program, node);
        assert_eq!(prefix, vec![entrypoint.into_os_string()]);
    }

    #[test]
    fn parses_nested_authentication_status() {
        let output = br#"{"account":{"authenticated":true},"unknown":"ignored"}"#;

        assert_eq!(
            parse_authentication(output),
            CursorAuthentication::Authenticated
        );
    }
}
