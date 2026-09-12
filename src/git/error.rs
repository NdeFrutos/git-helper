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
    #[error("la rama actual todavía no tiene commits; no se puede hacer push")]
    UnbornHead,
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
    #[error("falta la identidad de Git (user.name y user.email)")]
    MissingIdentity { details: String },
    #[error("el índice del repositorio está bloqueado")]
    IndexLocked { details: String },
    #[error("un hook de Git rechazó la operación")]
    HookRejected {
        hook: Option<String>,
        details: String,
    },
    #[error("el servidor Git rechazó las credenciales")]
    RemoteAuthenticationFailed { details: String },
    #[error("la rama actual no está publicada en ningún remote")]
    UpstreamNotPublished { details: String },
    #[error("la rama local y la remota divergieron")]
    DivergentBranches { details: String },
    #[error("el remote rechazó el push por tener commits nuevos")]
    PushRejected { details: String },
    #[error("la operación sobrescribiría cambios locales")]
    LocalChangesWouldBeOverwritten { details: String },
}

impl GitError {
    /// Devuelve los detalles técnicos que conviene mostrar expandibles.
    #[must_use]
    pub fn technical_details(&self) -> String {
        let raw = match self {
            Self::CommandFailed { stderr, .. } => stderr.clone(),
            Self::SshAuthenticationFailed { details }
            | Self::SshHostKeyVerificationFailed { details }
            | Self::SshHostKeyChanged { details }
            | Self::SshConnectionFailed { details }
            | Self::MissingIdentity { details }
            | Self::IndexLocked { details }
            | Self::HookRejected { details, .. }
            | Self::RemoteAuthenticationFailed { details }
            | Self::UpstreamNotPublished { details }
            | Self::DivergentBranches { details }
            | Self::PushRejected { details }
            | Self::LocalChangesWouldBeOverwritten { details } => details.clone(),
            _ => self.to_string(),
        };
        redact_credentials(&raw)
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
            Self::MissingIdentity { .. } => "Git no sabe con qué identidad firmar el commit: faltan «user.name» o «user.email».".to_owned(),
            Self::IndexLocked { .. } => "El índice del repositorio está bloqueado: otra operación Git lo está usando o quedó interrumpida.".to_owned(),
            Self::HookRejected { hook, .. } => hook.as_ref().map_or_else(
                || "Un hook de Git rechazó la operación.".to_owned(),
                |hook| format!("El hook «{hook}» rechazó la operación."),
            ),
            Self::RemoteAuthenticationFailed { .. } => "El servidor Git rechazó las credenciales o no hay ninguna disponible sin interacción.".to_owned(),
            Self::UpstreamNotPublished { .. } => "La rama actual todavía no está publicada en ningún remote.".to_owned(),
            Self::DivergentBranches { .. } => "La rama local y la remota divergieron: cada una tiene commits que la otra no tiene.".to_owned(),
            Self::PushRejected { .. } => "El remote rechazó el push porque contiene commits que tu rama local no tiene.".to_owned(),
            Self::LocalChangesWouldBeOverwritten { .. } => "La operación sobrescribiría cambios locales sin confirmar.".to_owned(),
            // La primera línea resume; el resto del stderr sigue disponible en
            // los detalles expandibles, que es donde no se recorta nada.
            Self::CommandFailed { stderr, .. } => stderr
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map_or_else(
                    || "Git rechazó la operación.".to_owned(),
                    |first_line| format!("Git rechazó la operación: {first_line}"),
                ),
            Self::Process(ProcessError::Cancelled) => "Operación cancelada.".to_owned(),
            Self::Process(ProcessError::TimedOut(timeout)) => format!(
                "La operación superó el tiempo máximo de {timeout:?}."
            ),
            other => other.to_string(),
        }
    }

    /// Siguiente acción segura que el usuario puede ejecutar tras el fallo.
    ///
    /// Devuelve `None` cuando el error no está clasificado: es preferible
    /// mostrar el detalle técnico que inventar una causa.
    #[must_use]
    pub fn recommended_action(&self) -> Option<String> {
        let action = match self {
            Self::NotInstalled { .. } => {
                "Instala Git for Windows, reinicia Git Helper y pulsa Actualizar."
            }
            Self::MissingIdentity { .. } => {
                "Configura la identidad fuera de Git Helper con «git config --global user.name \"Tu nombre\"» y «git config --global user.email \"tu@correo\"»; después repite el commit. Git Helper no cambia tu configuración."
            }
            Self::IndexLocked { .. } => {
                "Espera a que termine la otra operación Git o cierra el programa que la mantiene abierta y pulsa Actualizar. Si confirmas que no hay ninguna en curso, borra «.git/index.lock» tú mismo: Git Helper no lo elimina automáticamente."
            }
            Self::HookRejected { .. } => {
                "Revisa la salida del hook, corrige lo que indica y repite la operación. Tu mensaje de commit se conserva."
            }
            Self::SshAuthenticationFailed { .. } => {
                "Carga tu clave con ssh-add, comprueba que está autorizada en el servidor y reintenta."
            }
            Self::SshHostKeyVerificationFailed { .. } => {
                "Conéctate una vez por ssh al servidor o añade su clave a ~/.ssh/known_hosts y reintenta."
            }
            Self::SshHostKeyChanged { .. } => {
                "No reintentes hasta comprobar el cambio: revisa ~/.ssh/known_hosts y confirma la huella con quien administra el servidor."
            }
            Self::SshConnectionFailed { .. } => {
                "Comprueba host, puerto, red y firewall; después pulsa la operación de nuevo."
            }
            Self::RemoteAuthenticationFailed { .. } => {
                "Actualiza las credenciales fuera de Git Helper (Git Credential Manager o un token personal) y reintenta: los procesos se ejecutan sin terminal interactiva."
            }
            Self::MissingUpstream | Self::UpstreamNotPublished { .. } => {
                "Publica la rama con Push: creará el upstream sin forzar nada. Después pull y fetch usarán ese remote."
            }
            Self::DivergentBranches { .. } => {
                "Resuelve la divergencia fuera de Git Helper con merge o rebase y vuelve aquí. Git Helper solo hace pull fast-forward y nunca push forzado."
            }
            Self::PushRejected { .. } => {
                "Haz Fetch, revisa los commits del remote e intégralos fuera de Git Helper antes de repetir el push. No uses push forzado."
            }
            Self::LocalChangesWouldBeOverwritten { .. } => {
                "Haz commit de los cambios locales afectados o guárdalos fuera de Git Helper antes de repetir la operación: no se hace stash automático."
            }
            Self::MissingRemote => {
                "Añade un remote fuera de Git Helper con «git remote add origin <url>» y vuelve a intentarlo."
            }
            Self::RemoteSelectionRequired { remotes } => {
                return Some(format!(
                    "Elige explícitamente el remote de la operación: {}.",
                    remotes.join(", ")
                ));
            }
            Self::RemoteNotConfigured { remote } => {
                return Some(format!(
                    "Configura el remote «{remote}» fuera de Git Helper o elige uno de los existentes."
                ));
            }
            Self::DetachedHead => {
                "Cambia a una rama fuera de Git Helper antes de repetir la operación."
            }
            Self::UnbornHead => "Crea el primer commit en esta rama antes de publicarla.",
            Self::EmptyCommitMessage => "Escribe un mensaje de commit antes de confirmar.",
            Self::ConflictDiscardUnsupported => {
                "Resuelve el conflicto fuera de Git Helper; desde aquí no se descartan rutas en conflicto."
            }
            Self::StagedDiscardWithoutHead => {
                "Antes del primer commit puedes quitar el archivo del stage y eliminarlo tú mismo si procede."
            }
            Self::UntrackedStateChanged { .. } | Self::StagedStateChanged => {
                "Pulsa Actualizar y repite la acción sobre el estado actual."
            }
            Self::ReferenceNotFound { .. } => {
                "Pulsa Actualizar: la referencia cambió en disco desde la última lectura."
            }
            Self::Process(ProcessError::TimedOut(_)) => {
                "Pulsa Actualizar para ver el estado real antes de reintentar: la operación pudo completarse en parte."
            }
            Self::CloneRootUnavailable => {
                "Elige otra carpeta de destino para el clon y vuelve a intentarlo."
            }
            _ => return None,
        };
        Some(action.to_owned())
    }
}

/// Oculta credenciales embebidas en URLs antes de mostrar o copiar detalles.
#[must_use]
pub fn redact_credentials(text: &str) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_end) = rest.find("://") {
        let (head, tail) = rest.split_at(scheme_end + 3);
        let authority_end = tail
            .find(|character: char| character == '/' || character.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, remainder) = tail.split_at(authority_end);
        redacted.push_str(head);
        match authority.rfind('@') {
            // Solo se oculta `usuario:secreto@host`: un `git@host` de SSH no
            // lleva secreto y borrarlo empeoraría el diagnóstico.
            Some(at) if authority[..at].contains(':') => {
                redacted.push_str("***@");
                redacted.push_str(&authority[at + 1..]);
            }
            _ => redacted.push_str(authority),
        }
        rest = remainder;
    }
    redacted.push_str(rest);
    redacted
}

/// Convierte stderr de Git/SSH en errores más accionables para clonado remoto.
#[must_use]
pub fn classify_remote_failure(stderr: &str, exit_code: Option<i32>) -> GitError {
    classify_command_failure(stderr, exit_code)
}

/// Convierte el stderr de cualquier comando Git fallido en un error tipado.
///
/// La clasificación se apoya en tokens que Git no traduce (nombres de opción y
/// de configuración, rutas, nombres de hook y marcadores de rechazo) y solo
/// después en frases conocidas en inglés y español. Un stderr que no encaje se
/// conserva íntegro como `CommandFailed` en lugar de atribuirle una causa.
#[must_use]
pub fn classify_command_failure(stderr: &str, exit_code: Option<i32>) -> GitError {
    let details = stderr.trim().to_owned();
    let lower = details.to_ascii_lowercase();

    if lower.contains("index.lock") {
        return GitError::IndexLocked { details };
    }
    if let Some(hook) = detect_hook(&lower) {
        return GitError::HookRejected {
            hook: Some(hook),
            details,
        };
    }
    if lower.contains("hook declined") || lower.contains("hook rechaz") {
        return GitError::HookRejected {
            hook: None,
            details,
        };
    }
    if lower.contains("user.email")
        || lower.contains("user.name")
        || lower.contains("auto-detect email")
        || lower.contains("empty ident name")
    {
        return GitError::MissingIdentity { details };
    }
    if lower.contains("permission denied (publickey)")
        || lower.contains("permission denied, please try again")
    {
        return GitError::SshAuthenticationFailed { details };
    }
    if lower.contains("host key verification failed")
        || lower.contains("no matching host key")
        || lower.contains("verificación de la clave")
    {
        return GitError::SshHostKeyVerificationFailed { details };
    }
    if lower.contains("remote host identification has changed")
        || lower.contains("host key changed")
    {
        return GitError::SshHostKeyChanged { details };
    }
    if lower.contains("connection timed out")
        || lower.contains("connection refused")
        || lower.contains("could not resolve hostname")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
    {
        return GitError::SshConnectionFailed { details };
    }
    if lower.contains("git_terminal_prompt")
        || lower.contains("terminal prompts disabled")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("authentication failed")
        || lower.contains("invalid username or password")
        || lower.contains("http basic: access denied")
        || lower.contains("error: 401")
        || lower.contains("error: 403")
        || lower.contains("fallo la autenticación")
        || lower.contains("falló la autenticación")
    {
        return GitError::RemoteAuthenticationFailed { details };
    }
    if lower.contains("--set-upstream") || lower.contains("no upstream branch") {
        return GitError::UpstreamNotPublished { details };
    }
    if lower.contains("non-fast-forward")
        || lower.contains("fetch first")
        || lower.contains("updates were rejected")
        || lower.contains("actualizaciones fueron rechazadas")
    {
        return GitError::PushRejected { details };
    }
    if lower.contains("pull.rebase")
        || lower.contains("divergent branches")
        || lower.contains("ramas divergentes")
        || lower.contains("not possible to fast-forward")
        || lower.contains("no es posible hacer un avance rápido")
    {
        return GitError::DivergentBranches { details };
    }
    if lower.contains("would be overwritten by")
        || lower.contains("commit your changes or stash them")
        || lower.contains("serían sobrescritos")
        || lower.contains("confirma tus cambios o utiliza stash")
    {
        return GitError::LocalChangesWouldBeOverwritten { details };
    }

    GitError::CommandFailed {
        exit_code,
        stderr: details,
    }
}

/// Nombres de hook, estables en cualquier idioma de Git.
///
/// Git no añade ningún marcador propio cuando un hook local falla: solo se
/// reenvía la salida del hook. Por eso un hook que no se identifica deja el
/// error sin clasificar, con su salida íntegra, en lugar de atribuirle una
/// causa que no consta.
const HOOK_NAMES: [&str; 9] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-merge-commit",
    "pre-rebase",
    "pre-push",
    "pre-receive",
    "post-receive",
];

fn detect_hook(lower_stderr: &str) -> Option<String> {
    HOOK_NAMES
        .into_iter()
        .find(|hook| lower_stderr.contains(hook))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{GitError, classify_command_failure, redact_credentials};

    #[test]
    fn classifies_missing_identity_from_the_untranslated_config_keys() {
        let stderr = "Autor: identidad desconocida\n\n*** Dime quién eres.\n\nEjecuta\n\n  git config --global user.email \"tu@correo\"\n  git config --global user.name \"Tu nombre\"\n";

        let error = classify_command_failure(stderr, Some(128));

        assert!(matches!(error, GitError::MissingIdentity { .. }));
        let action = error
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("user.email"));
        assert!(error.technical_details().contains("git config --global"));
    }

    #[test]
    fn classifies_a_locked_index_without_offering_to_delete_it() {
        let error = classify_command_failure(
            "fatal: Unable to create '/repo/.git/index.lock': File exists.",
            Some(128),
        );

        assert!(matches!(error, GitError::IndexLocked { .. }));
        let action = error
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("Git Helper no lo elimina"));
    }

    #[test]
    fn classifies_hook_rejections_and_names_the_hook() {
        let error = classify_command_failure(
            ".git/hooks/pre-commit: la comprobación de formato falló\n",
            Some(1),
        );

        let GitError::HookRejected { hook, .. } = &error else {
            panic!("un rechazo de hook debe clasificarse como tal: {error:?}");
        };
        assert_eq!(hook.as_deref(), Some("pre-commit"));
        assert!(error.user_message().contains("pre-commit"));
        assert!(
            error
                .technical_details()
                .contains("la comprobación de formato falló")
        );
    }

    #[test]
    fn classifies_remote_authentication_without_asking_for_a_terminal() {
        for stderr in [
            "fatal: Authentication failed for 'https://example.invalid/repo.git/'",
            "fatal: could not read Username for 'https://example.invalid': terminal prompts disabled",
        ] {
            let error = classify_command_failure(stderr, Some(128));

            assert!(
                matches!(error, GitError::RemoteAuthenticationFailed { .. }),
                "{stderr} debería clasificarse como fallo de credenciales"
            );
        }
    }

    #[test]
    fn separates_missing_upstream_divergence_and_rejected_push() {
        let upstream = classify_command_failure(
            "fatal: The current branch feature has no upstream branch.\nTo push the current branch and set the remote as upstream, use\n\n    git push --set-upstream origin feature\n",
            Some(128),
        );
        assert!(matches!(upstream, GitError::UpstreamNotPublished { .. }));

        let divergent = classify_command_failure(
            "fatal: Need to specify how to reconcile divergent branches.",
            Some(128),
        );
        assert!(matches!(divergent, GitError::DivergentBranches { .. }));

        let rejected = classify_command_failure(
            " ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs",
            Some(1),
        );
        assert!(matches!(rejected, GitError::PushRejected { .. }));
        let action = rejected
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("Fetch"));
        assert!(action.to_lowercase().contains("no uses push forzado"));
    }

    #[test]
    fn classifies_local_changes_that_would_be_overwritten() {
        let error = classify_command_failure(
            "error: Your local changes to the following files would be overwritten by merge:\n\tsrc/lib.rs",
            Some(1),
        );

        assert!(matches!(
            error,
            GitError::LocalChangesWouldBeOverwritten { .. }
        ));
        let action = error
            .recommended_action()
            .expect("debe recomendar una acción");
        assert!(action.contains("no se hace stash automático"));
    }

    #[test]
    fn keeps_an_unknown_failure_verbatim_and_without_an_invented_cause() {
        let stderr = "error: algo inesperado ocurrió en el servidor";

        let error = classify_command_failure(stderr, Some(9));

        let GitError::CommandFailed {
            exit_code,
            stderr: detail,
        } = &error
        else {
            panic!("un stderr desconocido no debe clasificarse: {error:?}");
        };
        assert_eq!(*exit_code, Some(9));
        assert_eq!(detail, stderr);
        assert!(error.recommended_action().is_none());
    }

    #[test]
    fn an_unclassified_failure_summarizes_the_first_line_and_keeps_the_rest_expandable() {
        let error = classify_command_failure(
            "error: algo inesperado ocurrió\nsegunda línea con contexto",
            Some(9),
        );

        assert_eq!(
            error.user_message(),
            "Git rechazó la operación: error: algo inesperado ocurrió"
        );
        assert!(error.technical_details().contains("segunda línea"));
    }

    #[test]
    fn redacts_embedded_credentials_but_keeps_ssh_users() {
        let redacted = redact_credentials(
            "fatal: no se pudo acceder a https://usuario:secreto@example.invalid/repo.git y a ssh://git@example.invalid/repo.git",
        );

        assert!(!redacted.contains("secreto"));
        assert!(redacted.contains("https://***@example.invalid/repo.git"));
        assert!(redacted.contains("ssh://git@example.invalid/repo.git"));
    }

    #[test]
    fn technical_details_of_a_classified_error_are_redacted_too() {
        let error = classify_command_failure(
            "fatal: Authentication failed for 'https://usuario:secreto@example.invalid/repo.git'",
            Some(128),
        );

        assert!(!error.technical_details().contains("secreto"));
    }
}
