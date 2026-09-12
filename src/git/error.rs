use std::path::PathBuf;

use thiserror::Error;

use crate::process::ProcessError;

/// Motivo por el que Git rechazó una ruta concreta de un lote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathFailure {
    pub path: PathBuf,
    pub reason: String,
}

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
    #[error(
        "resultado parcial: {applied} de {requested} rutas aplicadas; {} fallaron",
        failures.len()
    )]
    PartialBatch {
        applied: usize,
        requested: usize,
        failures: Vec<PathFailure>,
    },
}

impl GitError {
    /// Devuelve los detalles técnicos que conviene mostrar expandibles.
    #[must_use]
    pub fn technical_details(&self) -> String {
        match self {
            Self::CommandFailed { stderr, .. } => stderr.clone(),
            Self::PartialBatch { failures, .. } => {
                use std::fmt::Write as _;

                let mut details = self.to_string();
                for failure in failures {
                    let _ = write!(details, "\n{}: {}", failure.path.display(), failure.reason);
                }
                details
            }
            _ => self.to_string(),
        }
    }

    /// Distingue una cancelación solicitada por el usuario de un fallo real.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Process(ProcessError::Cancelled)
                | Self::NotInstalled {
                    source: ProcessError::Cancelled
                }
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{GitError, PathFailure};

    #[test]
    fn partial_batch_reports_scope_and_every_failed_path() {
        let error = GitError::PartialBatch {
            applied: 2,
            requested: 3,
            failures: vec![PathFailure {
                path: PathBuf::from("ruta con ñ.txt"),
                reason: "pathspec did not match any files".to_owned(),
            }],
        };

        let message = error.to_string();
        let details = error.technical_details();

        assert!(message.contains("2 de 3"));
        assert!(message.contains("1 fallaron"));
        assert!(details.contains("ruta con ñ.txt"));
        assert!(details.contains("pathspec did not match any files"));
    }
}
