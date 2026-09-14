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
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
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

/// Visibilidad de consola solicitada al abrir una aplicación externa.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleVisibility {
    /// Aplicaciones gráficas: no deben mostrar ninguna consola.
    Hidden,
    /// Terminales pedidas por el usuario: necesitan su propia consola visible.
    Visible,
}

/// Solicitud para abrir una aplicación externa sin esperar a que termine.
///
/// Los argumentos ya vienen resueltos como cadenas del sistema; nunca se
/// construye una línea de comandos ni se delega en una shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub label: &'static str,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub current_directory: Option<PathBuf>,
    pub console: ConsoleVisibility,
}

/// Abre una aplicación externa y devuelve el control de inmediato.
///
/// Solo interesa si el proceso pudo crearse: el hijo sobrevive a Git Helper y su
/// salida no se captura, así que no hay pipes que vaciar ni espera que bloquee.
pub fn launch_detached(request: &LaunchRequest) -> Result<(), ProcessError> {
    let mut command = Command::new(&request.program);
    command.args(&request.arguments);
    if let Some(current_directory) = &request.current_directory {
        command.current_dir(current_directory);
    }
    match request.console {
        ConsoleVisibility::Hidden => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        // Una terminal necesita los descriptores estándar de la consola nueva. Git
        // Helper es una aplicación gráfica sin consola propia, así que dejar la
        // herencia por defecto entrega tres handles nulos y Windows aplica su
        // comportamiento estándar: conectar el hijo a la consola que acaba de
        // crear. Redirigir a NUL, en cambio, dejaría la ventana abierta con una
        // shell que lee EOF y se cierra al instante.
        ConsoleVisibility::Visible => {}
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        command.creation_flags(match request.console {
            ConsoleVisibility::Hidden => CREATE_NO_WINDOW,
            ConsoleVisibility::Visible => CREATE_NEW_CONSOLE,
        });
    }

    let child = command.spawn().map_err(ProcessError::Spawn)?;
    debug!(
        operation = request.label,
        program = %request.program.display(),
        "Aplicación externa abierta"
    );
    release_child(child, request.label);
    Ok(())
}

/// Windows libera el proceso al cerrar su handle; el hijo sigue vivo por su cuenta.
#[cfg(target_os = "windows")]
fn release_child(child: std::process::Child, _label: &'static str) {
    drop(child);
}

/// En Unix el hijo quedaría en zombi hasta que termine Git Helper: un hilo
/// bloqueado en `wait` lo recoge sin retrasar a quien pidió la acción.
#[cfg(not(target_os = "windows"))]
fn release_child(mut child: std::process::Child, label: &'static str) {
    let supervisor = thread::Builder::new()
        .name("external-launch-reaper".to_owned())
        .spawn(move || {
            let _ = child.wait();
            debug!(operation = label, "Aplicación externa finalizada");
        });
    if let Err(error) = supervisor {
        warn!(
            operation = label,
            error = %error,
            "No se pudo supervisar la aplicación externa"
        );
    }
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
    // Un fallo al terminar el árbol no se propaga por sí solo: `taskkill.exe`
    // también devuelve error cuando el hijo acaba de terminar por su cuenta, y
    // convertir esa carrera en un error borraría la clasificación de
    // cancelación o timeout que el llamador está a punto de devolver. El caso
    // que sí importa —un proceso que sigue vivo— lo detecta la espera acotada.
    #[cfg(target_os = "windows")]
    if child.try_wait().map_err(ProcessError::Wait)?.is_none()
        && let Err(tree_error) = terminate_process_tree(child.id())
    {
        warn!(
            error = %tree_error,
            "no se pudo terminar el árbol de procesos; se continúa con el hijo directo"
        );
    }

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
    use std::{
        fs,
        path::Path,
        process::{Command, Stdio},
    };

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

    /// Margen para que el descendiente arranque y publique su PID antes de que
    /// el timeout termine el árbol; si muriera antes, la prueba no comprobaría
    /// nada.
    #[cfg(windows)]
    const TIMEOUT_FIXTURE_LIMIT: Duration = Duration::from_secs(3);

    #[cfg(windows)]
    #[test]
    fn timeout_terminates_descendants_that_inherit_output_handles() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            false,
            SURVIVING_DESCENDANT_LIFETIME,
            TIMEOUT_FIXTURE_LIMIT,
        );
        let started_at = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            SystemProcessRunner.run(request, &CancellationToken::default())
        });

        let descendant_pid =
            read_descendant_pid(&temporary.path().join("child.pid"), Duration::from_secs(3))
                .unwrap_or_else(|| {
                    panic!(
                        "el fixture debe publicar el PID del descendiente; {}",
                        fixture_diagnostics(temporary.path())
                    )
                });
        let result = worker
            .join()
            .expect("el runner no debe dejar un worker colgado");

        assert!(matches!(result, Err(ProcessError::TimedOut(_))));
        assert!(started_at.elapsed() < Duration::from_secs(8));
        assert_process_stopped(descendant_pid);
    }

    #[cfg(windows)]
    #[test]
    fn cancellation_terminates_descendants_and_returns_promptly() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            false,
            SURVIVING_DESCENDANT_LIFETIME,
            Duration::from_secs(10),
        );
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let started_at = std::time::Instant::now();
        let worker =
            std::thread::spawn(move || SystemProcessRunner.run(request, &worker_cancellation));

        let descendant_pid =
            read_descendant_pid(&temporary.path().join("child.pid"), Duration::from_secs(3))
                .unwrap_or_else(|| {
                    panic!(
                        "el fixture debe publicar el PID antes de cancelar; {}",
                        fixture_diagnostics(temporary.path())
                    )
                });
        cancellation.cancel();
        let result = worker
            .join()
            .expect("el runner no debe dejar un worker colgado");

        assert!(matches!(result, Err(ProcessError::Cancelled)));
        assert!(started_at.elapsed() < Duration::from_secs(5));
        assert_process_stopped(descendant_pid);
    }

    #[cfg(windows)]
    #[test]
    fn normal_parent_exit_does_not_wait_for_an_inherited_handle() {
        let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
        let request = write_fixture(
            temporary.path(),
            true,
            Duration::from_secs(2),
            Duration::from_secs(10),
        );
        let started_at = std::time::Instant::now();
        let result = SystemProcessRunner.run(request, &CancellationToken::default());

        assert!(result.is_ok());
        assert!(started_at.elapsed() < Duration::from_secs(3));
        assert!(
            wait_for_file(
                &temporary.path().join("marker.txt"),
                Duration::from_secs(10)
            ),
            "el descendiente heredado debe terminar sin bloquear al padre; {}",
            fixture_diagnostics(temporary.path())
        );
    }

    #[cfg(windows)]
    #[test]
    fn terminating_a_child_that_already_exited_is_not_an_infrastructure_error() {
        let mut child = Command::new("cmd.exe")
            .args(["/D", "/S", "/C", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("debe iniciar el proceso de prueba");
        while child
            .try_wait()
            .expect("debe poder consultar el estado del proceso")
            .is_none()
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        // Terminar un proceso que ya acabó es la carrera habitual al cancelar:
        // no debe degradarse a un error que oculte Cancelled o TimedOut.
        assert!(super::terminate_child(&mut child).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn repeated_cancellation_does_not_leave_descendant_processes() {
        for iteration in 0..8 {
            let temporary = tempfile::tempdir().expect("debe crear el directorio temporal");
            let request = write_fixture(
                temporary.path(),
                false,
                SURVIVING_DESCENDANT_LIFETIME,
                Duration::from_secs(10),
            );
            let pid_path = temporary.path().join("child.pid");
            let cancellation = CancellationToken::default();
            let worker_cancellation = cancellation.clone();
            let worker =
                std::thread::spawn(move || SystemProcessRunner.run(request, &worker_cancellation));

            let child_pid =
                read_descendant_pid(&pid_path, Duration::from_secs(3)).unwrap_or_else(|| {
                    panic!(
                        "el fixture {iteration} no publicó el PID; {}",
                        fixture_diagnostics(temporary.path())
                    )
                });
            cancellation.cancel();
            assert!(matches!(
                worker
                    .join()
                    .expect("el runner no debe dejar un worker colgado"),
                Err(ProcessError::Cancelled)
            ));
            assert_process_stopped(child_pid);
        }
    }

    /// Directorio donde el descendiente publica su PID y su marca de final.
    #[cfg(windows)]
    const FIXTURE_DIRECTORY_VARIABLE: &str = "GIT_HELPER_PROCESS_FIXTURE_DIR";
    /// Milisegundos que el descendiente permanece vivo antes de marcar su final.
    #[cfg(windows)]
    const FIXTURE_LIFETIME_VARIABLE: &str = "GIT_HELPER_PROCESS_FIXTURE_MS";
    #[cfg(windows)]
    const FIXTURE_DESCENDANT_TEST: &str = "process::tests::process_tree_fixture_descendant";
    /// Vida del descendiente que debe sobrevivir a su padre y ser terminado
    /// por el runner: más larga que cualquier plazo de las pruebas.
    #[cfg(windows)]
    const SURVIVING_DESCENDANT_LIFETIME: Duration = Duration::from_secs(30);

    /// Descendiente de las pruebas de árbol de procesos.
    ///
    /// El propio binario de pruebas hace de fixture. Depender de
    /// `powershell.exe` dejaba el resultado en manos del arranque del
    /// intérprete, que en CI tarda lo suficiente como para que la prueba no
    /// llegue a comprobar nada. Sin la variable de entorno no hace nada, así
    /// que en una ejecución normal de la suite es un test vacío.
    #[cfg(windows)]
    #[test]
    fn process_tree_fixture_descendant() {
        let Ok(directory) = std::env::var(FIXTURE_DIRECTORY_VARIABLE) else {
            return;
        };
        let directory = PathBuf::from(directory);
        let lifetime = std::env::var(FIXTURE_LIFETIME_VARIABLE)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(SURVIVING_DESCENDANT_LIFETIME, Duration::from_millis);
        fs::write(directory.join("child.pid"), std::process::id().to_string())
            .expect("el descendiente debe publicar su PID");
        std::thread::sleep(lifetime);
        let _ = fs::write(directory.join("marker.txt"), "survivor");
    }

    /// Prepara un `cmd.exe` que lanza el descendiente heredando sus handles.
    ///
    /// Con `detached`, `cmd.exe` termina de inmediato y deja al descendiente
    /// vivo con los handles de salida heredados.
    #[cfg(windows)]
    fn write_fixture(
        directory: &Path,
        detached: bool,
        lifetime: Duration,
        timeout: Duration,
    ) -> ProcessRequest {
        let descendant = std::env::current_exe().expect("el binario de pruebas debe existir");
        let launch = format!(
            r#""{}" {FIXTURE_DESCENDANT_TEST} --exact --nocapture"#,
            descendant.display()
        );
        let command = if detached {
            format!(r#"start "" /B {launch}"#)
        } else {
            launch
        };
        let script = directory.join("process-tree-fixture.cmd");
        fs::write(&script, format!("@echo off\r\n{command}\r\n"))
            .expect("debe escribir el fixture de procesos");
        ProcessRequest {
            label: "process-tree-fixture",
            program: PathBuf::from("cmd.exe"),
            arguments: vec!["/D".into(), "/S".into(), "/C".into(), script.into()],
            environment: vec![
                (
                    FIXTURE_DIRECTORY_VARIABLE.into(),
                    directory.as_os_str().to_owned(),
                ),
                (
                    FIXTURE_LIFETIME_VARIABLE.into(),
                    lifetime.as_millis().to_string().into(),
                ),
            ],
            removed_environment: Vec::new(),
            current_directory: Some(directory.to_path_buf()),
            stdin: None,
            timeout,
        }
    }

    /// Contenido del directorio del fixture, para que un fallo en CI diga si
    /// llegó a arrancar el descendiente o no.
    #[cfg(windows)]
    fn fixture_diagnostics(directory: &Path) -> String {
        fs::read_dir(directory).map_or_else(
            |error| format!("no se pudo listar {}: {error}", directory.display()),
            |entries| {
                let names: Vec<String> = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect();
                format!("contenido del fixture: {names:?}")
            },
        )
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

    /// Lee el PID publicado por el fixture reintentando hasta el plazo dado.
    ///
    /// La existencia del archivo no basta: `Set-Content` puede mantenerlo
    /// abierto y vacío mientras el test intenta leerlo.
    #[cfg(windows)]
    fn read_descendant_pid(path: &Path, timeout: Duration) -> Option<u32> {
        let started_at = std::time::Instant::now();
        loop {
            if let Ok(contents) = fs::read_to_string(path)
                && let Ok(process_id) = contents.trim().parse::<u32>()
            {
                return Some(process_id);
            }
            if started_at.elapsed() >= timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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
