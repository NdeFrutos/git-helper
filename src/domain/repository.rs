use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    BranchReference, ChangeSelection, CommitId, CommitSummary, HeadState, Remote, UpstreamState,
    status::FileChange,
};

/// Identificador estable de una sesión de repositorio.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct RepositoryId(Uuid);

impl RepositoryId {
    /// Crea un identificador aleatorio para una pestaña nueva.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RepositoryId {
    fn default() -> Self {
        Self::new()
    }
}

/// Vista interna seleccionada en una pestaña.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RepositoryView {
    #[default]
    Changes,
    History,
}

/// Tipo de trabajo que mantiene ocupada una sesión.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationKind {
    Refresh,
    Stage,
    Unstage,
    Discard,
    Commit,
    Fetch,
    Pull,
    Push,
    GenerateCommitMessage,
}

/// Estado efímero del refresco de lecturas de una sesión.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RefreshState {
    #[default]
    Idle,
    Running {
        generation: u64,
    },
    Succeeded {
        message: String,
    },
    Failed {
        message: String,
        details: String,
    },
    Cancelled {
        message: String,
    },
}

/// Estado efímero de la última mutación de una sesión.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum MutationState {
    #[default]
    Idle,
    Running {
        kind: OperationKind,
        generation: u64,
    },
    Succeeded {
        kind: OperationKind,
        message: String,
    },
    Failed {
        kind: OperationKind,
        message: String,
        details: String,
    },
    Cancelled {
        kind: OperationKind,
        message: String,
    },
}

/// Coordina los refreshes solicitados para una sesión de repositorio.
///
/// Mientras hay un refresh en curso, las invalidaciones se agrupan en una
/// única ejecución posterior. Esto evita lanzar una tarea por evento y hace
/// que un cambio que llega durante una lectura no se pierda.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshCoordinator {
    in_flight: bool,
    dirty: bool,
}

impl RefreshCoordinator {
    /// Solicita un refresh. Devuelve `true` solo cuando debe iniciarse uno.
    pub fn request(&mut self) -> bool {
        if self.in_flight {
            self.dirty = true;
            return false;
        }
        self.in_flight = true;
        self.dirty = false;
        true
    }

    /// Marca que habrá que reconciliar el repositorio cuando termine la
    /// operación Git actual.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Finaliza el refresh actual e indica si hay que lanzar otro.
    pub fn finish(&mut self) -> bool {
        if !self.in_flight {
            return false;
        }
        self.in_flight = false;
        let should_refresh_again = self.dirty;
        self.dirty = false;
        should_refresh_again
    }

    /// Indica si hay una lectura de estado en curso.
    #[must_use]
    pub const fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// Contadores derivados del working tree, recalculados solo cuando cambian los cambios.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChangeCounters {
    pub change_count: usize,
    pub staged_count: usize,
}

/// Estado Git del working tree, índice y referencias consultables sin tocar el historial.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkingTreeSnapshot {
    pub head: HeadState,
    pub upstream: Option<UpstreamState>,
    pub remotes: Vec<Remote>,
    pub branches: Vec<BranchReference>,
    pub changes: Vec<FileChange>,
}

impl WorkingTreeSnapshot {
    /// Deriva contadores de la lista de cambios para evitar recorrerla en cada render.
    #[must_use]
    pub fn change_counters(&self) -> ChangeCounters {
        ChangeCounters {
            change_count: self.changes.len(),
            staged_count: self
                .changes
                .iter()
                .filter(|change| change.has_staged_change())
                .count(),
        }
    }
}

/// Página de historial paginada anclada a una referencia y OID estables.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySnapshot {
    pub commits: Arc<Vec<CommitSummary>>,
    pub has_more_commits: bool,
    pub history_reference: Option<String>,
    pub history_oid: Option<String>,
}

impl Default for HistorySnapshot {
    fn default() -> Self {
        Self {
            commits: Arc::new(Vec::new()),
            has_more_commits: false,
            history_reference: None,
            history_oid: None,
        }
    }
}

impl HistorySnapshot {
    /// Historial vacío tras invalidación o antes de la primera carga.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Sesión independiente asociada a una pestaña superior.
#[derive(Clone, Debug)]
pub struct RepositorySession {
    pub id: RepositoryId,
    pub root_path: PathBuf,
    pub working_tree: Arc<WorkingTreeSnapshot>,
    pub history: Arc<HistorySnapshot>,
    pub change_counters: ChangeCounters,
    pub selected_view: RepositoryView,
    pub selected_change: Option<ChangeSelection>,
    pub selected_commit: Option<CommitId>,
    pub refresh_state: RefreshState,
    pub mutation_state: MutationState,
    pub status_message: String,
    pub error: Option<String>,
    pub refresh_generation: u64,
    pub history_generation: u64,
    pub history_loaded: bool,
    pub history_loading: bool,
    pub refresh_coordinator: RefreshCoordinator,
    pub history_invalidated_during_refresh: bool,
}

impl RepositorySession {
    /// Crea una sesión vacía mientras se obtiene su primer snapshot.
    #[must_use]
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            id: RepositoryId::new(),
            root_path,
            working_tree: Arc::new(WorkingTreeSnapshot::default()),
            history: Arc::new(HistorySnapshot::empty()),
            change_counters: ChangeCounters::default(),
            selected_view: RepositoryView::default(),
            selected_change: None,
            selected_commit: None,
            refresh_state: RefreshState::default(),
            mutation_state: MutationState::default(),
            status_message: "Preparando repositorio…".to_owned(),
            error: None,
            refresh_generation: 0,
            history_generation: 0,
            history_loaded: false,
            history_loading: false,
            refresh_coordinator: RefreshCoordinator::default(),
            history_invalidated_during_refresh: false,
        }
    }

    /// Indica si hay una lectura de estado en curso para esta pestaña.
    #[must_use]
    pub fn is_refreshing(&self) -> bool {
        matches!(self.refresh_state, RefreshState::Running { .. })
    }

    /// Indica si hay una mutación Git o generación de mensaje en curso.
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        matches!(self.mutation_state, MutationState::Running { .. })
    }

    /// Las lecturas y mutaciones se serializan por repositorio para no operar
    /// sobre un snapshot que está siendo reemplazado.
    #[must_use]
    pub fn can_mutate(&self) -> bool {
        !self.is_mutating() && !self.is_refreshing()
    }

    /// Un refresh explícito no debe competir con otro refresh ni con una mutación.
    #[must_use]
    pub fn can_refresh(&self) -> bool {
        !self.is_refreshing() && !self.is_mutating()
    }
}

#[cfg(test)]
mod snapshot_tests {
    use std::path::PathBuf;

    use super::{ChangeCounters, WorkingTreeSnapshot};
    use crate::domain::{ChangeKind, FileChange};

    #[test]
    fn change_counters_track_staged_files_without_rescanning_in_render() {
        let snapshot = WorkingTreeSnapshot {
            changes: vec![
                FileChange {
                    path: PathBuf::from("a.rs"),
                    original_path: None,
                    index_status: ChangeKind::Modified,
                    worktree_status: ChangeKind::Unmodified,
                    is_conflicted: false,
                },
                FileChange {
                    path: PathBuf::from("b.rs"),
                    original_path: None,
                    index_status: ChangeKind::Unmodified,
                    worktree_status: ChangeKind::Modified,
                    is_conflicted: false,
                },
            ],
            ..WorkingTreeSnapshot::default()
        };

        assert_eq!(
            snapshot.change_counters(),
            ChangeCounters {
                change_count: 2,
                staged_count: 1,
            }
        );
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;

    #[test]
    fn refresh_and_mutation_states_are_independent() {
        let mut session = RepositorySession::new(PathBuf::from("repo"));
        session.refresh_state = RefreshState::Running { generation: 1 };

        assert!(session.is_refreshing());
        assert!(!session.is_mutating());
        assert!(!session.can_mutate());
        assert!(!session.can_refresh());

        session.mutation_state = MutationState::Running {
            kind: OperationKind::Stage,
            generation: 1,
        };
        assert!(session.is_refreshing());
        assert!(session.is_mutating());
        assert!(!session.can_mutate());
        assert!(!session.can_refresh());
    }

    #[test]
    fn cancelled_mutation_is_not_a_failure() {
        let cancelled = MutationState::Cancelled {
            kind: OperationKind::Commit,
            message: "Operación cancelada".to_owned(),
        };
        let failed = MutationState::Failed {
            kind: OperationKind::Commit,
            message: "fallo".to_owned(),
            details: "stderr".to_owned(),
        };

        assert_ne!(cancelled, failed);
    }
}

#[cfg(test)]
mod tests {
    use super::RefreshCoordinator;

    #[test]
    fn coalesces_invalidations_until_the_current_refresh_finishes() {
        let mut coordinator = RefreshCoordinator::default();

        assert!(coordinator.request());
        assert!(coordinator.in_flight());

        coordinator.mark_dirty();
        coordinator.mark_dirty();

        assert!(coordinator.finish());
        assert!(!coordinator.in_flight());
        assert!(!coordinator.finish());
    }

    #[test]
    fn does_not_start_more_than_one_refresh_for_repeated_requests() {
        let mut coordinator = RefreshCoordinator::default();

        assert!(coordinator.request());
        assert!(!coordinator.request());
        assert!(coordinator.finish());
        assert!(!coordinator.finish());
    }
}

/// Preferencias persistentes independientes de los repositorios.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub cursor_cli_path: Option<PathBuf>,
    pub cursor_context_consent: bool,
}

/// Preferencia de tema visual.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    System,
    Dark,
    Light,
}

/// Estado de aplicación mantenido por el modelo principal de GPUI.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub repositories: Vec<RepositorySession>,
    pub active_repository_id: Option<RepositoryId>,
    pub recent_repositories: Vec<PathBuf>,
    pub settings: AppSettings,
}
