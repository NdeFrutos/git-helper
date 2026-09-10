use std::path::PathBuf;

use crate::domain::{ChangeKind, FileChange, HeadState, StatusSnapshot};

use super::GitError;

/// Interpreta la salida NUL-delimitada de `git status --porcelain=v2`.
pub fn parse_status(output: &[u8]) -> Result<StatusSnapshot, GitError> {
    let text = std::str::from_utf8(output).map_err(|_| GitError::InvalidUtf8 {
        context: "git status",
    })?;
    let records: Vec<&str> = text.split('\0').collect();
    let mut parser = StatusParser::default();
    let mut index = 0;

    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        match record.as_bytes().first().copied() {
            Some(b'#') => parser.parse_header(record)?,
            Some(b'1') => parser.changes.push(parse_ordinary(record)?),
            Some(b'2') => {
                let original_path =
                    records
                        .get(index + 1)
                        .ok_or_else(|| GitError::InvalidStatus {
                            message: "falta la ruta original de un rename/copy".to_owned(),
                        })?;
                if original_path.is_empty() {
                    return Err(GitError::InvalidStatus {
                        message: "la ruta original de un rename/copy está vacía".to_owned(),
                    });
                }
                parser
                    .changes
                    .push(parse_renamed_or_copied(record, original_path)?);
                index += 1;
            }
            Some(b'u') => parser.changes.push(parse_unmerged(record)?),
            Some(b'?') => parser.changes.push(parse_untracked(record)?),
            Some(b'!') => {}
            _ => {
                return Err(GitError::InvalidStatus {
                    message: format!("tipo de registro desconocido: {record:?}"),
                });
            }
        }
        index += 1;
    }

    Ok(parser.finish())
}

#[derive(Default)]
struct StatusParser {
    oid: Option<String>,
    head_name: Option<String>,
    is_detached: bool,
    is_unborn: bool,
    upstream_name: Option<String>,
    ahead: u32,
    behind: u32,
    changes: Vec<FileChange>,
}

impl StatusParser {
    fn parse_header(&mut self, record: &str) -> Result<(), GitError> {
        if let Some(value) = record.strip_prefix("# branch.oid ") {
            if value == "(initial)" {
                self.is_unborn = true;
            } else {
                self.oid = Some(value.to_owned());
            }
        } else if let Some(value) = record.strip_prefix("# branch.head ") {
            if value == "(detached)" {
                self.is_detached = true;
            } else {
                self.head_name = Some(value.to_owned());
            }
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            self.upstream_name = Some(value.to_owned());
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            let mut counters = value.split_whitespace();
            self.ahead = parse_counter(counters.next(), '+', "ahead")?;
            self.behind = parse_counter(counters.next(), '-', "behind")?;
        }
        Ok(())
    }

    fn finish(self) -> StatusSnapshot {
        let head = if self.is_unborn {
            HeadState::Unborn
        } else if self.is_detached {
            HeadState::Detached {
                oid: self.oid.unwrap_or_default(),
            }
        } else if let Some(name) = self.head_name {
            HeadState::Branch {
                name,
                oid: self.oid,
            }
        } else {
            HeadState::Unborn
        };

        StatusSnapshot {
            head,
            upstream_name: self.upstream_name,
            ahead: self.ahead,
            behind: self.behind,
            changes: self.changes,
        }
    }
}

fn parse_counter(value: Option<&str>, prefix: char, name: &str) -> Result<u32, GitError> {
    let value = value.ok_or_else(|| GitError::InvalidStatus {
        message: format!("falta el contador {name}"),
    })?;
    value
        .strip_prefix(prefix)
        .ok_or_else(|| GitError::InvalidStatus {
            message: format!("contador {name} inválido: {value:?}"),
        })?
        .parse()
        .map_err(|_| GitError::InvalidStatus {
            message: format!("contador {name} inválido: {value:?}"),
        })
}

fn parse_ordinary(record: &str) -> Result<FileChange, GitError> {
    let fields: Vec<&str> = record.splitn(9, ' ').collect();
    require_field_count(&fields, 9, "ordinario")?;
    let (index_status, worktree_status) = parse_xy(fields[1])?;
    Ok(FileChange {
        path: PathBuf::from(fields[8]),
        original_path: None,
        index_status,
        worktree_status,
        is_conflicted: false,
    })
}

fn parse_renamed_or_copied(record: &str, original_path: &str) -> Result<FileChange, GitError> {
    let fields: Vec<&str> = record.splitn(10, ' ').collect();
    require_field_count(&fields, 10, "rename/copy")?;
    let (index_status, worktree_status) = parse_xy(fields[1])?;
    Ok(FileChange {
        path: PathBuf::from(fields[9]),
        original_path: Some(PathBuf::from(original_path)),
        index_status,
        worktree_status,
        is_conflicted: false,
    })
}

fn parse_unmerged(record: &str) -> Result<FileChange, GitError> {
    let fields: Vec<&str> = record.splitn(11, ' ').collect();
    require_field_count(&fields, 11, "conflicto")?;
    Ok(FileChange {
        path: PathBuf::from(fields[10]),
        original_path: None,
        index_status: ChangeKind::Unmerged,
        worktree_status: ChangeKind::Unmerged,
        is_conflicted: true,
    })
}

fn parse_untracked(record: &str) -> Result<FileChange, GitError> {
    let path = record
        .strip_prefix("? ")
        .ok_or_else(|| GitError::InvalidStatus {
            message: format!("registro untracked inválido: {record:?}"),
        })?;
    if path.is_empty() {
        return Err(GitError::InvalidStatus {
            message: "una ruta untracked está vacía".to_owned(),
        });
    }
    Ok(FileChange {
        path: PathBuf::from(path),
        original_path: None,
        index_status: ChangeKind::Unmodified,
        worktree_status: ChangeKind::Untracked,
        is_conflicted: false,
    })
}

fn parse_xy(value: &str) -> Result<(ChangeKind, ChangeKind), GitError> {
    let mut statuses = value.chars();
    let index_status = statuses.next().ok_or_else(|| GitError::InvalidStatus {
        message: "falta el estado del índice".to_owned(),
    })?;
    let worktree_status = statuses.next().ok_or_else(|| GitError::InvalidStatus {
        message: "falta el estado del working tree".to_owned(),
    })?;
    if statuses.next().is_some() {
        return Err(GitError::InvalidStatus {
            message: format!("estado XY inválido: {value:?}"),
        });
    }
    Ok((
        parse_change_kind(index_status)?,
        parse_change_kind(worktree_status)?,
    ))
}

fn parse_change_kind(value: char) -> Result<ChangeKind, GitError> {
    match value {
        '.' | ' ' => Ok(ChangeKind::Unmodified),
        'A' => Ok(ChangeKind::Added),
        'M' => Ok(ChangeKind::Modified),
        'D' => Ok(ChangeKind::Deleted),
        'R' => Ok(ChangeKind::Renamed),
        'C' => Ok(ChangeKind::Copied),
        'U' => Ok(ChangeKind::Unmerged),
        '?' => Ok(ChangeKind::Untracked),
        _ => Err(GitError::InvalidStatus {
            message: format!("código de estado desconocido: {value:?}"),
        }),
    }
}

fn require_field_count(
    fields: &[&str],
    expected: usize,
    record_type: &str,
) -> Result<(), GitError> {
    if fields.len() == expected && !fields[expected - 1].is_empty() {
        Ok(())
    } else {
        Err(GitError::InvalidStatus {
            message: format!(
                "registro {record_type} incompleto: se esperaban {expected} campos y hay {}",
                fields.len()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::domain::{ChangeKind, HeadState};

    use super::parse_status;

    #[test]
    fn parses_branch_headers_and_mixed_changes() {
        let output = concat!(
            "# branch.oid 0123456789abcdef\0",
            "# branch.head feature/prueba\0",
            "# branch.upstream origin/feature/prueba\0",
            "# branch.ab +2 -3\0",
            "1 M. N... 100644 100644 100644 a b staged.txt\0",
            "1 .M N... 100644 100644 100644 a b ruta con espacios.txt\0",
            "? nuevo-ñ.txt\0",
        );

        let snapshot = parse_status(output.as_bytes()).expect("el fixture debe ser válido");

        assert_eq!(
            snapshot.head,
            HeadState::Branch {
                name: "feature/prueba".to_owned(),
                oid: Some("0123456789abcdef".to_owned()),
            }
        );
        assert_eq!(
            snapshot.upstream_name.as_deref(),
            Some("origin/feature/prueba")
        );
        assert_eq!((snapshot.ahead, snapshot.behind), (2, 3));
        assert_eq!(snapshot.changes.len(), 3);
        assert_eq!(snapshot.changes[0].index_status, ChangeKind::Modified);
        assert_eq!(snapshot.changes[1].worktree_status, ChangeKind::Modified);
        assert_eq!(snapshot.changes[2].worktree_status, ChangeKind::Untracked);
    }

    #[test]
    fn parses_rename_with_nul_delimited_original_path() {
        let output = concat!(
            "# branch.oid abc\0",
            "# branch.head main\0",
            "2 R. N... 100644 100644 100644 a b R100 nueva ruta.txt\0",
            "ruta original.txt\0",
        );

        let snapshot = parse_status(output.as_bytes()).expect("el fixture debe ser válido");

        assert_eq!(snapshot.changes[0].path, PathBuf::from("nueva ruta.txt"));
        assert_eq!(
            snapshot.changes[0].original_path,
            Some(PathBuf::from("ruta original.txt"))
        );
        assert_eq!(snapshot.changes[0].index_status, ChangeKind::Renamed);
    }

    #[test]
    fn preserves_tabs_and_newlines_inside_paths() {
        let output = "? carpeta/linea\ncon\ttab.txt\0";

        let snapshot = parse_status(output.as_bytes()).expect("el fixture debe ser válido");

        assert_eq!(
            snapshot.changes[0].path,
            PathBuf::from("carpeta/linea\ncon\ttab.txt")
        );
    }

    #[test]
    fn parses_unborn_and_detached_heads() {
        let unborn =
            parse_status(b"# branch.oid (initial)\0# branch.head main\0? primer archivo.txt\0")
                .expect("el fixture unborn debe ser válido");
        let detached = parse_status(b"# branch.oid deadbeef\0# branch.head (detached)\0")
            .expect("el fixture detached debe ser válido");

        assert_eq!(unborn.head, HeadState::Unborn);
        assert_eq!(
            detached.head,
            HeadState::Detached {
                oid: "deadbeef".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_unknown_records() {
        let error = parse_status(b"x salida inesperada\0").expect_err("debe fallar");

        assert!(error.to_string().contains("tipo de registro desconocido"));
    }
}
