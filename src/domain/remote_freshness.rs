use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasher,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{BranchKind, BranchReference, RepositoryId, RepositorySession};

/// Intervalo por defecto del fetch periódico (5 minutos).
pub const DEFAULT_PERIODIC_FETCH_INTERVAL_SECS: u64 = 300;

/// Intervalos disponibles para el fetch periódico configurable.
pub const PERIODIC_FETCH_INTERVALS: [u64; 3] = [300, 900, 1800];

/// Backoff máximo tras fallos consecutivos de fetch automático.
const MAX_FETCH_BACKOFF_MULTIPLIER: u32 = 4;

/// Abstracción de reloj inyectable para pruebas del fetch periódico.
pub trait Clock: Send + Sync {
    /// Devuelve el instante actual en segundos Unix.
    fn now_secs(&self) -> u64;
}

/// Reloj del sistema usado en producción.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Reloj fijo para pruebas deterministas.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock {
    now_secs: u64,
}

impl FixedClock {
    /// Crea un reloj que siempre devuelve el mismo instante.
    #[must_use]
    pub const fn new(now_secs: u64) -> Self {
        Self { now_secs }
    }
}

impl Clock for FixedClock {
    fn now_secs(&self) -> u64 {
        self.now_secs
    }
}

/// Origen conocido del último estado remoto reflejado localmente.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum FetchOrigin {
    #[default]
    Unknown,
    Manual,
    Periodic,
    External,
}

/// Estado de frescura de un remote concreto dentro de una sesión.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteFreshnessRecord {
    pub last_success_at: Option<u64>,
    pub last_attempt_at: Option<u64>,
    pub origin: FetchOrigin,
    pub last_error: Option<String>,
    pub consecutive_failures: u32,
    pub fetch_in_progress: bool,
    #[serde(default)]
    pub ref_fingerprint: HashMap<String, String>,
}

impl RemoteFreshnessRecord {
    /// Calcula el retardo hasta el próximo intento automático respetando backoff.
    #[must_use]
    pub fn backoff_delay_secs(&self, base_interval_secs: u64) -> u64 {
        if self.consecutive_failures == 0 {
            return base_interval_secs;
        }
        let multiplier = self.consecutive_failures.min(MAX_FETCH_BACKOFF_MULTIPLIER);
        base_interval_secs.saturating_mul(1 << multiplier)
    }

    /// Indica si los datos remotos deben considerarse potencialmente antiguos.
    #[must_use]
    pub fn is_stale(&self, now_secs: u64, base_interval_secs: u64) -> bool {
        match self.last_success_at {
            None => true,
            Some(last_success) => {
                now_secs.saturating_sub(last_success) > base_interval_secs.saturating_mul(2)
            }
        }
    }
}

/// Registro de frescura por remote para una pestaña de repositorio.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteFreshnessTracker {
    records: HashMap<String, RemoteFreshnessRecord>,
    next_scheduled_at: Option<u64>,
}

impl RemoteFreshnessTracker {
    /// Obtiene o crea el registro de un remote.
    pub fn record_mut(&mut self, remote_name: &str) -> &mut RemoteFreshnessRecord {
        self.records.entry(remote_name.to_owned()).or_default()
    }

    /// Consulta el registro de un remote si existe.
    #[must_use]
    pub fn record(&self, remote_name: &str) -> Option<&RemoteFreshnessRecord> {
        self.records.get(remote_name)
    }

    /// Reinicia la programación tras un fetch manual.
    pub fn reset_schedule_after_manual_fetch(&mut self, now_secs: u64, interval_secs: u64) {
        self.next_scheduled_at = Some(now_secs.saturating_add(interval_secs));
    }

    /// Programa el próximo fetch automático lo antes posible.
    pub fn schedule_immediately(&mut self, now_secs: u64) {
        self.next_scheduled_at = Some(now_secs);
    }

    /// Marca el inicio de un fetch para un remote.
    pub fn mark_fetch_started(&mut self, remote_name: &str, now_secs: u64) {
        let record = self.record_mut(remote_name);
        record.fetch_in_progress = true;
        record.last_attempt_at = Some(now_secs);
        record.last_error = None;
    }

    /// Registra un fetch exitoso iniciado por la aplicación.
    pub fn mark_fetch_succeeded(
        &mut self,
        remote_name: &str,
        origin: FetchOrigin,
        now_secs: u64,
        branches: &[BranchReference],
        interval_secs: u64,
    ) {
        let fingerprint = remote_ref_fingerprint(remote_name, branches);
        let record = self.record_mut(remote_name);
        record.fetch_in_progress = false;
        record.last_success_at = Some(now_secs);
        record.last_attempt_at = Some(now_secs);
        record.origin = origin;
        record.last_error = None;
        record.consecutive_failures = 0;
        record.ref_fingerprint = fingerprint;
        self.next_scheduled_at = Some(now_secs.saturating_add(interval_secs));
    }

    /// Registra un fetch fallido conservando el último éxito previo.
    pub fn mark_fetch_failed(
        &mut self,
        remote_name: &str,
        now_secs: u64,
        error: String,
        interval_secs: u64,
    ) {
        let record = self.record_mut(remote_name);
        record.fetch_in_progress = false;
        record.last_attempt_at = Some(now_secs);
        record.last_error = Some(error);
        record.consecutive_failures = record.consecutive_failures.saturating_add(1);
        let delay = record.backoff_delay_secs(interval_secs);
        self.next_scheduled_at = Some(now_secs.saturating_add(delay));
    }

    /// Detecta cambios en refs remotas no originados por un fetch de la app.
    pub fn detect_external_updates(
        &mut self,
        remotes: &[String],
        branches: &[BranchReference],
        now_secs: u64,
    ) {
        for remote_name in remotes {
            let fingerprint = remote_ref_fingerprint(remote_name, branches);
            let record = self.record_mut(remote_name);
            if record.fetch_in_progress {
                continue;
            }
            if fingerprint.is_empty() {
                continue;
            }
            if record.ref_fingerprint.is_empty() {
                record.ref_fingerprint = fingerprint;
                continue;
            }
            if record.ref_fingerprint != fingerprint {
                record.ref_fingerprint = fingerprint;
                record.origin = FetchOrigin::External;
            }
        }
        if self.next_scheduled_at.is_none() {
            self.next_scheduled_at = Some(now_secs);
        }
    }

    /// Devuelve la etiqueta informativa para un remote concreto.
    #[must_use]
    pub fn label_for_remote(&self, remote_name: &str, now_secs: u64) -> String {
        let Some(record) = self.record(remote_name) else {
            return format!("{remote_name}: sin fetch conocido");
        };
        format_freshness_label(remote_name, record, now_secs)
    }

    /// Indica si el fetch automático está pendiente para la sesión.
    #[must_use]
    pub fn is_due(&self, now_secs: u64) -> bool {
        self.next_scheduled_at.is_some_and(|due| now_secs >= due)
    }
}

/// Candidato elegible para un fetch periódico.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeriodicFetchCandidate {
    pub repository_id: RepositoryId,
    pub remote_name: String,
}

/// Selecciona el siguiente fetch automático priorizando la pestaña activa.
#[must_use]
pub fn select_periodic_fetch(
    now_secs: u64,
    active_repository_id: Option<RepositoryId>,
    repositories: &[RepositorySession],
    periodic_fetch_enabled: bool,
    interval_secs: u64,
    preferred_remotes: &HashMap<String, String, impl BuildHasher>,
    in_flight: &HashSet<(RepositoryId, String), impl BuildHasher>,
) -> Option<PeriodicFetchCandidate> {
    if !periodic_fetch_enabled {
        return None;
    }

    let mut active_candidate = None;
    let mut background_candidate = None;

    for repository in repositories {
        if repository.is_mutating()
            || repository.is_refreshing()
            || matches!(
                repository.mutation_state,
                super::MutationState::Running {
                    kind: super::OperationKind::GenerateCommitMessage,
                    ..
                }
            )
        {
            continue;
        }

        let Some(remote_name) = resolve_periodic_remote(
            repository,
            preferred_remotes.get(&normalized_repo_key(&repository.root_path)),
        ) else {
            continue;
        };

        if in_flight.contains(&(repository.id, remote_name.clone())) {
            continue;
        }

        let record = repository
            .remote_freshness
            .record(&remote_name)
            .cloned()
            .unwrap_or_default();
        if record.fetch_in_progress {
            continue;
        }

        let due_at = repository
            .remote_freshness
            .next_scheduled_at
            .unwrap_or(now_secs);
        let backoff_due = record.last_attempt_at.is_none_or(|last_attempt| {
            now_secs >= last_attempt.saturating_add(record.backoff_delay_secs(interval_secs))
        });
        if now_secs < due_at || !backoff_due {
            continue;
        }

        let candidate = PeriodicFetchCandidate {
            repository_id: repository.id,
            remote_name,
        };
        if Some(repository.id) == active_repository_id {
            active_candidate = Some(candidate);
            break;
        }
        if background_candidate.is_none() {
            background_candidate = Some(candidate);
        }
    }

    active_candidate.or(background_candidate)
}

/// Resuelve el remote objetivo del fetch automático sin solicitar selección interactiva.
pub(crate) fn resolve_periodic_remote(
    repository: &RepositorySession,
    preferred_remote: Option<&String>,
) -> Option<String> {
    if let Some(upstream) = repository.working_tree.upstream.as_ref() {
        return Some(upstream.remote_name.clone());
    }
    if let Some(preferred) = preferred_remote
        && repository
            .working_tree
            .remotes
            .iter()
            .any(|remote| remote.name == *preferred)
    {
        return Some(preferred.clone());
    }
    match repository.working_tree.remotes.as_slice() {
        [remote] => Some(remote.name.clone()),
        _ => None,
    }
}

/// Construye una huella estable de refs remote-tracking para un remote.
#[must_use]
pub fn remote_ref_fingerprint(
    remote_name: &str,
    branches: &[BranchReference],
) -> HashMap<String, String> {
    let prefix = format!("refs/remotes/{remote_name}/");
    branches
        .iter()
        .filter(|branch| branch.kind == BranchKind::RemoteTracking)
        .filter(|branch| branch.full_name.starts_with(&prefix))
        .filter_map(|branch| {
            branch
                .oid
                .as_ref()
                .map(|oid| (branch.full_name.clone(), oid.clone()))
        })
        .collect()
}

/// Formatea la antigüedad relativa de un instante Unix.
#[must_use]
pub fn format_relative_age(now_secs: u64, then_secs: u64) -> String {
    let elapsed = now_secs.saturating_sub(then_secs);
    if elapsed < 60 {
        "hace un momento".to_owned()
    } else if elapsed < 3600 {
        format!("hace {} min", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("hace {} h", elapsed / 3600)
    } else {
        format!("hace {} d", elapsed / 86_400)
    }
}

/// Genera el texto informativo de frescura remota para la UI.
#[must_use]
pub fn format_freshness_label(
    remote_name: &str,
    record: &RemoteFreshnessRecord,
    now_secs: u64,
) -> String {
    if record.fetch_in_progress {
        return format!("{remote_name}: fetch en curso…");
    }
    if let Some(error) = &record.last_error {
        let age = record.last_success_at.map_or_else(
            || "sin éxito previo".to_owned(),
            |success| format_relative_age(now_secs, success),
        );
        return format!("{remote_name}: fetch fallido ({age}) — {error}");
    }
    match (record.origin, record.last_success_at) {
        (FetchOrigin::External, _) => {
            format!("{remote_name}: actualizado externamente (hora desconocida)")
        }
        (_, None) => format!("{remote_name}: sin fetch conocido"),
        (_, Some(success)) => {
            let age = format_relative_age(now_secs, success);
            let origin = match record.origin {
                FetchOrigin::Manual => "manual",
                FetchOrigin::Periodic => "automático",
                FetchOrigin::External | FetchOrigin::Unknown => "desconocido",
            };
            format!("{remote_name}: {age} ({origin})")
        }
    }
}

/// Resume la frescura del remote principal visible en la barra de acciones.
#[must_use]
pub fn primary_remote_label(
    repository: &RepositorySession,
    preferred_remotes: &HashMap<String, String, impl BuildHasher>,
    now_secs: u64,
) -> Option<String> {
    let remote_name = resolve_periodic_remote(
        repository,
        preferred_remotes.get(&normalized_repo_key(&repository.root_path)),
    )?;
    Some(
        repository
            .remote_freshness
            .label_for_remote(&remote_name, now_secs),
    )
}

/// Etiqueta legible para el botón de auto-fetch según estado e intervalo.
#[must_use]
pub fn format_periodic_fetch_interval_label(enabled: bool, interval_secs: u64) -> String {
    if !enabled {
        return "Auto-fetch off".to_owned();
    }
    if interval_secs.is_multiple_of(60) {
        format!("Auto-fetch {} min", interval_secs / 60)
    } else {
        format!("Auto-fetch {interval_secs}s")
    }
}

/// Devuelve el siguiente intervalo al ciclar con Shift+clic.
#[must_use]
pub fn next_periodic_fetch_interval(current_secs: u64) -> u64 {
    PERIODIC_FETCH_INTERVALS
        .iter()
        .find(|&&secs| secs > current_secs)
        .copied()
        .unwrap_or(PERIODIC_FETCH_INTERVALS[0])
}

/// Normaliza la ruta del repositorio para usarla como clave persistente.
#[must_use]
pub fn normalized_repo_key(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Duración recomendada entre comprobaciones del bucle de fetch periódico.
#[must_use]
pub const fn periodic_fetch_poll_interval() -> Duration {
    Duration::from_secs(30)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::{HeadState, Remote, UpstreamState};

    fn sample_repository(remotes: Vec<&str>, upstream_remote: Option<&str>) -> RepositorySession {
        let mut repository = RepositorySession::new(PathBuf::from(r"C:\repo"));
        repository.working_tree = std::sync::Arc::new(crate::domain::WorkingTreeSnapshot {
            head: HeadState::Branch {
                name: "main".to_owned(),
                oid: Some("abc".to_owned()),
            },
            upstream: upstream_remote.map(|remote| UpstreamState {
                full_name: format!("{remote}/main"),
                remote_name: remote.to_owned(),
                branch_name: "main".to_owned(),
                ahead: 0,
                behind: 0,
            }),
            remotes: remotes
                .into_iter()
                .map(|name| Remote {
                    name: name.to_owned(),
                })
                .collect(),
            ..crate::domain::WorkingTreeSnapshot::default()
        });
        repository
    }

    #[test]
    fn periodic_fetch_interval_label_formats_minutes_and_off_state() {
        assert_eq!(
            format_periodic_fetch_interval_label(true, 300),
            "Auto-fetch 5 min"
        );
        assert_eq!(
            format_periodic_fetch_interval_label(false, 300),
            "Auto-fetch off"
        );
    }

    #[test]
    fn periodic_fetch_interval_cycles_through_available_values() {
        assert_eq!(next_periodic_fetch_interval(300), 900);
        assert_eq!(next_periodic_fetch_interval(900), 1800);
        assert_eq!(next_periodic_fetch_interval(1800), 300);
    }

    #[test]
    fn periodic_fetch_is_disabled_by_default() {
        let repository = sample_repository(vec!["origin"], None);
        assert!(
            select_periodic_fetch(
                1_000,
                Some(repository.id),
                &[repository],
                false,
                DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
                &HashMap::new(),
                &HashSet::new(),
            )
            .is_none()
        );
    }

    #[test]
    fn periodic_fetch_prioritizes_active_repository() {
        let mut active = sample_repository(vec!["origin"], Some("origin"));
        let mut background = sample_repository(vec!["origin"], Some("origin"));
        let active_id = active.id;
        active.remote_freshness.next_scheduled_at = Some(0);
        background.remote_freshness.next_scheduled_at = Some(0);

        let candidate = select_periodic_fetch(
            1_000,
            Some(active_id),
            &[background, active],
            true,
            DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
            &HashMap::new(),
            &HashSet::new(),
        )
        .expect("debe elegir candidato");

        assert_eq!(candidate.repository_id, active_id);
    }

    #[test]
    fn periodic_fetch_respects_backoff_after_failures() {
        let mut repository = sample_repository(vec!["origin"], Some("origin"));
        repository.remote_freshness.next_scheduled_at = Some(0);
        repository
            .remote_freshness
            .record_mut("origin")
            .consecutive_failures = 2;
        repository
            .remote_freshness
            .record_mut("origin")
            .last_attempt_at = Some(1_000);

        assert!(
            select_periodic_fetch(
                1_100,
                Some(repository.id),
                &[repository],
                true,
                300,
                &HashMap::new(),
                &HashSet::new(),
            )
            .is_none()
        );

        let repository = sample_repository(vec!["origin"], Some("origin"));
        let mut repository = repository;
        repository.remote_freshness.next_scheduled_at = Some(0);
        repository
            .remote_freshness
            .record_mut("origin")
            .consecutive_failures = 2;
        repository
            .remote_freshness
            .record_mut("origin")
            .last_attempt_at = Some(1_000);

        assert!(
            select_periodic_fetch(
                3_500,
                Some(repository.id),
                &[repository],
                true,
                300,
                &HashMap::new(),
                &HashSet::new(),
            )
            .is_some()
        );
    }

    #[test]
    fn periodic_fetch_skips_repositories_with_multiple_remotes_without_preference() {
        let repository = sample_repository(vec!["origin", "upstream"], None);
        let mut repository = repository;
        repository.remote_freshness.next_scheduled_at = Some(0);

        assert!(
            select_periodic_fetch(
                1_000,
                Some(repository.id),
                &[repository],
                true,
                DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
                &HashMap::new(),
                &HashSet::new(),
            )
            .is_none()
        );
    }

    #[test]
    fn periodic_fetch_uses_configured_preferred_remote() {
        let repository = sample_repository(vec!["origin", "upstream"], None);
        let mut repository = repository;
        repository.remote_freshness.next_scheduled_at = Some(0);
        let mut preferred = HashMap::new();
        preferred.insert(
            normalized_repo_key(&repository.root_path),
            "upstream".to_owned(),
        );

        let candidate = select_periodic_fetch(
            1_000,
            Some(repository.id),
            &[repository],
            true,
            DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
            &preferred,
            &HashSet::new(),
        )
        .expect("debe usar el remote preferido");

        assert_eq!(candidate.remote_name, "upstream");
    }

    #[test]
    fn failed_fetch_keeps_previous_success_timestamp() {
        let mut tracker = RemoteFreshnessTracker::default();
        tracker.mark_fetch_succeeded(
            "origin",
            FetchOrigin::Manual,
            1_000,
            &[],
            DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
        );
        tracker.mark_fetch_failed(
            "origin",
            1_100,
            "auth failed".to_owned(),
            DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
        );

        let record = tracker.record("origin").expect("debe existir");
        assert_eq!(record.last_success_at, Some(1_000));
        assert_eq!(record.consecutive_failures, 1);
        assert!(record.last_error.is_some());
    }

    #[test]
    fn external_ref_changes_are_reported_honestly() {
        let mut tracker = RemoteFreshnessTracker::default();
        tracker.mark_fetch_succeeded(
            "origin",
            FetchOrigin::Manual,
            1_000,
            &[BranchReference {
                name: "main".to_owned(),
                full_name: "refs/remotes/origin/main".to_owned(),
                oid: Some("aaa".to_owned()),
                kind: BranchKind::RemoteTracking,
                is_active: false,
                upstream: crate::domain::BranchUpstream::NoUpstream,
            }],
            DEFAULT_PERIODIC_FETCH_INTERVAL_SECS,
        );
        tracker.detect_external_updates(
            &["origin".to_owned()],
            &[BranchReference {
                name: "main".to_owned(),
                full_name: "refs/remotes/origin/main".to_owned(),
                oid: Some("bbb".to_owned()),
                kind: BranchKind::RemoteTracking,
                is_active: false,
                upstream: crate::domain::BranchUpstream::NoUpstream,
            }],
            1_200,
        );

        let record = tracker.record("origin").expect("debe existir");
        assert_eq!(record.origin, FetchOrigin::External);
        assert_eq!(record.last_success_at, Some(1_000));
        assert_eq!(
            format_freshness_label("origin", record, 1_200),
            "origin: actualizado externamente (hora desconocida)"
        );
    }

    #[test]
    fn manual_fetch_resets_schedule() {
        let mut tracker = RemoteFreshnessTracker::default();
        tracker.reset_schedule_after_manual_fetch(1_000, 300);
        assert_eq!(tracker.next_scheduled_at, Some(1_300));
    }
}
