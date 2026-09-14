use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{git::GitClient, process::CancellationToken};

/// Errores al resolver una ruta de repositorio desde la CLI.
#[derive(Debug, Error)]
pub enum CliPathError {
    #[error("la ruta no existe: {path}")]
    NotFound { path: PathBuf },
    #[error("{0}")]
    Git(#[from] crate::git::GitError),
}

/// Comprueba que la ruta existe y resuelve la raíz del repositorio Git.
pub fn resolve_repository_path(
    selected_path: &Path,
    git_client: &GitClient,
) -> Result<PathBuf, CliPathError> {
    if !selected_path.exists() {
        return Err(CliPathError::NotFound {
            path: selected_path.to_path_buf(),
        });
    }

    let cancellation = CancellationToken::default();
    let root_path = git_client.discover_repository(selected_path, &cancellation)?;
    Ok(root_path)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::tempdir;

    use super::resolve_repository_path;
    use crate::git::GitClient;

    #[test]
    fn rejects_missing_paths() {
        let client = GitClient::default();
        let error = resolve_repository_path(Path::new("ruta-inexistente"), &client).unwrap_err();
        assert!(error.to_string().contains("no existe"));
    }

    #[test]
    fn resolves_repository_root_from_subdirectory() {
        let temporary = tempdir().expect("debe crear el directorio temporal");
        let repository = temporary.path();
        std::process::Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .current_dir(repository)
            .output()
            .expect("Git debe estar disponible para la prueba");
        let nested = repository.join("src");
        fs::create_dir_all(&nested).expect("debe crear el subdirectorio");

        let client = GitClient::default();
        let resolved = resolve_repository_path(&nested, &client).expect("debe resolver el repo");
        assert_eq!(
            resolved.canonicalize().expect("debe canonicalizar"),
            repository.canonicalize().expect("debe canonicalizar")
        );
    }
}
