use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Estado de una ruta en el índice o en el árbol de trabajo.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ChangeKind {
    #[default]
    Unmodified,
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
}

impl ChangeKind {
    /// Indica si el estado representa una modificación real.
    #[must_use]
    pub const fn is_changed(self) -> bool {
        !matches!(self, Self::Unmodified)
    }
}

/// Cambio observado por Git para una ruta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index_status: ChangeKind,
    pub worktree_status: ChangeKind,
    pub is_conflicted: bool,
}

impl FileChange {
    /// Indica si la fila debe aparecer en el grupo de cambios staged.
    #[must_use]
    pub const fn has_staged_change(&self) -> bool {
        self.index_status.is_changed() && !self.is_conflicted
    }

    /// Indica si la fila debe aparecer en el grupo de cambios no staged.
    #[must_use]
    pub const fn has_worktree_change(&self) -> bool {
        self.worktree_status.is_changed()
            && !matches!(self.worktree_status, ChangeKind::Untracked)
            && !self.is_conflicted
    }

    /// Indica si la ruta todavía no está bajo seguimiento.
    #[must_use]
    pub const fn is_untracked(&self) -> bool {
        matches!(self.worktree_status, ChangeKind::Untracked)
    }
}

/// Selección de una representación concreta de un cambio.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeSelection {
    Conflict(PathBuf),
    Staged(PathBuf),
    Worktree(PathBuf),
    Untracked(PathBuf),
}

/// Estado de HEAD comunicado por `git status`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum HeadState {
    Branch {
        name: String,
        oid: Option<String>,
    },
    Detached {
        oid: String,
    },
    #[default]
    Unborn,
}

/// Resultado estructurado de `git status --porcelain=v2`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusSnapshot {
    pub head: HeadState,
    pub upstream_name: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
}
