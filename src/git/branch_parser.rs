use crate::domain::{BranchKind, BranchReference, BranchUpstream};

use super::GitError;

pub const BRANCH_FORMAT: &str =
    "%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track)%00%(symref)%00";
const FIELD_COUNT: usize = 5;

/// Interpreta referencias locales y remote-tracking sin depender de separadores visibles.
pub fn parse_branch_refs(output: &[u8]) -> Result<Vec<BranchReference>, GitError> {
    let text = std::str::from_utf8(output).map_err(|_| GitError::InvalidUtf8 {
        context: "git for-each-ref",
    })?;
    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields
        .last()
        .is_some_and(|field| field.trim_matches(['\r', '\n']).is_empty())
    {
        fields.pop();
    }
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    if !fields.len().is_multiple_of(FIELD_COUNT) {
        return Err(GitError::InvalidBranches {
            message: format!(
                "se esperaban grupos de {FIELD_COUNT} campos y se recibieron {}",
                fields.len()
            ),
        });
    }

    fields
        .chunks_exact(FIELD_COUNT)
        .filter_map(|fields| match parse_branch(fields) {
            Ok(Some(branch)) => Some(Ok(branch)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn parse_branch(fields: &[&str]) -> Result<Option<BranchReference>, GitError> {
    // `for-each-ref` añade un salto de línea tras cada formato, incluso si
    // este termina en NUL. Las refs no admiten caracteres de control.
    let full_name = fields[0].trim_start_matches(['\r', '\n']);
    let (kind, prefix) = if full_name.starts_with("refs/heads/") {
        (BranchKind::Local, "refs/heads/")
    } else if full_name.starts_with("refs/remotes/") {
        (BranchKind::RemoteTracking, "refs/remotes/")
    } else {
        return Err(GitError::InvalidBranches {
            message: format!("referencia inesperada: {full_name:?}"),
        });
    };
    if !fields[4].is_empty() {
        return Ok(None);
    }
    let name = full_name
        .strip_prefix(prefix)
        .ok_or_else(|| GitError::InvalidBranches {
            message: format!("referencia sin prefijo esperado: {full_name:?}"),
        })?;
    if name.is_empty() {
        return Err(GitError::InvalidBranches {
            message: "una referencia de rama no puede tener nombre vacío".to_owned(),
        });
    }

    let upstream = if kind == BranchKind::Local {
        parse_upstream(fields[2], fields[3])?
    } else {
        BranchUpstream::NoUpstream
    };

    Ok(Some(BranchReference {
        name: name.to_owned(),
        full_name: full_name.to_owned(),
        oid: (!fields[1].is_empty()).then(|| fields[1].to_owned()),
        kind,
        is_active: false,
        upstream,
    }))
}

fn parse_upstream(full_name: &str, track: &str) -> Result<BranchUpstream, GitError> {
    if full_name.is_empty() {
        return Ok(BranchUpstream::NoUpstream);
    }
    if track == "[gone]" {
        return Ok(BranchUpstream::Gone {
            full_name: full_name.to_owned(),
        });
    }
    let (ahead, behind) = parse_track(track)?;
    Ok(BranchUpstream::Configured {
        full_name: full_name.to_owned(),
        ahead,
        behind,
    })
}

fn parse_track(track: &str) -> Result<(u32, u32), GitError> {
    if track.is_empty() || track == "[up to date]" {
        return Ok((0, 0));
    }
    let value = track
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| GitError::InvalidBranches {
            message: format!("estado upstream inválido: {track:?}"),
        })?;
    let mut ahead = 0;
    let mut behind = 0;
    for part in value.split(", ") {
        let (name, count) = part
            .split_once(' ')
            .ok_or_else(|| GitError::InvalidBranches {
                message: format!("contador upstream inválido: {part:?}"),
            })?;
        let count = count.parse().map_err(|_| GitError::InvalidBranches {
            message: format!("contador upstream inválido: {part:?}"),
        })?;
        match name {
            "ahead" => ahead = count,
            "behind" => behind = count,
            _ => {
                return Err(GitError::InvalidBranches {
                    message: format!("dirección upstream desconocida: {name:?}"),
                });
            }
        }
    }
    Ok((ahead, behind))
}

#[cfg(test)]
mod tests {
    use crate::domain::{BranchKind, BranchUpstream};

    use super::parse_branch_refs;

    #[test]
    fn parses_local_remote_unicode_and_tracking_states() {
        let output = concat!(
            "refs/heads/feature/ñ nueva\0abc\0refs/remotes/team/origin/feature\0[ahead 2, behind 3]\0\0\n",
            "refs/heads/sin-upstream\0def\0\0\0\0\n",
            "refs/heads/desaparecida\0ghi\0refs/remotes/origin/desaparecida\0[gone]\0\0\n",
            "refs/remotes/team/origin/feature\0abc\0\0\0\0\n",
        );

        let branches = parse_branch_refs(output.as_bytes()).expect("fixture válido");

        assert_eq!(branches.len(), 4);
        assert_eq!(branches[0].kind, BranchKind::Local);
        assert_eq!(branches[0].name, "feature/ñ nueva");
        assert_eq!(
            branches[0].upstream,
            BranchUpstream::Configured {
                full_name: "refs/remotes/team/origin/feature".to_owned(),
                ahead: 2,
                behind: 3,
            }
        );
        assert_eq!(branches[1].upstream, BranchUpstream::NoUpstream);
        assert_eq!(
            branches[2].upstream,
            BranchUpstream::Gone {
                full_name: "refs/remotes/origin/desaparecida".to_owned(),
            }
        );
        assert_eq!(branches[3].kind, BranchKind::RemoteTracking);
    }

    #[test]
    fn excludes_symbolic_refs_instead_of_showing_them_as_branches() {
        let output = concat!(
            "refs/remotes/origin/HEAD",
            "\0",
            "00abc",
            "\0\0\0",
            "refs/remotes/origin/main",
            "\0\n"
        );
        let branches =
            parse_branch_refs(output.as_bytes()).expect("la referencia simbólica se excluye");
        assert!(branches.is_empty());
    }
}
