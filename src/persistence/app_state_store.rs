use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::warn;

use crate::domain::{
    AppSettings, AppState, ChangeCounters, HistorySnapshot, MutationState, RefreshCoordinator,
    RefreshState, RepositoryId, RepositorySession, RepositoryView, SshCloneMapping,
    WorkingTreeSnapshot,
};

/// Versión escrita por esta build. Al añadir un paso nuevo, súbela en uno y añade
/// su brazo en `migrate`; nunca reutilices un número ya publicado por otra rama.
const CURRENT_SCHEMA_VERSION: u32 = 3;
const APPLICATION_DIRECTORY: &str = "GitHelper";
const STATE_FILE_NAME: &str = "state.json";

/// Posición y dimensiones lógicas de la ventana.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Parte persistente de una sesión; excluye snapshots y operaciones.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedRepository {
    pub id: RepositoryId,
    pub root_path: PathBuf,
    pub selected_view: RepositoryView,
    /// Borrador local del mensaje de commit; solo se limpia tras un commit exitoso.
    #[serde(default)]
    pub commit_draft: Option<String>,
}

/// Esquema versionado escrito en `%LOCALAPPDATA%`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedAppState {
    pub schema_version: u32,
    #[serde(default)]
    pub repositories: Vec<PersistedRepository>,
    pub active_repository_id: Option<RepositoryId>,
    #[serde(default)]
    pub recent_repositories: Vec<PathBuf>,
    #[serde(default)]
    pub ssh_clone_mappings: Vec<SshCloneMapping>,
    #[serde(default)]
    pub settings: AppSettings,
    pub window_placement: Option<WindowPlacement>,
}

impl Default for PersistedAppState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            repositories: Vec::new(),
            active_repository_id: None,
            recent_repositories: Vec::new(),
            ssh_clone_mappings: Vec::new(),
            settings: AppSettings::default(),
            window_placement: None,
        }
    }
}

impl PersistedAppState {
    /// Extrae únicamente datos permitidos del modelo en ejecución.
    #[must_use]
    pub fn from_app_state(
        app_state: &AppState,
        commit_drafts: &HashMap<RepositoryId, String>,
        window_placement: Option<WindowPlacement>,
    ) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            repositories: app_state
                .repositories
                .iter()
                .map(|repository| {
                    let commit_draft = commit_drafts
                        .get(&repository.id)
                        .filter(|draft| !draft.is_empty())
                        .cloned();
                    PersistedRepository {
                        id: repository.id,
                        root_path: repository.root_path.clone(),
                        selected_view: repository.selected_view,
                        commit_draft,
                    }
                })
                .collect(),
            active_repository_id: app_state.active_repository_id,
            recent_repositories: app_state.recent_repositories.clone(),
            ssh_clone_mappings: app_state.ssh_clone_mappings.clone(),
            settings: app_state.settings.clone(),
            window_placement,
        }
    }

    /// Reconstruye sesiones vacías que recibirán un refresh posterior.
    #[must_use]
    pub fn into_app_state(self) -> (AppState, HashMap<RepositoryId, String>) {
        let mut commit_drafts = HashMap::new();
        let repositories = self
            .repositories
            .into_iter()
            .map(|repository| {
                if let Some(draft) = repository.commit_draft.filter(|draft| !draft.is_empty()) {
                    commit_drafts.insert(repository.id, draft);
                }
                RepositorySession {
                    id: repository.id,
                    root_path: repository.root_path,
                    working_tree: Arc::new(WorkingTreeSnapshot::default()),
                    history: Arc::new(HistorySnapshot::empty()),
                    change_counters: ChangeCounters::default(),
                    selected_view: repository.selected_view,
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
                    path_accessible: true,
                }
            })
            .collect();
        (
            AppState {
                repositories,
                active_repository_id: self.active_repository_id,
                recent_repositories: self.recent_repositories,
                ssh_clone_mappings: self.ssh_clone_mappings,
                settings: self.settings,
            },
            commit_drafts,
        )
    }
}

/// Resultado tolerante de una lectura; un JSON corrupto produce estado vacío.
#[derive(Clone, Debug)]
pub struct LoadedState {
    pub state: PersistedAppState,
    pub corruption_backup: Option<PathBuf>,
}

/// Fallos de almacenamiento que no deben cerrar la aplicación.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("no se pudo determinar la carpeta local de la aplicación")]
    LocalDataDirectoryUnavailable,
    #[error("no se pudo acceder a {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no se pudo serializar el estado: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("la versión de estado {found} es posterior a la soportada {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("no se pudo persistir el archivo temporal: {0}")]
    Persist(#[from] tempfile::PersistError),
}

/// Almacén JSON con reemplazo atómico.
#[derive(Clone, Debug)]
pub struct AppStateStore {
    state_path: PathBuf,
}

impl AppStateStore {
    /// Usa exactamente `%LOCALAPPDATA%\GitHelper\state.json` en Windows.
    pub fn default_location() -> Result<Self, PersistenceError> {
        let local_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| BaseDirs::new().map(|directories| directories.data_local_dir().to_owned()))
            .ok_or(PersistenceError::LocalDataDirectoryUnavailable)?;
        Ok(Self::new(
            local_data.join(APPLICATION_DIRECTORY).join(STATE_FILE_NAME),
        ))
    }

    /// Permite usar una ubicación aislada en pruebas.
    #[must_use]
    pub fn new(state_path: PathBuf) -> Self {
        Self { state_path }
    }

    /// Devuelve la ubicación del archivo de estado.
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Lee, valida y migra el estado disponible.
    pub fn load(&self) -> Result<LoadedState, PersistenceError> {
        if !self.state_path.exists() {
            return Ok(LoadedState {
                state: PersistedAppState::default(),
                corruption_backup: None,
            });
        }
        let mut file = File::open(&self.state_path).map_err(|source| PersistenceError::Io {
            path: self.state_path.clone(),
            source,
        })?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .map_err(|source| PersistenceError::Io {
                path: self.state_path.clone(),
                source,
            })?;

        let parsed = serde_json::from_slice::<PersistedAppState>(&contents);
        let state = match parsed {
            Ok(state) => state,
            Err(parse_error) => {
                let backup_path = self.back_up_corrupt_state()?;
                warn!(
                    error = %parse_error,
                    backup = %backup_path.display(),
                    "Se recuperó un estado persistido corrupto"
                );
                return Ok(LoadedState {
                    state: PersistedAppState::default(),
                    corruption_backup: Some(backup_path),
                });
            }
        };
        if state.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(PersistenceError::UnsupportedSchema {
                found: state.schema_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        Ok(LoadedState {
            state: migrate(state),
            corruption_backup: None,
        })
    }

    /// Escribe y sincroniza un temporal antes de sustituir el estado.
    pub fn save(&self, state: &PersistedAppState) -> Result<(), PersistenceError> {
        let parent = self
            .state_path
            .parent()
            .ok_or(PersistenceError::LocalDataDirectoryUnavailable)?;
        fs::create_dir_all(parent).map_err(|source| PersistenceError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let serialized = serde_json::to_vec_pretty(state)?;
        let mut temporary =
            NamedTempFile::new_in(parent).map_err(|source| PersistenceError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        temporary
            .write_all(&serialized)
            .map_err(|source| PersistenceError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| PersistenceError::Io {
                path: temporary.path().to_path_buf(),
                source,
            })?;
        temporary.persist(&self.state_path)?;
        Ok(())
    }

    fn back_up_corrupt_state(&self) -> Result<PathBuf, PersistenceError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup_path = self
            .state_path
            .with_file_name(format!("state.corrupt-{timestamp}.json"));
        fs::rename(&self.state_path, &backup_path).map_err(|source| PersistenceError::Io {
            path: self.state_path.clone(),
            source,
        })?;
        Ok(backup_path)
    }
}

/// Aplica los pasos de migración uno a uno hasta alcanzar la versión actual.
///
/// Cada paso solo conoce su propia transición (`n` → `n + 1`), de modo que una rama
/// que añada otro paso encima solo tiene que registrar su brazo y subir
/// `CURRENT_SCHEMA_VERSION`; no hay comparaciones contra una versión heredada fija.
fn migrate(mut state: PersistedAppState) -> PersistedAppState {
    while state.schema_version < CURRENT_SCHEMA_VERSION {
        match state.schema_version {
            0 | 1 => {
                normalize_ssh_clone_mappings(&mut state);
                state.schema_version = 2;
            }
            2 => {
                normalize_commit_drafts(&mut state);
                state.schema_version = 3;
            }
            unknown => {
                warn!(
                    version = unknown,
                    "No hay paso de migración registrado; se adopta la versión actual"
                );
                state.schema_version = CURRENT_SCHEMA_VERSION;
            }
        }
    }
    // Ambas normalizaciones son idempotentes y reparan estados escritos por
    // builds intermedias que compartieron temporalmente el mismo número.
    normalize_ssh_clone_mappings(&mut state);
    normalize_commit_drafts(&mut state);
    state
}

/// v2 → v3: los borradores de commit pasan a persistirse; normaliza los vacíos a `None`.
fn normalize_commit_drafts(state: &mut PersistedAppState) {
    for repository in &mut state.repositories {
        if repository
            .commit_draft
            .as_deref()
            .is_some_and(str::is_empty)
        {
            repository.commit_draft = None;
        }
    }
}

/// Descarta mapeos SSH incompletos o duplicados heredados de versiones previas.
fn normalize_ssh_clone_mappings(state: &mut PersistedAppState) {
    let mut seen = Vec::new();
    state.ssh_clone_mappings.retain(|mapping| {
        if mapping.ssh_url_normalized.trim().is_empty()
            || mapping.local_path.as_os_str().is_empty()
            || seen.contains(&mapping.ssh_url_normalized)
        {
            return false;
        }
        seen.push(mapping.ssh_url_normalized.clone());
        true
    });
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::domain::{AppState, RepositorySession, RepositoryView, SshCloneMapping};

    use super::{AppStateStore, PersistedAppState, WindowPlacement};

    #[test]
    fn saves_and_restores_sessions() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let mut app_state = AppState::default();
        let mut repository = RepositorySession::new(PathBuf::from(r"C:\repo con ñ"));
        repository.selected_view = RepositoryView::History;
        app_state.active_repository_id = Some(repository.id);
        app_state.repositories.push(repository);
        let mut commit_drafts = HashMap::new();
        commit_drafts.insert(
            app_state.repositories[0].id,
            "feat: borrador persistido".to_owned(),
        );
        let persisted = PersistedAppState::from_app_state(
            &app_state,
            &commit_drafts,
            Some(WindowPlacement {
                x: 10.0,
                y: 20.0,
                width: 960.0,
                height: 640.0,
            }),
        );

        store.save(&persisted).expect("debe guardar");
        store
            .save(&persisted)
            .expect("debe sustituir el estado de forma atómica");
        let loaded = store.load().expect("debe cargar");

        assert_eq!(loaded.state, persisted);
        assert!(loaded.corruption_backup.is_none());
    }

    #[test]
    fn migrates_legacy_schema_without_commit_drafts() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        let store = AppStateStore::new(state_path.clone());
        let mut app_state = AppState::default();
        let repository = RepositorySession::new(PathBuf::from("legacy-repo"));
        let repository_id = repository.id;
        app_state.repositories.push(repository);
        app_state.active_repository_id = Some(repository_id);
        let persisted = PersistedAppState::from_app_state(&app_state, &HashMap::new(), None);
        store.save(&persisted).expect("debe guardar");

        let mut legacy =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&state_path).unwrap())
                .expect("debe parsear el estado guardado");
        legacy["schema_version"] = 1.into();
        if let Some(repositories) = legacy
            .get_mut("repositories")
            .and_then(|value| value.as_array_mut())
        {
            for repository in repositories {
                if let Some(fields) = repository.as_object_mut() {
                    fields.remove("commit_draft");
                }
            }
        }
        fs::write(
            &state_path,
            serde_json::to_string_pretty(&legacy).expect("debe serializar"),
        )
        .expect("debe escribir el fixture legacy");

        let loaded = store.load().expect("debe migrar");

        assert_eq!(loaded.state.schema_version, super::CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.state.repositories.len(), 1);
        assert!(loaded.state.repositories[0].commit_draft.is_none());
    }

    #[test]
    fn migration_ladder_advances_step_by_step_to_the_current_version() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        let store = AppStateStore::new(state_path.clone());
        let persisted = PersistedAppState::from_app_state(
            &AppState::default(),
            &HashMap::new(),
            Some(WindowPlacement {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            }),
        );
        store.save(&persisted).expect("debe guardar");

        for stored_version in 0..super::CURRENT_SCHEMA_VERSION {
            let mut stored = serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(&state_path).expect("debe leer el estado"),
            )
            .expect("debe parsear el estado guardado");
            stored["schema_version"] = stored_version.into();
            fs::write(
                &state_path,
                serde_json::to_string_pretty(&stored).expect("debe serializar"),
            )
            .expect("debe escribir el fixture");

            let loaded = store.load().expect("debe migrar");

            assert_eq!(loaded.state.schema_version, super::CURRENT_SCHEMA_VERSION);
            assert_eq!(loaded.state.window_placement, persisted.window_placement);
        }
    }

    #[test]
    fn rejects_state_written_by_a_newer_schema() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        let store = AppStateStore::new(state_path.clone());
        let persisted = PersistedAppState {
            schema_version: super::CURRENT_SCHEMA_VERSION + 1,
            ..PersistedAppState::default()
        };
        store.save(&persisted).expect("debe guardar");

        let error = store.load().expect_err("no debe aceptar un esquema futuro");

        assert!(matches!(
            error,
            super::PersistenceError::UnsupportedSchema { .. }
        ));
    }

    #[test]
    fn backs_up_corrupt_json_and_returns_empty_state() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        fs::write(&state_path, b"{ no es json").expect("debe escribir el fixture");
        let store = AppStateStore::new(state_path);

        let loaded = store.load().expect("debe recuperarse");

        assert!(loaded.state.repositories.is_empty());
        assert!(loaded.corruption_backup.is_some());
        assert!(!store.state_path().exists());
    }

    #[test]
    fn migrates_legacy_state_without_ssh_mappings() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        fs::write(
            &state_path,
            br#"{"schema_version":1,"repositories":[],"recent_repositories":[],"settings":{}}"#,
        )
        .expect("debe escribir el fixture legacy");
        let store = AppStateStore::new(state_path);

        let loaded = store.load().expect("debe migrar");

        assert_eq!(loaded.state.schema_version, super::CURRENT_SCHEMA_VERSION);
        assert!(loaded.state.ssh_clone_mappings.is_empty());
    }

    #[test]
    fn migrates_v2_state_without_losing_ssh_defaults() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let state_path = temporary_directory.path().join("state.json");
        fs::write(
            &state_path,
            br#"{"schema_version":2,"repositories":[],"recent_repositories":[],"settings":{}}"#,
        )
        .expect("debe escribir el fixture v2 de otro cambio");
        let store = AppStateStore::new(state_path);

        let loaded = store.load().expect("debe cargar sin perder datos");

        assert_eq!(loaded.state.schema_version, super::CURRENT_SCHEMA_VERSION);
        assert!(loaded.state.ssh_clone_mappings.is_empty());
    }

    #[test]
    fn persists_ssh_clone_mappings() {
        let temporary_directory = tempdir().expect("debe crear el temporal");
        let store = AppStateStore::new(temporary_directory.path().join("state.json"));
        let mut app_state = AppState::default();
        app_state.ssh_clone_mappings.push(SshCloneMapping {
            ssh_url_normalized: "ssh://github.com/org/repo".to_owned(),
            local_path: PathBuf::from(r"C:\GitHelper\repos\repo"),
        });
        let persisted = PersistedAppState::from_app_state(&app_state, &HashMap::new(), None);
        store.save(&persisted).expect("debe guardar");
        let loaded = store.load().expect("debe cargar");
        assert_eq!(
            loaded.state.ssh_clone_mappings,
            persisted.ssh_clone_mappings
        );
    }
}
