use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tracing::warn;

const DEBOUNCE_DURATION: Duration = Duration::from_millis(250);

/// Efectos de una ráfaga de cambios que la UI debe invalidar.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RepositoryChange {
    pub git_config_changed: bool,
    pub history_changed: bool,
    pub ignore_rules_changed: bool,
}

impl RepositoryChange {
    fn merge(&mut self, other: Self) {
        self.git_config_changed |= other.git_config_changed;
        self.history_changed |= other.history_changed;
        self.ignore_rules_changed |= other.ignore_rules_changed;
    }
}

/// Descarta resultados anteriores al último refresh solicitado.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenerationGate {
    current: u64,
}

impl GenerationGate {
    /// Reserva una generación nueva.
    pub fn next_generation(&mut self) -> u64 {
        self.current = self.current.saturating_add(1);
        self.current
    }

    /// Indica si un resultado todavía puede reemplazar el snapshot.
    #[must_use]
    pub const fn accepts(&self, generation: u64) -> bool {
        generation == self.current
    }
}

/// Errores al configurar la vigilancia; F5 seguirá disponible.
#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("no se pudo crear el watcher: {0}")]
    Create(#[source] notify::Error),
    #[error("no se pudo vigilar {path:?}: {source}")]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

enum WorkerMessage {
    Event(notify::Result<Event>),
    Stop,
}

/// Mantiene vivos el watcher y su worker de debounce.
pub struct RepositoryWatcher {
    _watcher: RecommendedWatcher,
    worker_sender: mpsc::Sender<WorkerMessage>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RepositoryWatcher {
    /// Vigila el working tree y el git-dir real; agrupa ráfagas antes de notificar.
    pub fn start(
        repository_root: &Path,
        git_directory: &Path,
        ignored_paths: Vec<PathBuf>,
        on_refresh_requested: Arc<dyn Fn(RepositoryChange) + Send + Sync>,
    ) -> Result<Self, WatcherError> {
        let (worker_sender, worker_receiver) = mpsc::channel();
        let event_sender = worker_sender.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let callback_stopping = Arc::clone(&stopping);
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if callback_stopping.load(Ordering::Acquire) {
                return;
            }
            if let Err(send_error) = event_sender.send(WorkerMessage::Event(result)) {
                warn!(error = %send_error, "No se pudo reenviar un evento del watcher");
            }
        })
        .map_err(WatcherError::Create)?;

        watcher
            .watch(repository_root, RecursiveMode::Recursive)
            .map_err(|source| WatcherError::Watch {
                path: repository_root.to_path_buf(),
                source,
            })?;
        if !git_directory.starts_with(repository_root) {
            watcher
                .watch(git_directory, RecursiveMode::Recursive)
                .map_err(|source| WatcherError::Watch {
                    path: git_directory.to_path_buf(),
                    source,
                })?;
        }

        let repository_root = repository_root.to_path_buf();
        let git_directory = git_directory.to_path_buf();
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            let mut pending: Option<(Instant, RepositoryChange)> = None;
            loop {
                let timeout = pending.map_or(Duration::from_mins(1), |(started, _)| {
                    DEBOUNCE_DURATION.saturating_sub(started.elapsed())
                });
                match worker_receiver.recv_timeout(timeout) {
                    Ok(WorkerMessage::Event(_)) if worker_stopping.load(Ordering::Acquire) => break,
                    Ok(WorkerMessage::Event(Ok(event))) => {
                        if let Some(change) =
                            classify_event(&event, &repository_root, &git_directory, &ignored_paths)
                        {
                            let accumulated = pending.map_or(change, |(_, mut accumulated)| {
                                accumulated.merge(change);
                                accumulated
                            });
                            pending = Some((Instant::now(), accumulated));
                        }
                    }
                    Ok(WorkerMessage::Event(Err(watcher_error))) => {
                        warn!(error = %watcher_error, "El watcher notificó un error");
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some((_, change)) = pending.take() {
                            on_refresh_requested(change);
                        }
                    }
                    Ok(WorkerMessage::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            worker_sender,
            stopping,
            worker: Some(worker),
        })
    }
}

impl Drop for RepositoryWatcher {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.worker_sender.send(WorkerMessage::Stop);
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            warn!("El worker del watcher terminó inesperadamente");
        }
    }
}

fn classify_event(
    event: &Event,
    repository_root: &Path,
    git_directory: &Path,
    ignored_paths: &[PathBuf],
) -> Option<RepositoryChange> {
    let mut result: Option<RepositoryChange> = None;
    for path in &event.paths {
        if let Some(change) = classify_path(path, repository_root, git_directory, ignored_paths) {
            result
                .get_or_insert_with(RepositoryChange::default)
                .merge(change);
        }
    }
    result
}

fn classify_path(
    path: &Path,
    repository_root: &Path,
    git_directory: &Path,
    ignored_paths: &[PathBuf],
) -> Option<RepositoryChange> {
    if let Ok(relative) = path.strip_prefix(git_directory) {
        let is_exact = |candidate: &str| relative == Path::new(candidate);
        if is_exact("config") {
            return Some(RepositoryChange {
                git_config_changed: true,
                ..RepositoryChange::default()
            });
        }
        if is_exact("info/exclude") {
            return Some(RepositoryChange {
                ignore_rules_changed: true,
                ..RepositoryChange::default()
            });
        }
        if is_exact("HEAD")
            || is_exact("packed-refs")
            || is_exact("MERGE_HEAD")
            || is_exact("REBASE_HEAD")
            || is_exact("CHERRY_PICK_HEAD")
            || relative.starts_with("refs")
        {
            return Some(RepositoryChange {
                history_changed: true,
                ..RepositoryChange::default()
            });
        }
        return is_exact("index").then_some(RepositoryChange::default());
    }

    if !path.starts_with(repository_root)
        || ignored_paths
            .iter()
            .any(|ignored| path == ignored || path.starts_with(ignored))
    {
        return None;
    }

    let ignore_rules_changed = path
        .file_name()
        .is_some_and(|file_name| file_name == ".gitignore");
    Some(RepositoryChange {
        ignore_rules_changed,
        ..RepositoryChange::default()
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{GenerationGate, classify_path};

    #[test]
    fn rejects_stale_generations() {
        let mut gate = GenerationGate::default();
        let first = gate.next_generation();
        let second = gate.next_generation();

        assert!(!gate.accepts(first));
        assert!(gate.accepts(second));
    }

    #[test]
    fn filters_git_noise_and_ignored_trees() {
        let root = Path::new(r"C:\repo");
        let git = root.join(".git");
        let ignored = vec![root.join("target")];

        assert!(classify_path(&git.join("objects/aa/object"), root, &git, &ignored).is_none());
        assert!(classify_path(&root.join("target/debug/app.exe"), root, &git, &ignored).is_none());
        assert!(classify_path(&git.join("index"), root, &git, &ignored).is_some());
        assert!(classify_path(&root.join("src/lib.rs"), root, &git, &ignored).is_some());
    }

    #[test]
    fn identifies_cache_invalidation_events() {
        let root = Path::new(r"C:\repo");
        let git = root.join(".git");

        let config = classify_path(&git.join("config"), root, &git, &[]).unwrap();
        let history = classify_path(&git.join("refs/heads/main"), root, &git, &[]).unwrap();
        let ignores = classify_path(&root.join(".gitignore"), root, &git, &[]).unwrap();

        assert!(config.git_config_changed);
        assert!(history.history_changed);
        assert!(ignores.ignore_rules_changed);
    }
}
