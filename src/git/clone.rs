use std::path::{Path, PathBuf};

use crate::process::CancellationToken;

use super::{GitClient, GitError, ParsedSshUrl, normalize_ssh_url, ssh_urls_equivalent};

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
    if destination.exists() {
        if destination
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_none())
        {
            return Ok(CloneDestinationPlan::CloneInto(destination.to_path_buf()));
        }
        match client.discover_repository(destination, cancellation) {
            Ok(root_path) => {
                let origin_url = client.remote_url(&root_path, "origin", cancellation)?;
                if ssh_urls_equivalent(&parsed_url.normalized, &origin_url)? {
                    Ok(CloneDestinationPlan::OpenExisting(root_path))
                } else {
                    Err(GitError::CloneDestinationConflict {
                        path: destination.to_path_buf(),
                        message:
                            "El destino ya es un repositorio Git con un remote origin distinto"
                                .to_owned(),
                    })
                }
            }
            Err(GitError::CommandFailed { .. }) => Err(GitError::CloneDestinationConflict {
                path: destination.to_path_buf(),
                message: "El destino existe y no es un repositorio Git vacío".to_owned(),
            }),
            Err(error) => Err(error),
        }
    } else if let Some(parent) = destination.parent() {
        if parent.exists() || parent.parent().is_some() {
            Ok(CloneDestinationPlan::CloneInto(destination.to_path_buf()))
        } else {
            Err(GitError::Io {
                path: parent.to_path_buf(),
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            })
        }
    } else {
        Ok(CloneDestinationPlan::CloneInto(destination.to_path_buf()))
    }
}

/// Comprueba si una URL remota almacenada sigue siendo equivalente a la solicitada.
pub fn remote_matches_requested_url(
    requested_url: &str,
    remote_url: &str,
) -> Result<bool, GitError> {
    if ssh_urls_equivalent(requested_url, remote_url)? {
        return Ok(true);
    }
    let requested = normalize_ssh_url(requested_url)?;
    let remote = normalize_ssh_url(remote_url).unwrap_or_else(|_| remote_url.to_owned());
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
