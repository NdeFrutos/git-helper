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
    #[error("no se pudo preparar el entorno aislado para Cursor: {0}")]
    IsolatedWorkspace(#[source] std::io::Error),
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

impl CursorError {
    /// Detalles técnicos del proveedor, sin credenciales embebidas.
    #[must_use]
    pub fn technical_details(&self) -> String {
        let raw = match self {
            Self::CommandFailed { stderr, .. } => stderr.clone(),
            Self::ResultError { message } => message.clone(),
            other => other.to_string(),
        };
        crate::git::redact_credentials(&raw)
    }

    /// Explicación breve del fallo para la banda de error.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::NotInstalled { .. } => {
                "Cursor CLI no está instalado o no se puede ejecutar.".to_owned()
            }
            Self::Process(ProcessError::Cancelled) => {
                "Generación cancelada; se conserva el borrador.".to_owned()
            }
            Self::Process(ProcessError::TimedOut(timeout)) => {
                format!("Cursor CLI no respondió en {timeout:?}.")
            }
            Self::IsolatedWorkspace(_) => {
                "No se pudo preparar el entorno aislado para Cursor CLI.".to_owned()
            }
            Self::CommandFailed { .. } => {
                if is_provider_authentication_failure(&self.technical_details()) {
                    "Cursor CLI rechazó la solicitud por autenticación o permisos.".to_owned()
                } else {
                    "Cursor CLI rechazó la solicitud.".to_owned()
                }
            }
            Self::InvalidUtf8 | Self::InvalidJson(_) | Self::MissingResult => {
                "Cursor CLI devolvió una respuesta que no se puede interpretar.".to_owned()
            }
            Self::EmptyResult => "Cursor CLI devolvió un mensaje vacío.".to_owned(),
            Self::ResultError { .. } => "Cursor CLI devolvió un error.".to_owned(),
            Self::NoStagedChanges => {
                "No hay cambios staged con los que generar un mensaje.".to_owned()
            }
            Self::Process(_) => self.to_string(),
        }
    }

    /// Siguiente acción segura tras un fallo del proveedor de IA.
    ///
    /// Devuelve `None` cuando no se reconoce la causa: el borrador se conserva
    /// y no conviene sugerir una reparación inventada.
    #[must_use]
    pub fn recommended_action(&self) -> Option<String> {
        let action = match self {
            Self::NotInstalled { .. } => {
                "Instala Cursor CLI y asegúrate de que está en el PATH, o escribe el mensaje a mano: el commit no depende de la generación."
            }
            Self::Process(ProcessError::TimedOut(_)) => {
                "Tu borrador se conserva. Pulsa «Generar con Cursor» para reintentar o escribe el mensaje a mano; no se ha tocado el repositorio."
            }
            Self::IsolatedWorkspace(_) => {
                "Comprueba el espacio y los permisos de la carpeta temporal del sistema y reintenta."
            }
            Self::CommandFailed { .. } => {
                if is_provider_authentication_failure(&self.technical_details()) {
                    "Inicia sesión en Cursor CLI fuera de Git Helper y reintenta; desde aquí no se piden credenciales."
                } else {
                    "Revisa los detalles del proveedor y reintenta; si persiste, escribe el mensaje a mano."
                }
            }
            Self::InvalidUtf8 | Self::InvalidJson(_) | Self::MissingResult | Self::EmptyResult => {
                "Reintenta la generación; si vuelve a fallar, escribe el mensaje a mano. Tu borrador no se ha modificado."
            }
            Self::NoStagedChanges => {
                "Prepara al menos un cambio en el stage y vuelve a generar el mensaje."
            }
            // Una cancelación no es un fallo que reparar y el error propio del
            // proveedor ya viaja en los detalles: no se inventa un siguiente paso.
            Self::ResultError { .. } | Self::Process(_) => return None,
        };
        Some(action.to_owned())
    }
}

/// Reconoce fallos de sesión o permisos del proveedor sin depender del idioma
/// del mensaje: se buscan códigos y términos que las CLI no traducen.
fn is_provider_authentication_failure(details: &str) -> bool {
    let lower = details.to_ascii_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("unauthenticated")
        || lower.contains("not logged in")
        || lower.contains("login")
        || lower.contains("api key")
        || lower.contains("forbidden")
}

#[cfg(test)]
mod tests {
    use super::CursorError;
    use crate::process::ProcessError;

    #[test]
    fn a_cancelled_generation_keeps_the_draft_and_invents_no_repair() {
        let error = CursorError::Process(ProcessError::Cancelled);

        assert!(error.user_message().contains("se conserva el borrador"));
        assert!(error.recommended_action().is_none());
    }

    #[test]
    fn a_provider_session_failure_points_at_the_provider_not_at_git() {
        let error = CursorError::CommandFailed {
            exit_code: Some(1),
            stderr: "error: 401 Unauthorized".to_owned(),
        };

        assert!(error.user_message().contains("autenticación"));
        let action = error
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("Inicia sesión"));
    }

    #[test]
    fn an_unusable_response_offers_to_retry_or_write_the_message_by_hand() {
        let error = CursorError::MissingResult;

        let action = error
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("a mano"));
        assert!(action.contains("borrador no se ha modificado"));
    }

    #[test]
    fn provider_details_are_redacted_before_being_shown() {
        let error = CursorError::CommandFailed {
            exit_code: Some(1),
            stderr: "no se pudo usar https://usuario:secreto@example.invalid/api".to_owned(),
        };

        let details = error.technical_details();

        assert!(!details.contains("secreto"));
        assert!(details.contains("https://***@example.invalid/api"));
    }

    #[test]
    fn an_unclassified_provider_error_keeps_its_message_without_guessing() {
        let error = CursorError::ResultError {
            message: "el modelo devolvió un error interno".to_owned(),
        };

        assert_eq!(
            error.technical_details(),
            "el modelo devolvió un error interno"
        );
        assert!(error.recommended_action().is_none());
    }
}
