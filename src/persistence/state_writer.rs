use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use super::{AppStateStore, PersistedAppState, PersistenceError};

const CLOSE_FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// Escritor único con coalescencia: la última versión programada siempre gana.
#[derive(Clone, Debug)]
pub struct StateWriter {
    store: AppStateStore,
    sequence: Arc<AtomicU64>,
    pending: Arc<Mutex<Option<PersistedAppState>>>,
    write_lock: Arc<Mutex<()>>,
}

impl StateWriter {
    #[must_use]
    pub fn new(store: AppStateStore) -> Self {
        Self {
            store,
            sequence: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(None)),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Programa una escritura diferida; descarta peticiones obsoletas antes de tocar disco.
    ///
    /// # Panics
    ///
    /// Si un mutex interno se envenena, lo que no debería ocurrir en uso normal.
    pub fn schedule(&self, state: PersistedAppState, debounce: Duration) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut pending = self
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse");
            *pending = Some(state);
        }

        let store = self.store.clone();
        let sequence_counter = Arc::clone(&self.sequence);
        let pending_state = Arc::clone(&self.pending);
        let write_lock = Arc::clone(&self.write_lock);

        thread::spawn(move || {
            thread::sleep(debounce);
            if sequence_counter.load(Ordering::SeqCst) != sequence {
                return;
            }
            let Some(state) = pending_state
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse")
                .take()
            else {
                return;
            };
            let _guard = write_lock
                .lock()
                .expect("el mutex de escritura no debe envenenarse");
            if sequence_counter.load(Ordering::SeqCst) != sequence {
                let mut pending = pending_state
                    .lock()
                    .expect("el mutex del estado pendiente no debe envenenarse");
                *pending = Some(state);
                return;
            }
            let _ = store.save(&state);
        });
    }

    /// Persiste de inmediato con tiempo de espera acotado; pensado para el cierre de ventana.
    ///
    /// # Panics
    ///
    /// Si un mutex interno se envenena, lo que no debería ocurrir en uso normal.
    pub fn flush(&self, state: PersistedAppState) -> Result<(), PersistenceError> {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        {
            let mut pending = self
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse");
            *pending = None;
        }

        let store = self.store.clone();
        let write_lock = Arc::clone(&self.write_lock);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);

        thread::spawn(move || {
            let _guard = write_lock
                .lock()
                .expect("el mutex de escritura no debe envenenarse");
            let result = store.save(&state);
            let _ = sender.send(result);
        });

        receiver
            .recv_timeout(CLOSE_FLUSH_TIMEOUT)
            .map_err(|_| PersistenceError::Io {
                path: self.store.state_path().to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "tiempo de espera agotado al guardar el estado en el cierre",
                ),
            })?
    }

    /// Tiempo máximo usado por `flush` en el cierre; útil para documentar el comportamiento.
    #[must_use]
    pub const fn close_flush_timeout() -> Duration {
        CLOSE_FLUSH_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, thread, time::Duration};

    use tempfile::tempdir;

    use super::*;
    use crate::persistence::WindowPlacement;

    #[test]
    fn latest_scheduled_state_is_not_overwritten_by_an_older_write() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store.clone());

        let first = PersistedAppState {
            recent_repositories: vec![PathBuf::from("first")],
            ..PersistedAppState::default()
        };
        let second = PersistedAppState {
            recent_repositories: vec![PathBuf::from("second")],
            ..PersistedAppState::default()
        };

        writer.schedule(first, Duration::from_millis(30));
        thread::sleep(Duration::from_millis(5));
        writer.schedule(second, Duration::from_millis(30));
        thread::sleep(Duration::from_millis(80));

        let loaded = store.load().expect("debe cargar");
        assert_eq!(
            loaded.state.recent_repositories,
            vec![PathBuf::from("second")]
        );
    }

    #[test]
    fn flush_persists_immediately_without_waiting_for_debounce() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store.clone());

        writer.schedule(
            PersistedAppState {
                recent_repositories: vec![PathBuf::from("stale")],
                ..PersistedAppState::default()
            },
            Duration::from_millis(200),
        );

        let flushed = PersistedAppState {
            window_placement: Some(WindowPlacement {
                x: 12.0,
                y: 24.0,
                width: 960.0,
                height: 640.0,
            }),
            ..PersistedAppState::default()
        };
        writer.flush(flushed.clone()).expect("debe guardar");

        let loaded = store.load().expect("debe cargar");
        assert_eq!(loaded.state.window_placement, flushed.window_placement);
        assert!(loaded.state.recent_repositories.is_empty());
    }

    #[test]
    fn flush_from_multiple_threads_leaves_a_consistent_final_state() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store.clone());

        let handles = (0..4).map(|index| {
            let writer = writer.clone();
            thread::spawn(move || {
                writer
                    .flush(PersistedAppState {
                        recent_repositories: vec![PathBuf::from(format!("repo-{index}"))],
                        ..PersistedAppState::default()
                    })
                    .expect("debe guardar");
            })
        });

        for handle in handles {
            handle.join().expect("debe terminar");
        }

        let loaded = store.load().expect("debe cargar");
        assert_eq!(loaded.state.recent_repositories.len(), 1);
        assert!(
            loaded.state.recent_repositories[0]
                .to_string_lossy()
                .starts_with("repo-")
        );
    }
}
