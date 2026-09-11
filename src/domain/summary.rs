use std::{
    collections::HashMap,
    hash::BuildHasher,
    path::{Path, PathBuf},
};

use super::{
    HeadState, MutationState, OperationKind, RefreshState, RepositoryId, RepositorySession,
    normalized_repo_key, primary_remote_label,
    remote_freshness::{RemoteFreshnessRecord, resolve_periodic_remote},
};

/// Presentación del snapshot local mostrado en el resumen.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SnapshotPresentation {
    #[default]
    Unknown,
    Loading,
    Current,
    Stale,
    Inaccessible,
}

/// Fila compacta derivada de una sesión sin invocar Git.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositorySummaryRow {
    pub id: RepositoryId,
    pub display_name: String,
    pub root_path: PathBuf,
    pub branch_label: String,
    pub change_count: usize,
    pub staged_count: usize,
    pub conflict_count: usize,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub snapshot_presentation: SnapshotPresentation,
    pub operation_label: Option<String>,
    pub error_hint: Option<String>,
    pub remote_freshness_label: Option<String>,
    pub remote_is_stale: bool,
}

/// Construye filas de resumen a partir de las sesiones abiertas.
#[must_use]
pub fn build_repository_summaries(
    repositories: &[RepositorySession],
    preferred_remotes: &HashMap<String, String, impl BuildHasher>,
    now_secs: u64,
    periodic_fetch_interval_secs: u64,
) -> Vec<RepositorySummaryRow> {
    repositories
        .iter()
        .map(|repository| {
            build_repository_summary(
                repository,
                preferred_remotes,
                now_secs,
                periodic_fetch_interval_secs,
            )
        })
        .collect()
}

/// Deriva una fila de resumen desde el snapshot ya cargado en la sesión.
#[must_use]
pub fn build_repository_summary(
    repository: &RepositorySession,
    preferred_remotes: &HashMap<String, String, impl BuildHasher>,
    now_secs: u64,
    periodic_fetch_interval_secs: u64,
) -> RepositorySummaryRow {
    let display_name = repository_display_name(&repository.root_path);
    let branch_label = summary_branch_label(&repository.working_tree.head);
    let conflict_count = repository
        .working_tree
        .changes
        .iter()
        .filter(|change| change.is_conflicted)
        .count();
    let (ahead, behind) = repository
        .working_tree
        .upstream
        .as_ref()
        .map_or((None, None), |upstream| {
            (Some(upstream.ahead), Some(upstream.behind))
        });
    let snapshot_presentation = snapshot_presentation_for(repository);
    let operation_label = operation_label_for(repository);
    let error_hint = error_hint_for(repository);
    let (remote_freshness_label, remote_is_stale) = remote_summary_for(
        repository,
        preferred_remotes,
        now_secs,
        periodic_fetch_interval_secs,
    );

    RepositorySummaryRow {
        id: repository.id,
        display_name,
        root_path: repository.root_path.clone(),
        branch_label,
        change_count: repository.change_counters.change_count,
        staged_count: repository.change_counters.staged_count,
        conflict_count,
        ahead,
        behind,
        snapshot_presentation,
        operation_label,
        error_hint,
        remote_freshness_label,
        remote_is_stale,
    }
}

fn repository_display_name(root_path: &Path) -> String {
    root_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Repositorio")
        .to_owned()
}

fn summary_branch_label(head: &HeadState) -> String {
    match head {
        HeadState::Branch { name, oid: Some(_) } => name.clone(),
        HeadState::Branch { name, oid: None } => format!("{name} (sin commits)"),
        HeadState::Detached { oid } => format!("HEAD @ {}", &oid[..oid.len().min(7)]),
        HeadState::Unborn => "Sin commit inicial".to_owned(),
    }
}

fn snapshot_presentation_for(repository: &RepositorySession) -> SnapshotPresentation {
    if !repository.path_accessible {
        return SnapshotPresentation::Inaccessible;
    }
    if repository.is_refreshing()
        || (!repository.has_loaded_snapshot
            && !matches!(
                repository.refresh_state,
                RefreshState::Failed { .. } | RefreshState::Cancelled { .. }
            ))
    {
        return SnapshotPresentation::Loading;
    }
    if !repository.has_loaded_snapshot {
        return SnapshotPresentation::Unknown;
    }
    if matches!(repository.refresh_state, RefreshState::Failed { .. }) {
        return SnapshotPresentation::Stale;
    }
    SnapshotPresentation::Current
}

fn operation_label_for(repository: &RepositorySession) -> Option<String> {
    if let RefreshState::Running { .. } = repository.refresh_state {
        return Some("Actualizando estado…".to_owned());
    }
    match &repository.mutation_state {
        MutationState::Running { kind, .. } => Some(operation_running_label(*kind).to_owned()),
        MutationState::Succeeded { kind, .. } => Some(operation_success_label(*kind)),
        MutationState::Cancelled { kind, .. } => {
            Some(format!("{} cancelada", operation_running_label(*kind)))
        }
        MutationState::Failed { kind, .. } => {
            Some(format!("{} fallida", operation_running_label(*kind)))
        }
        MutationState::Idle => None,
    }
}

fn error_hint_for(repository: &RepositorySession) -> Option<String> {
    if !repository.path_accessible {
        return Some(
            repository
                .error
                .clone()
                .unwrap_or_else(|| "Ruta no accesible".to_owned()),
        );
    }
    if let RefreshState::Failed { message, .. } = &repository.refresh_state {
        return Some(message.clone());
    }
    if let MutationState::Failed { message, .. } = &repository.mutation_state {
        return Some(message.clone());
    }
    repository.error.clone()
}

fn remote_summary_for(
    repository: &RepositorySession,
    preferred_remotes: &HashMap<String, String, impl BuildHasher>,
    now_secs: u64,
    periodic_fetch_interval_secs: u64,
) -> (Option<String>, bool) {
    let Some(remote_name) = resolve_periodic_remote(
        repository,
        preferred_remotes.get(&normalized_repo_key(&repository.root_path)),
    ) else {
        return (None, true);
    };
    let label = primary_remote_label(repository, preferred_remotes, now_secs);
    let remote_is_stale = repository
        .remote_freshness
        .record(&remote_name)
        .is_none_or(|record| {
            remote_record_is_stale(record, now_secs, periodic_fetch_interval_secs)
        });
    (label, remote_is_stale)
}

fn remote_record_is_stale(
    record: &RemoteFreshnessRecord,
    now_secs: u64,
    periodic_fetch_interval_secs: u64,
) -> bool {
    record.fetch_in_progress || record.is_stale(now_secs, periodic_fetch_interval_secs)
}

const fn operation_running_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Refresh => "Actualización",
        OperationKind::Stage => "Stage",
        OperationKind::Unstage => "Unstage",
        OperationKind::Discard => "Descarte",
        OperationKind::Commit => "Commit",
        OperationKind::Clone => "Clonado",
        OperationKind::Fetch => "Fetch",
        OperationKind::Pull => "Pull",
        OperationKind::Push => "Push",
        OperationKind::GenerateCommitMessage => "Generación de mensaje",
    }
}

fn operation_success_label(kind: OperationKind) -> String {
    match kind {
        OperationKind::Refresh => "Estado actualizado".to_owned(),
        OperationKind::Stage => "Stage completado".to_owned(),
        OperationKind::Unstage => "Unstage completado".to_owned(),
        OperationKind::Discard => "Descarte completado".to_owned(),
        OperationKind::Commit => "Commit creado".to_owned(),
        OperationKind::Clone => "Clonado completado".to_owned(),
        OperationKind::Fetch => "Fetch completado".to_owned(),
        OperationKind::Pull => "Pull completado".to_owned(),
        OperationKind::Push => "Push completado".to_owned(),
        OperationKind::GenerateCommitMessage => "Mensaje generado".to_owned(),
    }
}

/// Etiqueta corta del estado del snapshot para la fila del resumen.
#[must_use]
pub fn snapshot_presentation_label(presentation: SnapshotPresentation) -> &'static str {
    match presentation {
        SnapshotPresentation::Unknown => "Sin datos",
        SnapshotPresentation::Loading => "Cargando…",
        SnapshotPresentation::Current => "Actual",
        SnapshotPresentation::Stale => "Desactualizado",
        SnapshotPresentation::Inaccessible => "No accesible",
    }
}

/// Resume contadores de cambios para la columna compacta.
#[must_use]
pub fn format_change_counters(row: &RepositorySummaryRow) -> String {
    if row.change_count == 0 && row.conflict_count == 0 {
        return "Limpio".to_owned();
    }
    let mut parts = Vec::new();
    if row.conflict_count > 0 {
        parts.push(format!("{} conflictos", row.conflict_count));
    }
    if row.change_count > 0 {
        parts.push(format!("{} cambios", row.change_count));
    }
    if row.staged_count > 0 {
        parts.push(format!("{} staged", row.staged_count));
    }
    parts.join(" · ")
}

/// Resume ahead/behind marcando cuando la referencia remota puede estar antigua.
#[must_use]
pub fn format_sync_counters(row: &RepositorySummaryRow) -> Option<String> {
    let (ahead, behind) = (row.ahead?, row.behind?);
    let prefix = if row.remote_is_stale {
        "refs locales · "
    } else {
        ""
    };
    Some(format!("{prefix}↑{ahead} ↓{behind}"))
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use super::*;
    use crate::domain::{
        ChangeKind, FetchOrigin, FileChange, Remote, UpstreamState, WorkingTreeSnapshot,
    };

    fn sample_repository() -> RepositorySession {
        let mut repository = RepositorySession::new(PathBuf::from("/projects/demo"));
        repository.working_tree = Arc::new(WorkingTreeSnapshot {
            head: HeadState::Branch {
                name: "main".to_owned(),
                oid: Some("abc123".to_owned()),
            },
            upstream: Some(UpstreamState {
                full_name: "origin/main".to_owned(),
                remote_name: "origin".to_owned(),
                branch_name: "main".to_owned(),
                ahead: 2,
                behind: 1,
            }),
            remotes: vec![Remote {
                name: "origin".to_owned(),
            }],
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
                    index_status: ChangeKind::Unmerged,
                    worktree_status: ChangeKind::Unmerged,
                    is_conflicted: true,
                },
            ],
            ..WorkingTreeSnapshot::default()
        });
        repository.change_counters = repository.working_tree.change_counters();
        repository.has_loaded_snapshot = true;
        repository
    }

    #[test]
    fn summary_row_reflects_snapshot_counters_and_branch() {
        let repository = sample_repository();
        let row = build_repository_summary(&repository, &HashMap::new(), 1_000, 300);

        assert_eq!(row.display_name, "demo");
        assert_eq!(row.branch_label, "main");
        assert_eq!(row.change_count, 2);
        assert_eq!(row.staged_count, 1);
        assert_eq!(row.conflict_count, 1);
        assert_eq!(row.ahead, Some(2));
        assert_eq!(row.behind, Some(1));
        assert_eq!(row.snapshot_presentation, SnapshotPresentation::Current);
    }

    #[test]
    fn stale_remote_marks_sync_counters_as_local_refs() {
        let mut repository = sample_repository();
        repository.remote_freshness.mark_fetch_succeeded(
            "origin",
            FetchOrigin::Manual,
            100,
            &[],
            300,
        );

        let row = build_repository_summary(&repository, &HashMap::new(), 10_000, 300);

        assert!(row.remote_is_stale);
        assert_eq!(
            format_sync_counters(&row),
            Some("refs locales · ↑2 ↓1".to_owned())
        );
    }

    #[test]
    fn running_mutation_surfaces_operation_without_git_calls() {
        let mut repository = sample_repository();
        repository.mutation_state = MutationState::Running {
            kind: OperationKind::Commit,
            generation: 1,
        };

        let row = build_repository_summary(&repository, &HashMap::new(), 1_000, 300);

        assert_eq!(row.operation_label.as_deref(), Some("Commit"));
    }

    #[test]
    fn inaccessible_repository_is_reported_separately() {
        let mut repository = sample_repository();
        repository.path_accessible = false;
        repository.error = Some("Disco no disponible".to_owned());

        let row = build_repository_summary(&repository, &HashMap::new(), 1_000, 300);

        assert_eq!(
            row.snapshot_presentation,
            SnapshotPresentation::Inaccessible
        );
        assert_eq!(row.error_hint.as_deref(), Some("Disco no disponible"));
    }

    #[test]
    fn failed_refresh_keeps_stale_snapshot_visible_in_summary() {
        let mut repository = sample_repository();
        repository.refresh_state = RefreshState::Failed {
            message: "fallo".to_owned(),
            details: "stderr".to_owned(),
        };

        let row = build_repository_summary(&repository, &HashMap::new(), 1_000, 300);

        assert_eq!(row.snapshot_presentation, SnapshotPresentation::Stale);
        assert_eq!(row.error_hint.as_deref(), Some("fallo"));
        assert_eq!(row.change_count, 2);
    }
}
