use std::{path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ChangeSelection, CommitId, CommitSummary, HeadState, Remote, UpstreamState, status::FileChange,
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

/// Estado efímero de la última operación.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum OperationState {
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
}

/// Snapshot inmutable que la UI puede conservar durante un refresh.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepositorySnapshot {
    pub head: HeadState,
    pub upstream: Option<UpstreamState>,
    pub remotes: Vec<Remote>,
    pub changes: Vec<FileChange>,
    pub commits: Vec<CommitSummary>,
    pub has_more_commits: bool,
}

/// Sesión independiente asociada a una pestaña superior.
#[derive(Clone, Debug)]
pub struct RepositorySession {
    pub id: RepositoryId,
    pub root_path: PathBuf,
    pub snapshot: Arc<RepositorySnapshot>,
    pub selected_view: RepositoryView,
    pub selected_change: Option<ChangeSelection>,
    pub selected_commit: Option<CommitId>,
    pub operation_state: OperationState,
    pub refresh_generation: u64,
    pub history_generation: u64,
    pub history_loaded: bool,
    pub history_loading: bool,
}

impl RepositorySession {
    /// Crea una sesión vacía mientras se obtiene su primer snapshot.
    #[must_use]
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            id: RepositoryId::new(),
            root_path,
            snapshot: Arc::new(RepositorySnapshot::default()),
            selected_view: RepositoryView::default(),
            selected_change: None,
            selected_commit: None,
            operation_state: OperationState::default(),
            refresh_generation: 0,
            history_generation: 0,
            history_loaded: false,
            history_loading: false,
        }
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
