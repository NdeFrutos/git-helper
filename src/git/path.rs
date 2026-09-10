use std::path::{Component, Path, PathBuf};

use super::GitError;

/// Rechaza pathspecs vacíos, absolutos o capaces de escapar del repositorio.
pub fn validate_relative_path(path: &Path) -> Result<PathBuf, GitError> {
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(GitError::UnsafePath {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    if !has_normal_component {
        return Err(GitError::UnsafePath {
            path: path.to_path_buf(),
        });
    }
    Ok(path.to_path_buf())
}

/// Comprueba físicamente una ruta existente antes de eliminarla.
pub fn validate_existing_path_inside_repository(
    repository_root: &Path,
    relative_path: &Path,
) -> Result<PathBuf, GitError> {
    let relative_path = validate_relative_path(relative_path)?;
    let canonical_root = repository_root
        .canonicalize()
        .map_err(|source| GitError::Io {
            path: repository_root.to_path_buf(),
            source,
        })?;
    let full_path = repository_root.join(&relative_path);
    let canonical_path = full_path.canonicalize().map_err(|source| GitError::Io {
        path: full_path,
        source,
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(GitError::PathOutsideRepository {
            path: canonical_path,
        });
    }
    Ok(relative_path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::validate_relative_path;

    #[test]
    fn rejects_empty_absolute_and_parent_paths() {
        assert!(validate_relative_path(Path::new("")).is_err());
        assert!(validate_relative_path(Path::new("../secreto.txt")).is_err());
        assert!(validate_relative_path(Path::new(r"C:\secreto.txt")).is_err());
    }

    #[test]
    fn accepts_spaces_unicode_and_leading_dashes() {
        assert!(validate_relative_path(Path::new("ruta con ñ/-archivo.txt")).is_ok());
    }
}
