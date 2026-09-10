use std::path::{Path, PathBuf};

/// Resuelve el ejecutable de Cursor CLI usando configuración, ubicaciones habituales y PATH.
#[must_use]
pub fn resolve_cursor_executable(configured: Option<PathBuf>) -> PathBuf {
    if let Some(path) = configured.filter(|path| path_exists(path)) {
        return path;
    }

    for candidate in default_candidates() {
        if path_exists(&candidate) {
            return candidate;
        }
    }

    PathBuf::from("agent")
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn default_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let cursor_agent = PathBuf::from(local_app_data).join("cursor-agent");
        if let Some(entrypoint) = latest_version_entrypoint(&cursor_agent) {
            candidates.push(entrypoint);
        }
        candidates.extend([
            cursor_agent.join("agent.cmd"),
            cursor_agent.join("agent.exe"),
            cursor_agent.join("agent"),
            cursor_agent.join("cursor-agent.cmd"),
        ]);
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        let cursor_bin = PathBuf::from(user_profile).join(".cursor").join("bin");
        candidates.extend([
            cursor_bin.join("agent.exe"),
            cursor_bin.join("agent.cmd"),
            cursor_bin.join("agent"),
        ]);
    }

    candidates
}

/// El launcher raíz busca actualizaciones antes de cada ejecución. La entrada de
/// la versión instalada salta ese trabajo, pero conserva el mismo CLI y auth.
pub(super) fn latest_version_entrypoint(installation: &Path) -> Option<PathBuf> {
    let versions = std::fs::read_dir(installation.join("versions")).ok()?;
    versions
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let version = entry.file_name().to_string_lossy().into_owned();
            let date = version.split_once('-')?.0;
            let mut parts = date.split('.');
            let year = parts.next()?.parse::<u32>().ok()?;
            let month = parts.next()?.parse::<u32>().ok()?;
            let day = parts.next()?.parse::<u32>().ok()?;
            if parts.next().is_some() {
                return None;
            }
            let command = entry.path().join("cursor-agent.cmd");
            command
                .is_file()
                .then_some(((year, month, day, version), command))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, command)| command)
}

#[cfg(test)]
mod tests {
    use super::{latest_version_entrypoint, resolve_cursor_executable};

    #[test]
    fn uses_the_version_entrypoint_instead_of_the_slow_update_launcher() {
        let installation = tempfile::tempdir().expect("debe crear la instalación simulada");
        let older = installation.path().join("versions").join("2026.08.20-aaaa");
        let current = installation.path().join("versions").join("2026.09.02-bbbb");
        std::fs::create_dir_all(&older).expect("debe crear la versión anterior");
        std::fs::create_dir_all(&current).expect("debe crear la versión actual");
        std::fs::write(older.join("cursor-agent.cmd"), b"")
            .expect("debe crear el ejecutable anterior");
        std::fs::write(current.join("cursor-agent.cmd"), b"")
            .expect("debe crear el ejecutable actual");

        let resolved = latest_version_entrypoint(installation.path());

        assert_eq!(
            resolved.as_deref(),
            Some(current.join("cursor-agent.cmd").as_path())
        );
    }

    #[test]
    fn prefers_configured_path_when_it_exists() {
        let configured = std::env::temp_dir().join("agent-test.exe");
        std::fs::write(&configured, b"").expect("debe crear el archivo");

        let resolved = resolve_cursor_executable(Some(configured.clone()));

        assert_eq!(resolved, configured);
    }

    #[test]
    fn ignores_missing_configured_path() {
        let missing = std::env::temp_dir().join("git-helper-missing-agent.exe");
        let resolved = resolve_cursor_executable(Some(missing));

        assert!(resolved.exists() || resolved.as_os_str() == "agent");
    }
}
