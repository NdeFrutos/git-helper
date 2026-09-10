use std::{
    ffi::OsString,
    io::{Read, Write},
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
    #[error("un worker de comunicación terminó inesperadamente")]
    WorkerPanicked,
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
        let mut command = Command::new(&request.program);
        command
            .args(&request.arguments)
            .envs(request.environment.iter().cloned())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;

            command.creation_flags(CREATE_NO_WINDOW);
        }
        for variable in &request.removed_environment {
            command.env_remove(variable);
        }

        if request.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        if let Some(current_directory) = &request.current_directory {
            command.current_dir(current_directory);
        }

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::other("stdout no está disponible"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ProcessError::Spawn(std::io::Error::other("stderr no está disponible"))
        })?;

        let stdout_worker = thread::spawn(move || read_pipe(stdout));
        let stderr_worker = thread::spawn(move || read_pipe(stderr));
        let stdin_worker = request.stdin.map(|input| {
            let mut stdin = child.stdin.take();
            thread::spawn(move || {
                let Some(mut stdin) = stdin.take() else {
                    return Err(std::io::Error::other("stdin no está disponible"));
                };
                stdin.write_all(&input)?;
                drop(stdin);
                Ok(())
            })
        });

        let mut poll_interval = Duration::from_micros(150);
        let maximum_poll_interval = Duration::from_millis(25);
        let status = loop {
            if cancellation.is_cancelled() {
                terminate_child(&mut child)?;
                let _ = join_workers(stdout_worker, stderr_worker, stdin_worker);
                warn!(
                    operation = request.label,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "Proceso cancelado"
                );
                return Err(ProcessError::Cancelled);
            }
            if started_at.elapsed() >= request.timeout {
                terminate_child(&mut child)?;
                let _ = join_workers(stdout_worker, stderr_worker, stdin_worker);
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

        let (stdout, stderr) = join_workers(stdout_worker, stderr_worker, stdin_worker)?;
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

fn read_pipe(mut pipe: impl Read) -> Result<Vec<u8>, std::io::Error> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output)?;
    Ok(output)
}

fn terminate_child(child: &mut std::process::Child) -> Result<(), ProcessError> {
    if let Err(kill_error) = child.kill()
        && kill_error.kind() != std::io::ErrorKind::InvalidInput
    {
        return Err(ProcessError::Wait(kill_error));
    }
    child.wait().map_err(ProcessError::Wait)?;
    Ok(())
}

fn join_workers(
    stdout_worker: thread::JoinHandle<Result<Vec<u8>, std::io::Error>>,
    stderr_worker: thread::JoinHandle<Result<Vec<u8>, std::io::Error>>,
    stdin_worker: Option<thread::JoinHandle<Result<(), std::io::Error>>>,
) -> Result<(Vec<u8>, Vec<u8>), ProcessError> {
    if let Some(stdin_worker) = stdin_worker {
        stdin_worker
            .join()
            .map_err(|_| ProcessError::WorkerPanicked)?
            .map_err(ProcessError::Stdin)?;
    }
    let stdout = stdout_worker
        .join()
        .map_err(|_| ProcessError::WorkerPanicked)?
        .map_err(|source| ProcessError::ReadPipe {
            stream: "stdout",
            source,
        })?;
    let stderr = stderr_worker
        .join()
        .map_err(|_| ProcessError::WorkerPanicked)?
        .map_err(|source| ProcessError::ReadPipe {
            stream: "stderr",
            source,
        })?;
    Ok((stdout, stderr))
}
