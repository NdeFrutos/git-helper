mod args;
mod endpoint;
mod instance;
mod path;

pub use args::{InternalStartupArgs, parse_internal_startup_args};
pub use instance::{
    ForwardError, InstanceRequest, InstanceRequestReceiver, InstanceServer,
    forward_to_running_instance,
};

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use args::{help_text, parse_cli_args, wants_help};
use path::{CliPathError, resolve_repository_path};

use crate::git::GitClient;

/// Punto de entrada de `ghelper.exe`.
pub fn run_cli<I, S>(arguments: I) -> ExitCode
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();

    if wants_help(&arguments) {
        print!("{}", help_text());
        return ExitCode::SUCCESS;
    }

    let parsed = match parse_cli_args(arguments.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("Error: {error}");
            eprintln!();
            eprintln!("Usa ghelper --help para ver la sintaxis.");
            return ExitCode::from(2);
        }
    };

    let resolved_repository = match parsed.repository_path.as_deref() {
        Some(path) => match resolve_repository_path(Path::new(path), &GitClient::default()) {
            Ok(root_path) => Some(root_path),
            Err(error) => {
                eprintln!("{error}");
                return cli_exit_code_for_path_error(&error);
            }
        },
        None => None,
    };

    let request = match &resolved_repository {
        Some(path) => InstanceRequest::OpenRepository(path.clone()),
        None => InstanceRequest::Activate,
    };
    match forward_to_running_instance(&request) {
        Ok(()) => return ExitCode::SUCCESS,
        Err(ForwardError::NoInstance(_)) => {}
        Err(error) => {
            // No hay instancia utilizable: se arranca una nueva en lugar de fallar.
            eprintln!("Aviso: {error}. Se iniciará una nueva ventana de Git Helper.");
        }
    }

    if let Err(error) = launch_git_helper(resolved_repository) {
        eprintln!("No se pudo iniciar Git Helper: {error}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// Intenta reenviar una solicitud a otra instancia.
///
/// Devuelve `false` si no hay instancia publicada o si no confirma la solicitud,
/// de modo que la llamada continúe abriendo su propia ventana.
#[must_use]
pub fn try_forward_or_continue(request: &InstanceRequest) -> bool {
    forward_to_running_instance(request).is_ok()
}

fn launch_git_helper(open_repository: Option<PathBuf>) -> std::io::Result<()> {
    let executable = locate_git_helper_executable()?;
    let mut command = Command::new(&executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = open_repository {
        command.arg("--open-repo").arg(path);
    }
    command.spawn()?;
    Ok(())
}

fn locate_git_helper_executable() -> std::io::Result<PathBuf> {
    let current_executable = std::env::current_exe()?;
    let install_directory = current_executable
        .parent()
        .ok_or_else(|| std::io::Error::other("no se pudo resolver el directorio del ejecutable"))?;
    let candidate = install_directory.join("git-helper.exe");
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "no se encontró git-helper.exe junto a {}",
            current_executable.display()
        ),
    ))
}

fn cli_exit_code_for_path_error(error: &CliPathError) -> ExitCode {
    match error {
        CliPathError::NotFound { .. } | CliPathError::Git(_) => ExitCode::from(1),
    }
}
