use std::path::PathBuf;

use thiserror::Error;

use crate::process::ProcessError;

/// Errores tipados que la UI puede convertir en mensajes accionables.
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git no está instalado o no se puede ejecutar")]
    NotInstalled {
        #[source]
        source: ProcessError,
    },
    #[error("falló la ejecución de Git: {0}")]
    Process(#[from] ProcessError),
    #[error("no se pudo acceder a {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Git rechazó la operación: {stderr}")]
    CommandFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("Git devolvió texto que no es UTF-8 en {context}")]
    InvalidUtf8 { context: &'static str },
    #[error("no se pudo interpretar el estado Git: {message}")]
    InvalidStatus { message: String },
    #[error("no se pudo interpretar el historial Git: {message}")]
    InvalidLog { message: String },
    #[error("no se pudo interpretar el inventario de ramas Git: {message}")]
    InvalidBranches { message: String },
    #[error("la ruta no es segura para esta operación: {path:?}")]
    UnsafePath { path: PathBuf },
    #[error("la ruta no está dentro del repositorio: {path:?}")]
    PathOutsideRepository { path: PathBuf },
    #[error("el remote o la rama no son válidos: {value}")]
    InvalidReferenceName { value: String },
    #[error("la referencia Git ya no existe: {value}")]
    ReferenceNotFound { value: String },
    #[error("el mensaje de commit está vacío")]
    EmptyCommitMessage,
    #[error("un worker interno de Git terminó inesperadamente durante {operation}")]
    WorkerPanicked { operation: &'static str },
    #[error("la rama actual no tiene upstream")]
    MissingUpstream,
    #[error("se debe seleccionar uno de los remotes disponibles")]
    RemoteSelectionRequired { remotes: Vec<String> },
    #[error("el repositorio no tiene remotes configurados")]
    MissingRemote,
    #[error("HEAD está separado; esta operación no está disponible")]
    DetachedHead,
    #[error("no se puede descartar un conflicto desde Git Helper")]
    ConflictDiscardUnsupported,
    #[error("un cambio staged no se puede descartar antes del primer commit")]
    StagedDiscardWithoutHead,
    #[error("la ruta ya no es untracked y no se eliminará: {path:?}")]
    UntrackedStateChanged { path: PathBuf },
    #[error("el index cambió mientras se preparaba el contexto staged")]
    StagedStateChanged,
    #[error("URL SSH inválida: {message}")]
    InvalidSshUrl { message: String },
    #[error("esquema no soportado en esta versión: {scheme}")]
    UnsupportedUrlScheme { scheme: String },
    #[error("no se pudo determinar la carpeta local para clonar repositorios")]
    CloneRootUnavailable,
    #[error("no se puede usar {path:?}: {message}")]
    CloneDestinationConflict { path: PathBuf, message: String },
    #[error("autenticación SSH rechazada")]
    SshAuthenticationFailed { details: String },
    #[error("verificación de host SSH fallida")]
    SshHostKeyVerificationFailed { details: String },
    #[error("host SSH cambió; revisa known_hosts")]
    SshHostKeyChanged { details: String },
    #[error("no se pudo conectar al servidor Git por SSH")]
    SshConnectionFailed { details: String },
    #[error("el remote {remote} no está configurado")]
    RemoteNotConfigured { remote: String },
}

impl GitError {
    /// Devuelve los detalles técnicos que conviene mostrar expandibles.
    #[must_use]
    pub fn technical_details(&self) -> String {
        match self {
            Self::CommandFailed { stderr, .. } => stderr.clone(),
            Self::SshAuthenticationFailed { details }
            | Self::SshHostKeyVerificationFailed { details }
            | Self::SshHostKeyChanged { details }
            | Self::SshConnectionFailed { details } => details.clone(),
            _ => self.to_string(),
        }
    }

    /// Resume el error para la UI con indicaciones accionables cuando procede.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::InvalidSshUrl { message } | Self::CloneDestinationConflict { message, .. } => {
                message.clone()
            }
            Self::UnsupportedUrlScheme { scheme } => format!(
                "Solo se admiten URLs SSH en esta versión. El esquema «{scheme}» quedará para una futura ampliación."
            ),
            Self::CloneRootUnavailable => {
                "No se pudo determinar la carpeta local para clonar repositorios.".to_owned()
            }
            Self::SshAuthenticationFailed { .. } => "Autenticación SSH rechazada. Git Helper usa el SSH de Git for Windows (GIT_SSH, ~/.ssh/config y ssh-agent). Comprueba que tu clave esté cargada con ssh-add y autorizada en el servidor.".to_owned(),
            Self::SshHostKeyVerificationFailed { .. } => "No se pudo verificar la clave del host SSH. Conéctate una vez con ssh al servidor o añade su clave a ~/.ssh/known_hosts antes de clonar.".to_owned(),
            Self::SshHostKeyChanged { .. } => "La clave del host SSH cambió. Revisa ~/.ssh/known_hosts antes de volver a clonar; puede indicar un problema de seguridad.".to_owned(),
            Self::SshConnectionFailed { .. } => "No se pudo conectar al servidor Git por SSH. Comprueba host, puerto, red y firewall.".to_owned(),
            Self::RemoteNotConfigured { remote } => format!(
                "El repositorio no tiene configurado el remote «{remote}»."
            ),
            Self::NotInstalled { .. } => {
                "Git no está instalado o no se puede ejecutar.".to_owned()
            }
            Self::Process(ProcessError::Cancelled) => "Operación cancelada.".to_owned(),
            Self::Process(ProcessError::TimedOut(timeout)) => format!(
                "La operación superó el tiempo máximo de {timeout:?}."
            ),
            other => other.to_string(),
        }
    }
}

/// Convierte stderr de Git/SSH en errores más accionables para clonado remoto.
#[must_use]
pub fn classify_remote_failure(stderr: &str, exit_code: Option<i32>) -> GitError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("permission denied (publickey)")
        || lower.contains("permission denied, please try again")
    {
        return GitError::SshAuthenticationFailed {
            details: stderr.trim().to_owned(),
        };
    }
    if lower.contains("host key verification failed") || lower.contains("no matching host key") {
        return GitError::SshHostKeyVerificationFailed {
            details: stderr.trim().to_owned(),
        };
    }
    if lower.contains("remote host identification has changed")
        || lower.contains("host key changed")
    {
        return GitError::SshHostKeyChanged {
            details: stderr.trim().to_owned(),
        };
    }
    if lower.contains("connection timed out")
        || lower.contains("connection refused")
        || lower.contains("could not resolve hostname")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
    {
        return GitError::SshConnectionFailed {
            details: stderr.trim().to_owned(),
        };
    }
    GitError::CommandFailed {
        exit_code,
        stderr: stderr.trim().to_owned(),
    }
}
