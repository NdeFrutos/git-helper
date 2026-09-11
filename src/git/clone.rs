use std::path::{Path, PathBuf};

use crate::process::CancellationToken;

use super::{GitClient, GitError, ParsedSshUrl, normalize_ssh_url};

/// Decisión sobre qué hacer con un destino de clonado ya presente en disco.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloneDestinationPlan {
    CloneInto(PathBuf),
    OpenExisting(PathBuf),
}

/// Resuelve si conviene clonar o reutilizar un directorio destino existente.
pub fn plan_clone_destination(
    client: &GitClient,
    parsed_url: &ParsedSshUrl,
    destination: &Path,
    cancellation: &CancellationToken,
) -> Result<CloneDestinationPlan, GitError> {
    if !destination.exists() {
        return Ok(CloneDestinationPlan::CloneInto(destination.to_path_buf()));
    }
    if destination
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_none())
    {
        return Ok(CloneDestinationPlan::CloneInto(destination.to_path_buf()));
    }

    let conflict = |message: &str| GitError::CloneDestinationConflict {
        path: destination.to_path_buf(),
        message: message.to_owned(),
    };
    let Ok(root_path) = client.discover_repository(destination, cancellation) else {
        return Err(conflict(
            "El destino existe y no es un repositorio Git vacío",
        ));
    };
    // `git rev-parse --show-toplevel` sube por el árbol: si la raíz encontrada no es el
    // propio destino, este solo está dentro de otro repositorio y no puede reutilizarse.
    if !is_same_directory(&root_path, destination) {
        return Err(conflict(
            "El destino existe y está dentro de otro repositorio Git",
        ));
    }
    let origin_url = match client.remote_url(&root_path, "origin", cancellation) {
        Ok(origin_url) => origin_url,
        Err(GitError::RemoteNotConfigured { .. }) => {
            return Err(conflict(
                "El destino ya es un repositorio Git sin remote origin",
            ));
        }
        Err(error) => return Err(error),
    };
    if remote_matches_requested_url(&parsed_url.normalized, &origin_url)? {
        Ok(CloneDestinationPlan::OpenExisting(root_path))
    } else {
        Err(conflict(
            "El destino ya es un repositorio Git con un remote origin distinto",
        ))
    }
}

/// Compara dos rutas resolviendo enlaces y prefijos para evitar falsos negativos.
fn is_same_directory(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Comprueba si una URL remota almacenada sigue siendo equivalente a la solicitada.
pub fn remote_matches_requested_url(
    requested_url: &str,
    remote_url: &str,
) -> Result<bool, GitError> {
    let requested = normalize_ssh_url(requested_url)?;
    // Un origin que no es SSH (HTTPS, ruta local…) no es un error del usuario: simplemente
    // no coincide, así que se compara en crudo en lugar de propagar el fallo de parseo.
    let Ok(remote) = normalize_ssh_url(remote_url) else {
        return Ok(requested == remote_url.trim());
    };
    Ok(requested == remote)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::process::CancellationToken;

    use super::*;

    fn initialize_repository(repository: &Path) {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["init", "-b", "main"])
            .status()
            .expect("git init debe funcionar");
    }

    #[test]
    fn plans_clone_into_missing_directory() {
        let temporary = tempdir().expect("tempdir");
        let destination = temporary.path().join("nuevo-clon");
        let parsed = ParsedSshUrl {
            original: "git@github.com:org/repo.git".to_owned(),
            normalized: "ssh://github.com/org/repo".to_owned(),
            host: "github.com".to_owned(),
            repository_path: "org/repo".to_owned(),
            repository_name: "repo".to_owned(),
        };
        let client = GitClient::default();
        let plan = plan_clone_destination(
            &client,
            &parsed,
            &destination,
            &CancellationToken::default(),
        )
        .expect("debe planificar clonado");

        assert_eq!(plan, CloneDestinationPlan::CloneInto(destination));
    }

    fn sample_parsed_url() -> ParsedSshUrl {
        ParsedSshUrl {
            original: "git@github.com:org/repo.git".to_owned(),
            normalized: "ssh://github.com/org/repo".to_owned(),
            host: "github.com".to_owned(),
            repository_path: "org/repo".to_owned(),
            repository_name: "repo".to_owned(),
        }
    }

    #[test]
    fn rejects_destination_nested_inside_another_repository() {
        let temporary = tempdir().expect("tempdir");
        initialize_repository(temporary.path());
        std::process::Command::new("git")
            .arg("-C")
            .arg(temporary.path())
            .args(["remote", "add", "origin", "git@github.com:org/repo.git"])
            .status()
            .expect("remote add");
        let nested = temporary.path().join("vendor");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(nested.join("readme.txt"), b"contenido").expect("write");

        let error = plan_clone_destination(
            &GitClient::default(),
            &sample_parsed_url(),
            &nested,
            &CancellationToken::default(),
        )
        .expect_err("un subdirectorio de otro repositorio no es reutilizable");

        assert!(matches!(error, GitError::CloneDestinationConflict { .. }));
    }

    #[test]
    fn reports_conflict_when_existing_origin_is_not_ssh() {
        let temporary = tempdir().expect("tempdir");
        let repository = temporary.path().join("repo");
        fs::create_dir_all(&repository).expect("mkdir");
        initialize_repository(&repository);
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["remote", "add", "origin", "https://github.com/org/repo.git"])
            .status()
            .expect("remote add");

        let error = plan_clone_destination(
            &GitClient::default(),
            &sample_parsed_url(),
            &repository,
            &CancellationToken::default(),
        )
        .expect_err("un origin HTTPS distinto debe ser conflicto de destino");

        assert!(matches!(error, GitError::CloneDestinationConflict { .. }));
    }

    #[test]
    fn remote_comparison_tolerates_non_ssh_remotes() {
        assert!(
            !remote_matches_requested_url(
                "git@github.com:org/repo.git",
                "https://github.com/org/repo.git"
            )
            .expect("no debe fallar por el esquema del remote")
        );
    }

    #[test]
    fn opens_existing_repository_with_matching_origin() {
        let temporary = tempdir().expect("tempdir");
        let repository = temporary.path().join("repo");
        fs::create_dir_all(&repository).expect("mkdir");
        initialize_repository(&repository);
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["remote", "add", "origin", "git@github.com:org/repo.git"])
            .status()
            .expect("remote add");
        let parsed = ParsedSshUrl {
            original: "ssh://git@github.com/org/repo".to_owned(),
            normalized: "ssh://github.com/org/repo".to_owned(),
            host: "github.com".to_owned(),
            repository_path: "org/repo".to_owned(),
            repository_name: "repo".to_owned(),
        };
        let client = GitClient::default();
        let plan =
            plan_clone_destination(&client, &parsed, &repository, &CancellationToken::default())
                .expect("debe reutilizar el clon");

        assert!(matches!(plan, CloneDestinationPlan::OpenExisting(_)));
    }
}
