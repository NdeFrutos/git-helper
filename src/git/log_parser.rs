use crate::domain::{CommitDetails, CommitSummary, Reference};

use super::GitError;

pub const LOG_FIELD_COUNT: usize = 12;
pub const LOG_FORMAT: &str =
    "%H%x00%h%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%D%x00%s%x00%b";

/// Interpreta commits con campos NUL y cantidad fija.
pub fn parse_log(output: &[u8]) -> Result<Vec<CommitDetails>, GitError> {
    let text =
        std::str::from_utf8(output).map_err(|_| GitError::InvalidUtf8 { context: "git log" })?;
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    if !fields.len().is_multiple_of(LOG_FIELD_COUNT) {
        return Err(GitError::InvalidLog {
            message: format!(
                "se esperaban grupos de {LOG_FIELD_COUNT} campos y se recibieron {}",
                fields.len()
            ),
        });
    }

    fields
        .chunks_exact(LOG_FIELD_COUNT)
        .map(parse_commit)
        .collect()
}

fn parse_commit(fields: &[&str]) -> Result<CommitDetails, GitError> {
    let authored_at = parse_timestamp(fields[5], "fecha de autor")?;
    let committed_at = parse_timestamp(fields[8], "fecha de committer")?;
    let parent_ids = fields[2]
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    let references = parse_references(fields[9]);
    let summary = CommitSummary {
        id: fields[0].to_owned(),
        short_id: fields[1].to_owned(),
        subject: fields[10].to_owned(),
        author_name: fields[3].to_owned(),
        author_email: fields[4].to_owned(),
        authored_at,
        references,
    };

    Ok(CommitDetails {
        summary,
        body: fields[11].to_owned(),
        committer_name: fields[6].to_owned(),
        committer_email: fields[7].to_owned(),
        committed_at,
        parent_ids,
    })
}

fn parse_timestamp(value: &str, field_name: &str) -> Result<i64, GitError> {
    value.parse().map_err(|_| GitError::InvalidLog {
        message: format!("{field_name} inválida: {value:?}"),
    })
}

fn parse_references(value: &str) -> Vec<Reference> {
    value
        .split(", ")
        .filter(|reference| !reference.is_empty())
        .map(|reference| {
            if let Some(target) = reference.strip_prefix("HEAD -> ") {
                Reference::Head(strip_ref_prefix(target).to_owned())
            } else if let Some(tag) = reference.strip_prefix("tag: refs/tags/") {
                Reference::Tag(tag.to_owned())
            } else if let Some(branch) = reference.strip_prefix("refs/heads/") {
                Reference::LocalBranch(branch.to_owned())
            } else if let Some(branch) = reference.strip_prefix("refs/remotes/") {
                Reference::RemoteBranch(branch.to_owned())
            } else {
                Reference::Other(reference.to_owned())
            }
        })
        .collect()
}

fn strip_ref_prefix(value: &str) -> &str {
    value
        .strip_prefix("refs/heads/")
        .or_else(|| value.strip_prefix("refs/remotes/"))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use crate::domain::Reference;

    use super::parse_log;

    #[test]
    fn parses_commit_details_and_full_decorations() {
        let output = concat!(
            "0123456789\0",
            "0123456\0",
            "parent1 parent2\0",
            "Ana Ñ\0",
            "ana@example.com\0",
            "1700000000\0",
            "Comitter\0",
            "committer@example.com\0",
            "1700000100\0",
            "HEAD -> refs/heads/main, tag: refs/tags/v1.0, refs/remotes/origin/main\0",
            "feat: añade historial\0",
            "Cuerpo con\nvarias líneas\0",
        );

        let commits = parse_log(output.as_bytes()).expect("el fixture debe ser válido");
        let commit = &commits[0];

        assert_eq!(commit.summary.id, "0123456789");
        assert_eq!(commit.parent_ids, ["parent1", "parent2"]);
        assert_eq!(commit.body, "Cuerpo con\nvarias líneas");
        assert_eq!(
            commit.summary.references,
            [
                Reference::Head("main".to_owned()),
                Reference::Tag("v1.0".to_owned()),
                Reference::RemoteBranch("origin/main".to_owned()),
            ]
        );
    }

    #[test]
    fn accepts_empty_history() {
        assert!(
            parse_log(b"")
                .expect("la salida vacía es válida")
                .is_empty()
        );
    }

    #[test]
    fn rejects_incomplete_records() {
        let error = parse_log(b"hash\0short\0").expect_err("debe fallar");

        assert!(error.to_string().contains("grupos de 12 campos"));
    }
}
