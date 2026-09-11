use std::path::{Path, PathBuf};

use directories::BaseDirs;

use super::GitError;

/// URL SSH parseada y lista para clonar o comparar con un remote existente.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedSshUrl {
    pub original: String,
    pub normalized: String,
    pub host: String,
    pub repository_path: String,
    pub repository_name: String,
}

const UNSUPPORTED_SCHEMES: &[&str] = &["https://", "http://", "file://", "git://", "ftp://"];

/// Valida y parsea una URL SSH en los formatos habituales de Git.
pub fn parse_ssh_url(input: &str) -> Result<ParsedSshUrl, GitError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(GitError::InvalidSshUrl {
            message: "La URL SSH no puede estar vacía".to_owned(),
        });
    }
    let lower = trimmed.to_ascii_lowercase();
    for scheme in UNSUPPORTED_SCHEMES {
        if lower.starts_with(scheme) {
            return Err(GitError::UnsupportedUrlScheme {
                scheme: scheme.trim_end_matches("://").to_owned(),
            });
        }
    }

    let parsed = if lower.starts_with("ssh://") {
        parse_ssh_scheme(trimmed)?
    } else {
        parse_scp_style(trimmed)?
    };

    if parsed.repository_path.is_empty() || parsed.repository_name.is_empty() {
        return Err(GitError::InvalidSshUrl {
            message: "La URL SSH no incluye una ruta de repositorio válida".to_owned(),
        });
    }

    Ok(parsed)
}

/// Normaliza una URL SSH para comparar remotes equivalentes.
pub fn normalize_ssh_url(input: &str) -> Result<String, GitError> {
    Ok(parse_ssh_url(input)?.normalized)
}

/// Indica si dos URLs SSH apuntan al mismo repositorio remoto.
pub fn ssh_urls_equivalent(left: &str, right: &str) -> Result<bool, GitError> {
    Ok(normalize_ssh_url(left)? == normalize_ssh_url(right)?)
}

/// Devuelve la carpeta por defecto donde se almacenan los clones.
pub fn default_clone_root() -> Result<PathBuf, GitError> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| BaseDirs::new().map(|directories| directories.data_local_dir().to_owned()))
        .map(|root| root.join("GitHelper").join("repos"))
        .ok_or(GitError::CloneRootUnavailable)
}

/// Calcula el destino local sugerido para un repositorio clonado por SSH.
pub fn default_clone_destination(
    clone_root: &Path,
    repository_name: &str,
) -> Result<PathBuf, GitError> {
    let safe_name = sanitize_directory_name(repository_name);
    if safe_name.is_empty() {
        return Err(GitError::InvalidSshUrl {
            message: "No se pudo derivar un nombre de carpeta seguro para el clon".to_owned(),
        });
    }
    Ok(clone_root.join(safe_name))
}

fn parse_scp_style(input: &str) -> Result<ParsedSshUrl, GitError> {
    let Some(colon_index) = input.rfind(':') else {
        return Err(GitError::InvalidSshUrl {
            message: "Formato SSH no reconocido; usa git@host:org/repo.git o ssh://…".to_owned(),
        });
    };
    if colon_index == 0 || colon_index == input.len() - 1 {
        return Err(GitError::InvalidSshUrl {
            message: "Formato scp inválido; falta el host o la ruta del repositorio".to_owned(),
        });
    }
    let host_part = &input[..colon_index];
    let path_part = input[colon_index + 1..].trim_start_matches('/');
    if !host_part.contains('@') {
        return Err(GitError::InvalidSshUrl {
            message: "Formato scp inválido; se esperaba usuario@host:ruta".to_owned(),
        });
    }
    let host = host_part
        .split('@')
        .nth(1)
        .ok_or_else(|| GitError::InvalidSshUrl {
            message: "Formato scp inválido; no se pudo leer el host".to_owned(),
        })?
        .to_ascii_lowercase();
    Ok(build_parsed(input, &host, path_part))
}

fn parse_ssh_scheme(input: &str) -> Result<ParsedSshUrl, GitError> {
    let without_scheme = input
        .trim_start_matches("ssh://")
        .trim_start_matches("SSH://");
    let (authority, path_part) =
        without_scheme
            .split_once('/')
            .ok_or_else(|| GitError::InvalidSshUrl {
                message: "La URL ssh:// debe incluir una ruta de repositorio".to_owned(),
            })?;
    if path_part.is_empty() {
        return Err(GitError::InvalidSshUrl {
            message: "La URL ssh:// no incluye una ruta de repositorio".to_owned(),
        });
    }
    let host = authority
        .rsplit('@')
        .next()
        .ok_or_else(|| GitError::InvalidSshUrl {
            message: "La URL ssh:// no incluye un host válido".to_owned(),
        })?
        .split(':')
        .next()
        .ok_or_else(|| GitError::InvalidSshUrl {
            message: "La URL ssh:// no incluye un host válido".to_owned(),
        })?
        .to_ascii_lowercase();
    Ok(build_parsed(input, &host, path_part))
}

fn build_parsed(original: &str, host: &str, repository_path: &str) -> ParsedSshUrl {
    let repository_path = canonical_repository_path(repository_path);
    let repository_name = repository_path
        .rsplit('/')
        .next()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    let normalized = format!("ssh://{host}/{repository_path}");
    ParsedSshUrl {
        original: original.to_owned(),
        normalized,
        host: host.to_owned(),
        repository_path,
        repository_name,
    }
}

fn normalize_repository_path(path: &str) -> String {
    path.trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .replace('\\', "/")
}

fn canonical_repository_path(path: &str) -> String {
    let normalized = normalize_repository_path(path);
    normalized
        .strip_suffix(".git")
        .unwrap_or(normalized.as_str())
        .to_ascii_lowercase()
}

fn sanitize_directory_name(name: &str) -> String {
    let mut sanitized = String::new();
    for character in name.chars() {
        let mapped = match character {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        };
        sanitized.push(mapped);
    }
    sanitized.trim().trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scp_style_urls() {
        let parsed = parse_ssh_url("git@github.com:org/repo.git").expect("debe parsear scp");
        assert_eq!(parsed.host, "github.com");
        assert_eq!(parsed.repository_path, "org/repo");
        assert_eq!(parsed.repository_name, "repo");
        assert_eq!(parsed.normalized, "ssh://github.com/org/repo");
    }

    #[test]
    fn parses_ssh_scheme_urls() {
        let parsed =
            parse_ssh_url("ssh://git@gitlab.com:2222/group/project.git").expect("debe parsear ssh");
        assert_eq!(parsed.host, "gitlab.com");
        assert_eq!(parsed.repository_path, "group/project");
        assert_eq!(parsed.repository_name, "project");
    }

    #[test]
    fn rejects_https_and_empty_urls() {
        assert!(matches!(
            parse_ssh_url("https://github.com/org/repo.git"),
            Err(GitError::UnsupportedUrlScheme { .. })
        ));
        assert!(matches!(
            parse_ssh_url(""),
            Err(GitError::InvalidSshUrl { .. })
        ));
    }

    #[test]
    fn normalizes_equivalent_urls() {
        let left = normalize_ssh_url("git@github.com:Org/Repo.git").expect("left");
        let right = normalize_ssh_url("ssh://git@github.com/org/repo").expect("right");
        assert_eq!(left, right);
    }

    #[test]
    fn builds_default_destination_under_clone_root() {
        let root = Path::new("GitHelper").join("repos");
        let destination = default_clone_destination(&root, "demo").expect("destino");
        assert_eq!(destination, root.join("demo"));
    }
}
