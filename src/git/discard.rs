use std::path::PathBuf;

use crate::domain::{ChangeKind, FileChange};

use super::{GitError, validate_relative_path};

/// Forma exacta de descarte que se presentará en la confirmación.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscardMode {
    Worktree,
    StagedAndWorktree,
    UntrackedFile,
    UntrackedDirectory,
}

/// Operación destructiva ya validada, pero todavía no confirmada.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscardPlan {
    pub path: PathBuf,
    pub mode: DiscardMode,
}

/// Construye un plan explícito sin ejecutar ninguna mutación.
pub fn plan_discard(
    change: &FileChange,
    discard_staged: bool,
    has_head: bool,
    is_directory: bool,
) -> Result<DiscardPlan, GitError> {
    let path = validate_relative_path(&change.path)?;
    if change.is_conflicted {
        return Err(GitError::ConflictDiscardUnsupported);
    }

    let mode = if change.is_untracked() {
        if is_directory {
            DiscardMode::UntrackedDirectory
        } else {
            DiscardMode::UntrackedFile
        }
    } else if discard_staged {
        if !has_head {
            return Err(GitError::StagedDiscardWithoutHead);
        }
        DiscardMode::StagedAndWorktree
    } else if change.worktree_status.is_changed()
        && !matches!(change.worktree_status, ChangeKind::Untracked)
    {
        DiscardMode::Worktree
    } else {
        return Err(GitError::InvalidStatus {
            message: format!(
                "la ruta {} no tiene un cambio descartable en esa vista",
                path.display()
            ),
        });
    };

    Ok(DiscardPlan { path, mode })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::domain::{ChangeKind, FileChange};

    use super::{DiscardMode, plan_discard};

    fn staged_change() -> FileChange {
        FileChange {
            path: PathBuf::from("src/main.rs"),
            original_path: None,
            index_status: ChangeKind::Modified,
            worktree_status: ChangeKind::Unmodified,
            is_conflicted: false,
        }
    }

    #[test]
    fn rejects_staged_discard_without_head() {
        assert!(plan_discard(&staged_change(), true, false, false).is_err());
    }

    #[test]
    fn plans_head_restore_for_staged_change() {
        let plan = plan_discard(&staged_change(), true, true, false).expect("debe crear el plan");

        assert_eq!(plan.mode, DiscardMode::StagedAndWorktree);
    }

    #[test]
    fn rejects_conflicts() {
        let mut change = staged_change();
        change.is_conflicted = true;

        assert!(plan_discard(&change, true, true, false).is_err());
    }
}
