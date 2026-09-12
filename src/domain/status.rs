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

/// Grupo de la vista Cambios en el que se representa una ruta.
///
/// Una misma ruta puede aparecer a la vez como `Staged` y como `Worktree`;
/// cada representación controla únicamente su propio estado.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChangeRepresentation {
    Conflict,
    Staged,
    Worktree,
    Untracked,
}

impl ChangeRepresentation {
    /// Identificador estable para ids de elemento y mensajes.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::Staged => "staged",
            Self::Worktree => "worktree",
            Self::Untracked => "untracked",
        }
    }

    /// Los conflictos se resuelven fuera de Git Helper y no participan en
    /// stage ni unstage por selección.
    #[must_use]
    pub const fn is_selectable(self) -> bool {
        !matches!(self, Self::Conflict)
    }
}

/// Identidad de una fila de la vista Cambios.
///
/// La fila se identifica por ruta y representación, nunca por posición: así
/// un refresco que reordene o elimine filas no puede trasladar la selección a
/// un archivo distinto.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChangeSelection {
    pub path: PathBuf,
    pub representation: ChangeRepresentation,
}

impl ChangeSelection {
    /// Crea la identidad de una fila concreta.
    #[must_use]
    pub const fn new(path: PathBuf, representation: ChangeRepresentation) -> Self {
        Self {
            path,
            representation,
        }
    }
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
