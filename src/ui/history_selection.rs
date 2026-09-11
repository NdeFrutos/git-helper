use std::collections::{HashMap, VecDeque};

use crate::domain::{CommitDetails, CommitId, RepositoryId};

pub(crate) const DETAILS_CACHE_CAPACITY: usize = 64;

/// Identidad del historial del que procede un commit seleccionado.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct HistoryDetailsKey {
    pub(crate) reference: String,
    pub(crate) oid: String,
    pub(crate) commit_id: CommitId,
}

/// Token de una petición de detalles. Todo resultado debe conservar esta identidad.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HistoryDetailsRequest {
    pub(crate) repository_id: RepositoryId,
    pub(crate) request_id: u64,
    pub(crate) history_generation: u64,
    pub(crate) key: HistoryDetailsKey,
}

pub(crate) enum BeginDetailsSelection {
    Cached(CommitDetails),
    Loading(HistoryDetailsRequest),
}

pub(crate) enum DetailsCompletion {
    Applied(Box<CommitDetails>),
    Failed(String),
    Stale,
}

#[derive(Debug)]
struct DetailsCache {
    entries: HashMap<CommitId, CommitDetails>,
    order: VecDeque<CommitId>,
}

impl Default for DetailsCache {
    fn default() -> Self {
        Self {
            entries: HashMap::with_capacity(DETAILS_CACHE_CAPACITY),
            order: VecDeque::with_capacity(DETAILS_CACHE_CAPACITY),
        }
    }
}

impl DetailsCache {
    fn get(&mut self, commit_id: &str) -> Option<&CommitDetails> {
        if self.entries.contains_key(commit_id) {
            self.order.retain(|cached_id| cached_id != commit_id);
            self.order.push_back(commit_id.to_owned());
        }
        self.entries.get(commit_id)
    }

    fn insert(&mut self, details: CommitDetails) {
        let commit_id = details.summary.id.clone();
        if self.entries.contains_key(&commit_id) {
            self.order.retain(|cached_id| cached_id != &commit_id);
        } else if self.entries.len() == DETAILS_CACHE_CAPACITY
            && let Some(oldest_id) = self.order.pop_front()
        {
            self.entries.remove(&oldest_id);
        }
        self.order.push_back(commit_id.clone());
        self.entries.insert(commit_id, details);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Coordina selección, cancelación y caché de detalles sin depender de GPUI ni de temporizadores.
#[derive(Debug, Default)]
pub(crate) struct HistoryDetailsController {
    next_request_id: u64,
    active: HashMap<RepositoryId, HistoryDetailsRequest>,
    cache: DetailsCache,
}

impl HistoryDetailsController {
    /// Comienza una petición nueva o devuelve detalles ya cacheados por hash.
    pub(crate) fn begin(
        &mut self,
        repository_id: RepositoryId,
        history_generation: u64,
        key: HistoryDetailsKey,
    ) -> BeginDetailsSelection {
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request = HistoryDetailsRequest {
            repository_id,
            request_id: self.next_request_id,
            history_generation,
            key,
        };
        self.active.insert(repository_id, request.clone());
        if let Some(details) = self.cache.get(&request.key.commit_id).cloned() {
            self.active.remove(&repository_id);
            BeginDetailsSelection::Cached(details)
        } else {
            BeginDetailsSelection::Loading(request)
        }
    }

    /// Cancela la petición pendiente de una sesión, por ejemplo al cerrar o cambiar de rama.
    pub(crate) fn cancel(&mut self, repository_id: RepositoryId) {
        self.active.remove(&repository_id);
    }

    /// Indica si la sesión tiene una carga de detalles visible en curso.
    #[must_use]
    pub(crate) fn is_loading(&self, repository_id: RepositoryId) -> bool {
        self.active.contains_key(&repository_id)
    }

    /// Acepta una respuesta solo si sigue siendo la selección visible actual.
    pub(crate) fn complete(
        &mut self,
        request: &HistoryDetailsRequest,
        result: Result<CommitDetails, String>,
        current_generation: u64,
        current_key: Option<&HistoryDetailsKey>,
        selected_commit: Option<&str>,
    ) -> DetailsCompletion {
        let is_current = self.active.get(&request.repository_id) == Some(request)
            && request.history_generation == current_generation
            && current_key == Some(&request.key)
            && selected_commit == Some(request.key.commit_id.as_str());
        if !is_current {
            if self.active.get(&request.repository_id) == Some(request) {
                self.active.remove(&request.repository_id);
            }
            return DetailsCompletion::Stale;
        }
        self.active.remove(&request.repository_id);
        match result {
            Ok(details) if details.summary.id == request.key.commit_id => {
                self.cache.insert(details.clone());
                DetailsCompletion::Applied(Box::new(details))
            }
            Ok(_) => DetailsCompletion::Failed(
                "Git devolvió detalles de un commit distinto al seleccionado".to_owned(),
            ),
            Err(error) => DetailsCompletion::Failed(error),
        }
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.len()
    }

    #[cfg(test)]
    fn contains_cached(&self, commit_id: &str) -> bool {
        self.cache.entries.contains_key(commit_id)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BeginDetailsSelection, DETAILS_CACHE_CAPACITY, DetailsCompletion, HistoryDetailsController,
        HistoryDetailsKey, HistoryDetailsRequest,
    };
    use crate::domain::{CommitDetails, CommitSummary, RepositoryId};

    fn key(reference: &str, oid: &str, commit_id: &str) -> HistoryDetailsKey {
        HistoryDetailsKey {
            reference: reference.to_owned(),
            oid: oid.to_owned(),
            commit_id: commit_id.to_owned(),
        }
    }

    fn details(commit_id: &str) -> CommitDetails {
        CommitDetails {
            summary: CommitSummary {
                id: commit_id.to_owned(),
                short_id: commit_id.chars().take(7).collect(),
                subject: format!("subject {commit_id}"),
                author_name: "Test".to_owned(),
                author_email: "test@example.invalid".to_owned(),
                authored_at: 0,
                references: Vec::new(),
            },
            body: String::new(),
            committer_name: "Test".to_owned(),
            committer_email: "test@example.invalid".to_owned(),
            committed_at: 0,
            parent_ids: Vec::new(),
        }
    }

    fn make_request(selection: BeginDetailsSelection) -> HistoryDetailsRequest {
        match selection {
            BeginDetailsSelection::Loading(request) => request,
            BeginDetailsSelection::Cached(_) => panic!("la prueba necesita una carga"),
        }
    }

    #[test]
    fn ignores_out_of_order_response_after_a_new_selection() {
        let repository_id = RepositoryId::new();
        let mut controller = HistoryDetailsController::default();
        let request_a =
            make_request(controller.begin(repository_id, 7, key("refs/heads/main", "oid-a", "a")));
        let key_b = key("refs/heads/main", "oid-a", "b");
        let request_b = make_request(controller.begin(repository_id, 7, key_b.clone()));

        assert!(matches!(
            controller.complete(&request_a, Ok(details("a")), 7, Some(&key_b), Some("b")),
            DetailsCompletion::Stale
        ));
        assert!(controller.is_loading(repository_id));
        assert!(matches!(
            controller.complete(&request_b, Ok(details("b")), 7, Some(&key_b), Some("b")),
            DetailsCompletion::Applied(_)
        ));
    }

    #[test]
    fn cancelled_request_ignores_a_late_response() {
        let repository_id = RepositoryId::new();
        let mut controller = HistoryDetailsController::default();
        let current_key = key("refs/heads/main", "oid-a", "a");
        let request = make_request(controller.begin(repository_id, 1, current_key.clone()));

        controller.cancel(repository_id);

        assert!(matches!(
            controller.complete(&request, Ok(details("a")), 1, Some(&current_key), Some("a")),
            DetailsCompletion::Stale
        ));
        assert!(!controller.is_loading(repository_id));
    }

    #[test]
    fn rejects_response_after_history_generation_changes() {
        let repository_id = RepositoryId::new();
        let mut controller = HistoryDetailsController::default();
        let current_key = key("refs/heads/main", "oid-a", "a");
        let request = make_request(controller.begin(repository_id, 4, current_key.clone()));

        assert!(matches!(
            controller.complete(&request, Ok(details("a")), 5, Some(&current_key), Some("a")),
            DetailsCompletion::Stale
        ));
        assert!(!controller.is_loading(repository_id));
    }

    #[test]
    fn failed_requests_are_retryable_and_are_not_cached() {
        let repository_id = RepositoryId::new();
        let current_key = key("refs/heads/main", "oid-a", "a");
        let mut controller = HistoryDetailsController::default();
        let failed_request = make_request(controller.begin(repository_id, 1, current_key.clone()));

        assert!(matches!(
            controller.complete(
                &failed_request,
                Err("fallo determinista".to_owned()),
                1,
                Some(&current_key),
                Some("a"),
            ),
            DetailsCompletion::Failed(message) if message == "fallo determinista"
        ));
        let retry = make_request(controller.begin(repository_id, 1, current_key.clone()));
        assert!(matches!(
            controller.complete(&retry, Ok(details("a")), 1, Some(&current_key), Some("a")),
            DetailsCompletion::Applied(_)
        ));
        assert!(controller.contains_cached("a"));
    }

    #[test]
    fn returning_to_a_hash_uses_cached_details() {
        let repository_id = RepositoryId::new();
        let mut controller = HistoryDetailsController::default();
        let current_key = key("refs/heads/main", "oid-a", "a");
        let request = make_request(controller.begin(repository_id, 1, current_key.clone()));
        assert!(matches!(
            controller.complete(&request, Ok(details("a")), 1, Some(&current_key), Some("a")),
            DetailsCompletion::Applied(_)
        ));

        assert!(matches!(
            controller.begin(repository_id, 2, current_key),
            BeginDetailsSelection::Cached(_)
        ));
    }

    #[test]
    fn cache_is_bounded_and_evicts_least_recently_used_details() {
        let repository_id = RepositoryId::new();
        let mut controller = HistoryDetailsController::default();
        for index in 0..=DETAILS_CACHE_CAPACITY {
            let commit_id = format!("{index:040x}");
            let current_key = key("refs/heads/main", "oid", &commit_id);
            let request =
                make_request(controller.begin(repository_id, index as u64, current_key.clone()));
            assert!(matches!(
                controller.complete(
                    &request,
                    Ok(details(&commit_id)),
                    index as u64,
                    Some(&current_key),
                    Some(commit_id.as_str()),
                ),
                DetailsCompletion::Applied(_)
            ));
        }

        assert_eq!(controller.cache_len(), DETAILS_CACHE_CAPACITY);
        assert!(!controller.contains_cached("0000000000000000000000000000000000000000"));
        assert!(controller.contains_cached(&format!("{DETAILS_CACHE_CAPACITY:040x}")));
    }
}
