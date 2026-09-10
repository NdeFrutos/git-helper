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

#[cfg(test)]
mod tests {
    use super::resolve_cursor_executable;

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
