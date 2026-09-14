use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::{AppStateStore, PersistedAppState, PersistenceError};

const CLOSE_FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// Petición pendiente de escritura; solo existe una a la vez.
#[derive(Debug)]
struct Pending {
    state: Option<PersistedAppState>,
    deadline: Option<Instant>,
    /// Se incrementa en cada `schedule`/`flush` para descartar escrituras obsoletas.
    generation: u64,
    shutdown: bool,
}

#[derive(Debug)]
struct Shared {
    pending: Mutex<Pending>,
    signal: Condvar,
    write_lock: Mutex<()>,
    /// Hilos creados por este escritor; debe quedarse en uno salvo en los cierres.
    spawned_threads: AtomicUsize,
}

/// Hilo trabajador único; se detiene al soltar el último clon de `StateWriter`.
#[derive(Debug)]
struct Worker {
    shared: Arc<Shared>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse");
            pending.shutdown = true;
        }
        self.shared.signal.notify_all();
        // Soltar el `JoinHandle` desacopla el worker. Esperarlo aquí invalidaría el
        // plazo de cierre si el almacenamiento está bloqueado; `shutdown` hará que
        // termine en cuanto vuelva del I/O en curso.
        drop(
            self.handle
                .lock()
                .expect("el mutex del hilo trabajador no debe envenenarse")
                .take(),
        );
    }
}

/// Escritor único con coalescencia: la última versión programada siempre gana.
///
/// Todas las escrituras diferidas las realiza **un solo** hilo trabajador, de modo
/// que pulsar teclas en el mensaje de commit o arrastrar la ventana solo actualiza
/// el estado pendiente y reprograma el vencimiento en lugar de crear hilos.
#[derive(Clone, Debug)]
pub struct StateWriter {
    store: AppStateStore,
    /// Mantiene vivo el hilo trabajador y da acceso al estado compartido.
    worker: Arc<Worker>,
}

impl StateWriter {
    #[must_use]
    pub fn new(store: AppStateStore) -> Self {
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                state: None,
                deadline: None,
                generation: 0,
                shutdown: false,
            }),
            signal: Condvar::new(),
            write_lock: Mutex::new(()),
            spawned_threads: AtomicUsize::new(0),
        });
        let handle = spawn_worker(store.clone(), Arc::clone(&shared));
        Self {
            store,
            worker: Arc::new(Worker {
                shared,
                handle: Mutex::new(Some(handle)),
            }),
        }
    }

    fn shared(&self) -> &Shared {
        &self.worker.shared
    }

    /// Programa una escritura diferida; descarta peticiones obsoletas antes de tocar disco.
    ///
    /// # Panics
    ///
    /// Si un mutex interno se envenena, lo que no debería ocurrir en uso normal.
    pub fn schedule(&self, state: PersistedAppState, debounce: Duration) {
        {
            let mut pending = self
                .shared()
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse");
            pending.state = Some(state);
            pending.deadline = Some(Instant::now() + debounce);
            pending.generation += 1;
        }
        self.shared().signal.notify_all();
    }

    /// Persiste de inmediato con tiempo de espera acotado; pensado para el cierre de ventana.
    ///
    /// # Panics
    ///
    /// Si un mutex interno se envenena, lo que no debería ocurrir en uso normal.
    pub fn flush(&self, state: PersistedAppState) -> Result<(), PersistenceError> {
        {
            let mut pending = self
                .shared()
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse");
            pending.state = None;
            pending.deadline = None;
            pending.generation += 1;
        }
        self.shared().signal.notify_all();

        let store = self.store.clone();
        let shared = Arc::clone(&self.worker.shared);
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);

        shared.spawned_threads.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            let _guard = shared
                .write_lock
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

    /// Número de hilos creados por este escritor; `schedule` nunca debe aumentarlo.
    #[must_use]
    pub fn spawned_threads(&self) -> usize {
        self.shared().spawned_threads.load(Ordering::SeqCst)
    }

    /// Tiempo máximo usado por `flush` en el cierre; útil para documentar el comportamiento.
    #[must_use]
    pub const fn close_flush_timeout() -> Duration {
        CLOSE_FLUSH_TIMEOUT
    }
}

/// Espera vencimientos en un único hilo y escribe siempre la última versión programada.
fn spawn_worker(store: AppStateStore, shared: Arc<Shared>) -> JoinHandle<()> {
    shared.spawned_threads.fetch_add(1, Ordering::SeqCst);
    thread::spawn(move || {
        loop {
            let Some((state, generation)) = wait_for_due_state(&shared) else {
                return;
            };
            let _guard = shared
                .write_lock
                .lock()
                .expect("el mutex de escritura no debe envenenarse");
            let is_stale = shared
                .pending
                .lock()
                .expect("el mutex del estado pendiente no debe envenenarse")
                .generation
                != generation;
            if is_stale {
                continue;
            }
            let _ = store.save(&state);
        }
    })
}

/// Bloquea hasta que hay estado pendiente vencido; devuelve `None` al apagar el escritor.
fn wait_for_due_state(shared: &Shared) -> Option<(PersistedAppState, u64)> {
    let mut pending = shared
        .pending
        .lock()
        .expect("el mutex del estado pendiente no debe envenenarse");
    loop {
        if pending.shutdown {
            return None;
        }
        match (pending.state.is_some(), pending.deadline) {
            (true, Some(deadline)) => {
                let now = Instant::now();
                if now >= deadline {
                    pending.deadline = None;
                    let generation = pending.generation;
                    let state = pending
                        .state
                        .take()
                        .expect("la rama solo se toma con estado pendiente");
                    return Some((state, generation));
                }
                pending = shared
                    .signal
                    .wait_timeout(pending, deadline - now)
                    .expect("el mutex del estado pendiente no debe envenenarse")
                    .0;
            }
            _ => {
                pending = shared
                    .signal
                    .wait(pending)
                    .expect("el mutex del estado pendiente no debe envenenarse");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

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
        thread::sleep(Duration::from_millis(150));

        let loaded = store.load().expect("debe cargar");
        assert_eq!(
            loaded.state.recent_repositories,
            vec![PathBuf::from("second")]
        );
    }

    #[test]
    fn high_frequency_scheduling_reuses_a_single_worker_thread() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store.clone());
        assert_eq!(writer.spawned_threads(), 1);

        for index in 0..500 {
            writer.schedule(
                PersistedAppState {
                    recent_repositories: vec![PathBuf::from(format!("repo-{index}"))],
                    ..PersistedAppState::default()
                },
                Duration::from_millis(20),
            );
        }

        assert_eq!(
            writer.spawned_threads(),
            1,
            "schedule no debe crear un hilo por evento"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let loaded = store.load().expect("debe cargar");
            if loaded.state.recent_repositories == vec![PathBuf::from("repo-499")] {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "el escritor no llegó a persistir"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn dropping_the_last_clone_stops_the_worker_thread() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store);
        let clone = writer.clone();

        drop(writer);
        // El hilo sigue vivo mientras exista un clon; soltarlo debe terminar sin bloquear.
        drop(clone);
    }

    #[test]
    fn dropping_writer_never_waits_for_a_blocked_disk_write() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let writer = StateWriter::new(store);
        let shared = Arc::clone(&writer.worker.shared);
        let write_guard = shared
            .write_lock
            .lock()
            .expect("el mutex de escritura no debe envenenarse");

        writer.schedule(PersistedAppState::default(), Duration::ZERO);
        thread::sleep(Duration::from_millis(20));

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let drop_thread = thread::spawn(move || {
            drop(writer);
            let _ = sender.send(());
        });
        let result = receiver.recv_timeout(Duration::from_millis(100));

        drop(write_guard);
        drop_thread.join().expect("el hilo de drop debe terminar");
        assert!(
            result.is_ok(),
            "soltar el escritor no debe esperar al worker bloqueado en disco"
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
