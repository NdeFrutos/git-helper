use std::{
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
    AppSettings, AppState, ChangeSelectionState, MutationState, RefreshCoordinator, RefreshState,
    RepositoryId, RepositorySession, RepositorySnapshot, RepositoryView,
};

const CURRENT_SCHEMA_VERSION: u32 = 1;
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
            settings: AppSettings::default(),
            window_placement: None,
        }
    }
}

impl PersistedAppState {
    /// Extrae únicamente datos permitidos del modelo en ejecución.
    #[must_use]
    pub fn from_app_state(app_state: &AppState, window_placement: Option<WindowPlacement>) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            repositories: app_state
                .repositories
                .iter()
                .map(|repository| PersistedRepository {
                    id: repository.id,
                    root_path: repository.root_path.clone(),
                    selected_view: repository.selected_view,
                })
                .collect(),
            active_repository_id: app_state.active_repository_id,
            recent_repositories: app_state.recent_repositories.clone(),
            settings: app_state.settings.clone(),
            window_placement,
        }
    }

    /// Reconstruye sesiones vacías que recibirán un refresh posterior.
    #[must_use]
    pub fn into_app_state(self) -> AppState {
        let repositories = self
            .repositories
            .into_iter()
            .map(|repository| RepositorySession {
                id: repository.id,
                root_path: repository.root_path,
                snapshot: Arc::new(RepositorySnapshot::default()),
                selected_view: repository.selected_view,
                change_selection: ChangeSelectionState::default(),
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
            })
            .collect();
        AppState {
            repositories,
            active_repository_id: self.active_repository_id,
            recent_repositories: self.recent_repositories,
            settings: self.settings,
        }
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

fn migrate(mut state: PersistedAppState) -> PersistedAppState {
    state.schema_version = CURRENT_SCHEMA_VERSION;
    state
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::domain::{AppState, RepositorySession, RepositoryView};

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
        let persisted = PersistedAppState::from_app_state(
            &app_state,
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
}
