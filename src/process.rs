use std::{
    ffi::OsString,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use tracing::{debug, warn};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const PROCESS_TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);

/// Señal cooperativa que permite cancelar un proceso hijo.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    is_cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Solicita la cancelación sin bloquear al llamador.
    pub fn cancel(&self) {
        self.is_cancelled.store(true, Ordering::Release);
    }

    /// Indica si algún propietario solicitó cancelar.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Acquire)
    }
}

/// Solicitud segura para ejecutar un programa sin intervención de una shell.
#[derive(Clone, Debug)]
pub struct ProcessRequest {
    pub label: &'static str,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub removed_environment: Vec<OsString>,
    pub current_directory: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
}

/// Salida capturada de un proceso finalizado.
#[derive(Clone, Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Fallo de infraestructura al crear o supervisar un proceso.
#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("no se pudo iniciar el proceso: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("falló la comunicación por stdin: {0}")]
    Stdin(#[source] std::io::Error),
    #[error("falló la lectura de {stream}: {source}")]
    ReadPipe {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("el proceso fue cancelado")]
    Cancelled,
    #[error("el proceso superó el tiempo máximo de {0:?}")]
    TimedOut(Duration),
    #[error("no se pudo consultar o finalizar el proceso: {0}")]
    Wait(#[source] std::io::Error),
}

/// Abstracción inyectable usada por Git y Cursor CLI.
pub trait ProcessRunner: Send + Sync {
    /// Ejecuta una solicitud y devuelve toda su salida.
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError>;
}

/// Implementación que ejecuta procesos hijos del sistema operativo.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(
        &self,
        request: ProcessRequest,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        let started_at = Instant::now();
        let stdin = request.stdin;
        let mut command = Command::new(&request.program);
        command
            .args(&request.arguments)
            .envs(request.environment.iter().cloned())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;

            command.creation_flags(CREATE_NO_WINDOW);
        }
        for variable in &request.removed_environment {
            command.env_remove(variable);
        }

        // Los temporales evitan que un descendiente que herede un pipe mantenga
        // bloqueado el cleanup después de que termine o se cancele el proceso
        // padre. stdout y stderr siguen siendo capturados de forma independiente.
        let stdout_capture = tempfile::tempfile().map_err(ProcessError::Spawn)?;
        let stdout_reader = stdout_capture.try_clone().map_err(ProcessError::Spawn)?;
        let stderr_capture = tempfile::tempfile().map_err(ProcessError::Spawn)?;
        let stderr_reader = stderr_capture.try_clone().map_err(ProcessError::Spawn)?;
        command.stdout(Stdio::from(stdout_capture));
        command.stderr(Stdio::from(stderr_capture));

        if let Some(input) = stdin {
            let mut stdin_capture = tempfile::tempfile().map_err(ProcessError::Stdin)?;
            stdin_capture
                .write_all(&input)
                .map_err(ProcessError::Stdin)?;
            stdin_capture
                .seek(SeekFrom::Start(0))
                .map_err(ProcessError::Stdin)?;
            command.stdin(Stdio::from(stdin_capture));
        } else {
            command.stdin(Stdio::null());
        }
        if let Some(current_directory) = &request.current_directory {
            command.current_dir(current_directory);
        }

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;

        let mut poll_interval = Duration::from_micros(150);
        let maximum_poll_interval = Duration::from_millis(25);
        let status = loop {
            if cancellation.is_cancelled() {
                terminate_child(&mut child)?;
                warn!(
                    operation = request.label,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "Proceso cancelado"
                );
                return Err(ProcessError::Cancelled);
            }
            if started_at.elapsed() >= request.timeout {
                terminate_child(&mut child)?;
                warn!(
                    operation = request.label,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "Proceso agotó su tiempo máximo"
                );
                return Err(ProcessError::TimedOut(request.timeout));
            }
            if let Some(status) = child.try_wait().map_err(ProcessError::Wait)? {
                break status;
            }
            thread::sleep(poll_interval);
            poll_interval = poll_interval.saturating_mul(2).min(maximum_poll_interval);
        };

        let stdout = read_capture(stdout_reader).map_err(|source| ProcessError::ReadPipe {
            stream: "stdout",
            source,
        })?;
        let stderr = read_capture(stderr_reader).map_err(|source| ProcessError::ReadPipe {
            stream: "stderr",
            source,
        })?;
        debug!(
            operation = request.label,
            success = status.success(),
            elapsed_ms = started_at.elapsed().as_millis(),
            "Proceso finalizado"
        );
        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
    }
}

fn read_capture(mut capture: impl Read + Seek) -> Result<Vec<u8>, std::io::Error> {
    let mut output = Vec::new();
    capture.seek(SeekFrom::Start(0))?;
    capture.read_to_end(&mut output)?;
    Ok(output)
}

fn terminate_child(child: &mut std::process::Child) -> Result<(), ProcessError> {
    #[cfg(target_os = "windows")]
    let tree_error = if child.try_wait().map_err(ProcessError::Wait)?.is_none() {
        terminate_process_tree(child.id()).err()
    } else {
        None
    };

    if let Err(kill_error) = child.kill()
        && kill_error.kind() != std::io::ErrorKind::InvalidInput
    {
        return Err(ProcessError::Wait(kill_error));
    }
    #[cfg(target_os = "windows")]
    let wait_result = wait_for_process(child, PROCESS_TERMINATION_TIMEOUT).map(|_| ());
    #[cfg(not(target_os = "windows"))]
    let wait_result = child.wait().map(|_| ()).map_err(ProcessError::Wait);
    wait_result?;

    #[cfg(target_os = "windows")]
    if let Some(tree_error) = tree_error {
        return Err(tree_error);
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn terminate_process_tree(process_id: u32) -> Result<(), ProcessError> {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new(taskkill_executable());
    command
        .args([
            OsString::from("/PID"),
            process_id.to_string().into(),
            OsString::from("/T"),
            OsString::from("/F"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    let mut taskkill = command.spawn().map_err(ProcessError::Wait)?;
    let status = wait_for_process(&mut taskkill, PROCESS_TERMINATION_TIMEOUT)?;
    if status.success() {
        Ok(())
    } else {
        Err(ProcessError::Wait(std::io::Error::other(format!(
            "taskkill terminó con el código {:?}",
            status.code()
        ))))
    }
}

#[cfg(target_os = "windows")]
fn taskkill_executable() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(
        || PathBuf::from("taskkill.exe"),
        |system_root| {
            PathBuf::from(system_root)
                .join("System32")
                .join("taskkill.exe")
        },
    )
}

#[cfg(target_os = "windows")]
fn wait_for_exit_after_kill(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<(), ProcessError> {
    let started_at = Instant::now();
    let mut poll_interval = Duration::from_millis(1);
    let maximum_poll_interval = Duration::from_millis(25);
    while started_at.elapsed() < timeout {
        if child.try_wait().map_err(ProcessError::Wait)?.is_some() {
            return Ok(());
        }
        thread::sleep(poll_interval);
        poll_interval = poll_interval.saturating_mul(2).min(maximum_poll_interval);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn wait_for_process(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<ExitStatus, ProcessError> {
    let started_at = Instant::now();
    let mut poll_interval = Duration::from_millis(1);
    let maximum_poll_interval = Duration::from_millis(25);
    loop {
        if let Some(status) = child.try_wait().map_err(ProcessError::Wait)? {
            return Ok(status);
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = wait_for_exit_after_kill(child, timeout);
            return Err(ProcessError::Wait(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "taskkill no terminó dentro del límite de cleanup",
            )));
        }
        thread::sleep(poll_interval);
        poll_interval = poll_interval.saturating_mul(2).min(maximum_poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    #[cfg(windows)]
    use std::{fs, path::Path};

    use super::{CancellationToken, ProcessRequest, ProcessRunner, SystemProcessRunner};

    #[cfg(windows)]
    use super::ProcessError;

    #[test]
    fn captures_output_without_a_shell_owned_pipe() {
        let request = ProcessRequest {
            label: "process-capture-test",
            program: PathBuf::from(if cfg!(windows) { "git.exe" } else { "git" }),
            arguments: vec!["--version".into()],
            environment: Vec::new(),
            removed_environment: Vec::new(),
            current_directory: None,
            stdin: None,
            timeout: Duration::from_secs(10),
        };
        let output = SystemProcessRunner
            .run(request, &CancellationToken::default())
            .expect("Git debe poder devolver su versión");

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("git version"));
    }

    #[cfg(windows)]
    #[test]
    fn timeout_terminates_descendants_that_inherit_output_handles() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            descendant_command(),
            Duration::from_millis(250),
        );
        let started_at = std::time::Instant::now();
        let result = SystemProcessRunner.run(request, &CancellationToken::default());

        assert!(matches!(result, Err(ProcessError::TimedOut(_))));
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(
            wait_for_absent_file(&temporary.path().join("marker.txt"), Duration::from_secs(3)),
            "el descendiente no debe sobrevivir al timeout"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cancellation_terminates_descendants_and_returns_promptly() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            descendant_command(),
            Duration::from_secs(10),
        );
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let started_at = std::time::Instant::now();
        let worker =
            std::thread::spawn(move || SystemProcessRunner.run(request, &worker_cancellation));

        let pid_path = temporary.path().join("child.pid");
        assert!(
            wait_for_file(&pid_path, Duration::from_secs(3)),
            "el fixture debe iniciar el descendiente antes de cancelar"
        );
        cancellation.cancel();
        let result = worker
            .join()
            .expect("el runner no debe dejar un worker colgado");

        assert!(matches!(result, Err(ProcessError::Cancelled)));
        assert!(started_at.elapsed() < Duration::from_secs(3));
        assert!(
            wait_for_absent_file(&temporary.path().join("marker.txt"), Duration::from_secs(3)),
            "el descendiente no debe sobrevivir a la cancelación"
        );
    }

    #[cfg(windows)]
    #[test]
    fn normal_parent_exit_does_not_wait_for_an_inherited_handle() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            normal_exit_descendant_command(),
            Duration::from_secs(10),
        );
        let started_at = std::time::Instant::now();
        let result = SystemProcessRunner.run(request, &CancellationToken::default());

        assert!(result.is_ok());
        assert!(started_at.elapsed() < Duration::from_secs(3));
        assert!(
            wait_for_file(&temporary.path().join("marker.txt"), Duration::from_secs(5)),
            "el descendiente heredado debe terminar sin bloquear al padre"
        );
    }

    #[cfg(windows)]
    #[test]
    fn repeated_cancellation_does_not_leave_descendant_processes() {
        for iteration in 0..8 {
            let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
            let request = write_fixture(
                temporary.path(),
                descendant_command(),
                Duration::from_secs(10),
            );
            let pid_path = temporary.path().join("child.pid");
            let cancellation = CancellationToken::default();
            let worker_cancellation = cancellation.clone();
            let worker =
                std::thread::spawn(move || SystemProcessRunner.run(request, &worker_cancellation));

            assert!(
                wait_for_file(&pid_path, Duration::from_secs(3)),
                "el fixture {iteration} no inició el descendiente"
            );
            let child_pid = fs::read_to_string(&pid_path)
                .expect("el fixture debe escribir el PID del descendiente")
                .trim()
                .parse::<u32>()
                .expect("el PID del descendiente debe ser numérico");
            cancellation.cancel();
            assert!(matches!(
                worker
                    .join()
                    .expect("el runner no debe dejar un worker colgado"),
                Err(ProcessError::Cancelled)
            ));
            assert_process_stopped(child_pid);
            assert!(!temporary.path().join("marker.txt").exists());
        }
    }

    #[cfg(windows)]
    fn write_fixture(directory: &Path, command: &str, timeout: Duration) -> ProcessRequest {
        let script = directory.join("process-tree-fixture.cmd");
        fs::write(&script, format!("@echo off\r\n{command}\r\n"))
            .expect("debe escribir el fixture de procesos");
        ProcessRequest {
            label: "process-tree-fixture",
            program: PathBuf::from("cmd.exe"),
            arguments: vec!["/D".into(), "/S".into(), "/C".into(), script.into()],
            environment: Vec::new(),
            removed_environment: Vec::new(),
            current_directory: Some(directory.to_path_buf()),
            stdin: None,
            timeout,
        }
    }

    #[cfg(windows)]
    fn descendant_command() -> &'static str {
        r#"powershell.exe -NoProfile -NonInteractive -Command "$PID | Set-Content -LiteralPath child.pid; Start-Sleep -Seconds 30; Set-Content -LiteralPath marker.txt survivor""#
    }

    #[cfg(windows)]
    fn normal_exit_descendant_command() -> &'static str {
        r#"start "" /B powershell.exe -NoProfile -NonInteractive -Command "$PID | Set-Content -LiteralPath child.pid; Start-Sleep -Seconds 2; Set-Content -LiteralPath marker.txt survivor""#
    }

    #[cfg(windows)]
    fn wait_for_file(path: &Path, timeout: Duration) -> bool {
        let started_at = std::time::Instant::now();
        while started_at.elapsed() < timeout {
            if path.is_file() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[cfg(windows)]
    fn wait_for_absent_file(path: &Path, timeout: Duration) -> bool {
        let started_at = std::time::Instant::now();
        while started_at.elapsed() < timeout {
            if !path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        !path.exists()
    }

    #[cfg(windows)]
    fn assert_process_stopped(process_id: u32) {
        let filter = format!("PID eq {process_id}");
        let started_at = std::time::Instant::now();
        while started_at.elapsed() < Duration::from_secs(3) {
            let output = std::process::Command::new("tasklist.exe")
                .args(["/FI", &filter, "/FO", "CSV", "/NH"])
                .output()
                .expect("tasklist debe estar disponible en Windows");
            let process_row = format!("\"{process_id}\"");
            if !String::from_utf8_lossy(&output.stdout).contains(&process_row) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("el proceso descendiente {process_id} sigue vivo tras la cancelación");
    }
}
