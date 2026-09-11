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

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tracing::warn;

pub(crate) const DEBOUNCE_DURATION: Duration = Duration::from_millis(250);
pub(crate) const MAX_DEBOUNCE_DURATION: Duration = Duration::from_secs(2);
const EVENT_QUEUE_CAPACITY: usize = 256;

/// Efectos de una ráfaga de cambios que la UI debe invalidar.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RepositoryChange {
    pub git_config_changed: bool,
    pub history_changed: bool,
    ignore_paths_change: IgnorePathsChange,
    pub watcher_error: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
enum IgnorePathsChange {
    #[default]
    None,
    DirectoryCreated,
    RulesChanged,
}

impl RepositoryChange {
    fn merge(&mut self, other: Self) {
        self.git_config_changed |= other.git_config_changed;
        self.history_changed |= other.history_changed;
        self.ignore_paths_change = self.ignore_paths_change.max(other.ignore_paths_change);
        self.watcher_error |= other.watcher_error;
    }

    /// Indica que el filtro de ignorados debe reconstruirse al terminar el lote.
    #[must_use]
    pub const fn refresh_ignored_paths(self) -> bool {
        !matches!(self.ignore_paths_change, IgnorePathsChange::None)
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
    worker_sender: mpsc::SyncSender<WorkerMessage>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RepositoryWatcher {
    /// Vigila el working tree y el git-dir real; agrupa ráfagas antes de notificar.
    #[allow(
        clippy::too_many_lines,
        reason = "la configuración y el worker forman una única operación de inicio del watcher"
    )]
    pub fn start(
        repository_root: &Path,
        git_directory: &Path,
        common_git_directory: Option<&Path>,
        ignored_paths: Vec<PathBuf>,
        on_refresh_requested: Arc<dyn Fn(RepositoryChange) + Send + Sync>,
    ) -> Result<Self, WatcherError> {
        let (worker_sender, worker_receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let event_sender = worker_sender.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_stopping = Arc::clone(&stopping);
        let callback_overflowed = Arc::clone(&overflowed);
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if callback_stopping.load(Ordering::Acquire) {
                return;
            }
            if event_sender.try_send(WorkerMessage::Event(result)).is_err() {
                callback_overflowed.store(true, Ordering::Release);
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
        if let Some(common_git_directory) = common_git_directory
            && common_git_directory != git_directory
            && !common_git_directory.starts_with(repository_root)
        {
            watcher
                .watch(common_git_directory, RecursiveMode::Recursive)
                .map_err(|source| WatcherError::Watch {
                    path: common_git_directory.to_path_buf(),
                    source,
                })?;
        }

        let repository_root = repository_root.to_path_buf();
        let git_directory = git_directory.to_path_buf();
        let common_git_directory = common_git_directory.map(Path::to_path_buf);
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::spawn(move || {
            let mut pending: Option<(Instant, Instant, RepositoryChange)> = None;
            loop {
                if overflowed.swap(false, Ordering::AcqRel) {
                    let now = Instant::now();
                    pending = Some((now, now, RepositoryChange::default()));
                }
                let timeout =
                    pending.map_or(Duration::from_mins(1), |(first_seen, last_seen, _)| {
                        DEBOUNCE_DURATION
                            .saturating_sub(last_seen.elapsed())
                            .min(MAX_DEBOUNCE_DURATION.saturating_sub(first_seen.elapsed()))
                    });
                match worker_receiver.recv_timeout(timeout) {
                    Ok(WorkerMessage::Event(_)) if worker_stopping.load(Ordering::Acquire) => break,
                    Ok(WorkerMessage::Event(Ok(event))) => {
                        if let Some(change) = classify_event(
                            &event,
                            &repository_root,
                            &git_directory,
                            common_git_directory.as_deref(),
                            &ignored_paths,
                        ) {
                            let now = Instant::now();
                            let (first_seen, mut accumulated) = pending.map_or(
                                (now, RepositoryChange::default()),
                                |(first_seen, _, accumulated)| (first_seen, accumulated),
                            );
                            accumulated.merge(change);
                            pending = Some((first_seen, now, accumulated));
                        }
                    }
                    Ok(WorkerMessage::Event(Err(watcher_error))) => {
                        warn!(error = %watcher_error, "El watcher notificó un error");
                        let now = Instant::now();
                        let (first_seen, mut accumulated) = pending.map_or(
                            (now, RepositoryChange::default()),
                            |(first_seen, _, accumulated)| (first_seen, accumulated),
                        );
                        accumulated.watcher_error = true;
                        pending = Some((first_seen, now, accumulated));
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some((_, _, change)) = pending.take() {
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
    common_git_directory: Option<&Path>,
    ignored_paths: &[PathBuf],
) -> Option<RepositoryChange> {
    let mut result: Option<RepositoryChange> = None;
    for path in &event.paths {
        if let Some(change) = classify_path(
            path,
            repository_root,
            git_directory,
            common_git_directory,
            ignored_paths,
        ) {
            result
                .get_or_insert_with(RepositoryChange::default)
                .merge(change);
        }
    }
    if matches!(event.kind, EventKind::Create(_))
        && event.paths.iter().any(|path| {
            path.starts_with(repository_root)
                && !path.starts_with(git_directory)
                && common_git_directory.is_none_or(|directory| !path.starts_with(directory))
                && path.is_dir()
        })
    {
        result
            .get_or_insert_with(RepositoryChange::default)
            .ignore_paths_change = IgnorePathsChange::DirectoryCreated;
    }
    result
}

fn classify_path(
    path: &Path,
    repository_root: &Path,
    git_directory: &Path,
    common_git_directory: Option<&Path>,
    ignored_paths: &[PathBuf],
) -> Option<RepositoryChange> {
    let git_relative = path
        .strip_prefix(git_directory)
        .ok()
        .or_else(|| common_git_directory.and_then(|directory| path.strip_prefix(directory).ok()));
    if let Some(relative) = git_relative {
        let is_exact = |candidate: &str| relative == Path::new(candidate);
        if is_exact("config") || is_exact("config.worktree") {
            return Some(RepositoryChange {
                git_config_changed: true,
                ..RepositoryChange::default()
            });
        }
        if is_exact("info/exclude") {
            return Some(RepositoryChange {
                ignore_paths_change: IgnorePathsChange::RulesChanged,
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
        ignore_paths_change: if ignore_rules_changed {
            IgnorePathsChange::RulesChanged
        } else {
            IgnorePathsChange::None
        },
        ..RepositoryChange::default()
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use notify::{Event, EventKind, event::CreateKind};
    use tempfile::tempdir;

    use super::{GenerationGate, classify_event, classify_path};

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

        assert!(
            classify_path(&git.join("objects/aa/object"), root, &git, None, &ignored).is_none()
        );
        assert!(
            classify_path(
                &root.join("target/debug/app.exe"),
                root,
                &git,
                None,
                &ignored
            )
            .is_none()
        );
        assert!(classify_path(&git.join("index"), root, &git, None, &ignored).is_some());
        assert!(classify_path(&root.join("src/lib.rs"), root, &git, None, &ignored).is_some());
    }

    #[test]
    fn identifies_cache_invalidation_events() {
        let root = Path::new(r"C:\repo");
        let git = root.join(".git");

        let config = classify_path(&git.join("config"), root, &git, None, &[]).unwrap();
        let worktree_config =
            classify_path(&git.join("config.worktree"), root, &git, None, &[]).unwrap();
        let history = classify_path(&git.join("refs/heads/main"), root, &git, None, &[]).unwrap();
        let ignores = classify_path(&root.join(".gitignore"), root, &git, None, &[]).unwrap();

        assert!(config.git_config_changed);
        assert!(worktree_config.git_config_changed);
        assert!(history.history_changed);
        assert!(ignores.refresh_ignored_paths());
    }

    #[test]
    fn newly_created_directory_requests_ignored_path_reconciliation() {
        let temporary = tempdir().unwrap();
        let root = temporary.path();
        let git = root.join(".git");
        let generated = root.join("build");
        fs::create_dir_all(&git).unwrap();
        fs::create_dir_all(&generated).unwrap();
        let event = Event::new(EventKind::Create(CreateKind::Folder)).add_path(generated);

        let change = classify_event(&event, root, &git, None, &[]).unwrap();

        assert!(change.refresh_ignored_paths());
    }
}
