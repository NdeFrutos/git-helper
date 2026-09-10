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
    #[error("la ruta no es segura para esta operación: {path:?}")]
    UnsafePath { path: PathBuf },
    #[error("la ruta no está dentro del repositorio: {path:?}")]
    PathOutsideRepository { path: PathBuf },
    #[error("el remote o la rama no son válidos: {value}")]
    InvalidReferenceName { value: String },
    #[error("el mensaje de commit está vacío")]
    EmptyCommitMessage,
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
}

impl GitError {
    /// Devuelve los detalles técnicos que conviene mostrar expandibles.
    #[must_use]
    pub fn technical_details(&self) -> String {
        match self {
            Self::CommandFailed { stderr, .. } => stderr.clone(),
            _ => self.to_string(),
        }
    }
}
