use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tracing::warn;

const DEBOUNCE_DURATION: Duration = Duration::from_millis(250);

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

/// Mantiene vivos el watcher y su worker de debounce.
pub struct RepositoryWatcher {
    _watcher: RecommendedWatcher,
    stop_sender: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RepositoryWatcher {
    /// Vigila el working tree y el git-dir real; agrupa ráfagas antes de notificar.
    pub fn start(
        repository_root: &Path,
        git_directory: &Path,
        on_refresh_requested: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, WatcherError> {
        let (event_sender, event_receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if let Err(send_error) = event_sender.send(result) {
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

        let worker = thread::spawn(move || {
            let mut pending_since: Option<Instant> = None;
            loop {
                if stop_receiver.try_recv().is_ok() {
                    break;
                }
                let timeout = pending_since.map_or(Duration::from_secs(1), |started| {
                    DEBOUNCE_DURATION.saturating_sub(started.elapsed())
                });
                match event_receiver.recv_timeout(timeout) {
                    Ok(Ok(_event)) => pending_since = Some(Instant::now()),
                    Ok(Err(watcher_error)) => {
                        warn!(error = %watcher_error, "El watcher notificó un error");
                        pending_since = Some(Instant::now());
                    }
                    Err(RecvTimeoutError::Timeout) if pending_since.is_some() => {
                        pending_since = None;
                        on_refresh_requested();
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            stop_sender,
            worker: Some(worker),
        })
    }
}

impl Drop for RepositoryWatcher {
    fn drop(&mut self) {
        let _ = self.stop_sender.send(());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            warn!("El worker del watcher terminó inesperadamente");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GenerationGate;

    #[test]
    fn rejects_stale_generations() {
        let mut gate = GenerationGate::default();
        let first = gate.next_generation();
        let second = gate.next_generation();

        assert!(!gate.accepts(first));
        assert!(gate.accepts(second));
    }
}
