use thiserror::Error;

use crate::process::ProcessError;

/// Fallos aislados de la integración opcional con Cursor CLI.
#[derive(Debug, Error)]
pub enum CursorError {
    #[error("Cursor CLI no está instalado o no se puede ejecutar")]
    NotInstalled {
        #[source]
        source: ProcessError,
    },
    #[error("falló el proceso de Cursor CLI: {0}")]
    Process(#[from] ProcessError),
    #[error("Cursor CLI rechazó la solicitud: {stderr}")]
    CommandFailed {
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("Cursor CLI devolvió texto que no es UTF-8")]
    InvalidUtf8,
    #[error("Cursor CLI devolvió JSON no válido: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("Cursor CLI no devolvió un evento de tipo result")]
    MissingResult,
    #[error("Cursor CLI devolvió un error: {message}")]
    ResultError { message: String },
    #[error("Cursor CLI devolvió un mensaje vacío")]
    EmptyResult,
    #[error("no hay cambios staged para generar un mensaje")]
    NoStagedChanges,
}
