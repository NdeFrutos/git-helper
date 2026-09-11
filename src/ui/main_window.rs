use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, PathPromptOptions, PromptButton, PromptLevel,
    Render, Subscription, Window, div, prelude::*, px, uniform_list,
};

use crate::{
    actions::{
        CloseActiveRepository, CreateCommit, GenerateCommitMessage, NextRepository, OpenRepository,
        PreviousRepository, RefreshRepository, ShowChanges, ShowHistory,
    },
    cursor::{
        CommitMessageRequest, CursorClient, GenerationApplyDecision, build_cursor_context,
        resolve_cursor_executable, validate_generation_result,
    },
    domain::{
        AppState, BranchKind, BranchReference, BranchUpstream, ChangeKind, CommitDetails,
        FileChange, HeadState, MutationState, OperationKind, RefreshState, RepositoryId,
        RepositorySession, RepositorySnapshot, RepositoryView,
    },
    git::{DiscardPlan, GitClient, GitError, plan_discard, plan_fetch, plan_pull, plan_push},
    persistence::{AppStateStore, LoadedState, PersistedAppState, StateWriter, WindowPlacement},
    process::CancellationToken,
    watcher::RepositoryWatcher,
};

use super::{
    commit_input::{CommitInput, CommitMessageChanged},
    theme::{
        ACCENT_COLOR, BACKGROUND_COLOR, BORDER_COLOR, ELEVATED_BACKGROUND_COLOR, ERROR_COLOR,
        HOVER_BACKGROUND_COLOR, MUTED_TEXT_COLOR, PRIMARY_TEXT_COLOR, SELECTED_BACKGROUND_COLOR,
        SUCCESS_COLOR, WARNING_COLOR,
    },
};

const INITIAL_HISTORY_LIMIT: usize = 200;
const SAVE_DEBOUNCE_DURATION: Duration = Duration::from_secs(1);
const CHANGE_GROUP_ROW_HEIGHT_PX: u16 = 40;
const CHANGE_FILE_ROW_HEIGHT_PX: u16 = 40;

#[derive(Clone)]
enum ChangeListRow {
    Group {
        title: &'static str,
        count: usize,
        action: Option<GroupAction>,
        representation: ChangeRepresentation,
        is_collapsed: bool,
    },
    File {
        change: FileChange,
        representation: ChangeRepresentation,
    },
}

impl ChangeListRow {
    const fn height_px(&self) -> u16 {
        match self {
            Self::Group { .. } => CHANGE_GROUP_ROW_HEIGHT_PX,
            Self::File { .. } => CHANGE_FILE_ROW_HEIGHT_PX,
        }
    }
}

#[derive(Clone, Copy)]
enum GroupAction {
    StageAll,
    UnstageAll,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum ChangeRepresentation {
    Conflict,
    Staged,
    Worktree,
    Untracked,
}

#[derive(Clone, Copy, Default)]
struct RefreshOutcome {
    succeeded: bool,
    should_notify: bool,
    reload_history: bool,
    continuation: RefreshContinuation,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RefreshContinuation {
    #[default]
    Complete,
    Repeat,
}

struct PreparedRefresh {
    generation: u64,
    include_history: bool,
    root_path: PathBuf,
    cancellation: CancellationToken,
}

enum GenerationPreparationError {
    StagedChanged,
    Message(String),
}

struct GenerationCompletion {
    request: CommitMessageRequest,
    expected_index_identity: Option<Vec<u8>>,
    result: Result<String, String>,
    current_index_identity: Option<Result<Vec<u8>, String>>,
    was_cancelled: bool,
    staged_changed: bool,
}

/// Estado leído una sola vez durante el arranque junto al almacén que lo produjo.
pub struct StartupState {
    pub store: AppStateStore,
    pub loaded: LoadedState,
}

/// Modelo y presentación de la ventana principal.
pub struct MainWindow {
    /// Estado leído una sola vez en el arranque; `initialize` lo consume.
    loaded_state: Option<LoadedState>,
    state: AppState,
    git_client: GitClient,
    cursor_client: CursorClient,
    state_writer: Option<StateWriter>,
    window_placement: Option<WindowPlacement>,
    commit_inputs: HashMap<RepositoryId, Entity<CommitInput>>,
    selected_commit_details: HashMap<RepositoryId, CommitDetails>,
    commit_input_subscriptions: HashMap<RepositoryId, Subscription>,
    window_subscriptions: Vec<Subscription>,
    active_refresh_cancellations: HashMap<RepositoryId, CancellationToken>,
    active_mutation_cancellations: HashMap<RepositoryId, CancellationToken>,
    generation_requests: HashMap<RepositoryId, CommitMessageRequest>,
    next_generation_request_id: u64,
    repository_watchers: HashMap<RepositoryId, RepositoryWatcher>,
    pending_refreshes: HashSet<RepositoryId>,
    global_refresh_in_flight: Option<RepositoryId>,
    collapsed_groups: HashSet<(RepositoryId, ChangeRepresentation)>,
    change_rows: HashMap<RepositoryId, Arc<Vec<ChangeListRow>>>,
    git_version: Option<String>,
    global_status_message: String,
    global_error: Option<String>,
}

#[allow(
    clippy::assigning_clones,
    clippy::too_many_lines,
    clippy::unused_self,
    reason = "La entidad GPUI conserva métodos listener y render cohesionados; los mensajes cortos priorizan legibilidad"
)]
impl MainWindow {
    /// Crea la ventana con el estado ya leído en el arranque; no vuelve a tocar disco.
    ///
    /// `startup` procede de la única lectura de `state.json` que hace `app::run`,
    /// de modo que el respaldo por corrupción se detecta y se muestra una sola vez.
    #[must_use]
    pub fn new(startup: Option<StartupState>, _cx: &mut Context<Self>) -> Self {
        let (state_writer, loaded_state) = startup.map_or((None, None), |startup| {
            (Some(StateWriter::new(startup.store)), Some(startup.loaded))
        });
        let state = AppState::default();
        Self {
            loaded_state,
            state,
            git_client: GitClient::default(),
            cursor_client: CursorClient::new(PathBuf::from("agent")),
            state_writer,
            window_placement: None,
            commit_inputs: HashMap::new(),
            selected_commit_details: HashMap::new(),
            commit_input_subscriptions: HashMap::new(),
            window_subscriptions: Vec::new(),
            active_refresh_cancellations: HashMap::new(),
            active_mutation_cancellations: HashMap::new(),
            generation_requests: HashMap::new(),
            next_generation_request_id: 0,
            repository_watchers: HashMap::new(),
            pending_refreshes: HashSet::new(),
            global_refresh_in_flight: None,
            collapsed_groups: HashSet::new(),
            change_rows: HashMap::new(),
            git_version: None,
            global_status_message: "Preparando Git Helper…".to_owned(),
            global_error: None,
        }
    }

    /// Detecta Git y refresca todas las pestañas restauradas en background.
    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_placement = Some(capture_window_placement(window));
        let activation_subscription = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                let Some(repository_id) = this.state.active_repository_id else {
                    return;
                };
                this.force_refresh_repository(repository_id, cx);
                return;
            }
            this.save_state(cx);
        });
        self.window_subscriptions.push(activation_subscription);
        let bounds_subscription = cx.observe_window_bounds(window, |this, window, cx| {
            this.window_placement = Some(capture_window_placement(window));
            this.save_state(cx);
        });
        self.window_subscriptions.push(bounds_subscription);
        let entity = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    this.window_placement = Some(capture_window_placement(window));
                    if let Err(error) = this.flush_state(cx) {
                        this.global_error =
                            Some(format!("No se pudo guardar el estado al cerrar: {error}"));
                    }
                })
                .ok();
            true
        });

        if let Some(loaded) = self.loaded_state.take() {
            let corruption_backup = loaded.corruption_backup;
            let persisted = loaded.state;
            cx.spawn(async move |this, cx| {
                let (mut loaded_state, commit_drafts, cursor_executable) = cx
                    .background_spawn(async move {
                        let (mut state, commit_drafts) = persisted.into_app_state();
                        for repository in &mut state.repositories {
                            repository.path_accessible = repository.root_path.is_dir();
                            if !repository.path_accessible {
                                repository.status_message =
                                    "Repositorio no disponible; comprueba la ruta o el disco"
                                        .to_owned();
                                repository.error = Some(
                                    "La ruta del repositorio no responde. Puedes reintentar o cerrar la pestaña."
                                        .to_owned(),
                                );
                            }
                        }
                        if state.active_repository_id.is_some_and(|active_id| {
                            !state
                                .repositories
                                .iter()
                                .any(|repository| repository.id == active_id)
                        }) {
                            state.active_repository_id =
                                state.repositories.first().map(|repository| repository.id);
                        }
                        let cursor_executable =
                            resolve_cursor_executable(state.settings.cursor_cli_path.clone());
                        (state, commit_drafts, cursor_executable)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let current_active = this.state.active_repository_id;
                    loaded_state.repositories.retain(|loaded_repository| {
                        !this.state.repositories.iter().any(|current_repository| {
                            normalized_path_key(&current_repository.root_path)
                                == normalized_path_key(&loaded_repository.root_path)
                        })
                    });
                    let restored = loaded_state
                        .repositories
                        .iter()
                        .map(|repository| (repository.id, repository.path_accessible))
                        .collect::<Vec<_>>();
                    this.state
                        .repositories
                        .append(&mut loaded_state.repositories);
                    if current_active.is_none() {
                        this.state.active_repository_id = loaded_state.active_repository_id;
                    }
                    this.state.recent_repositories = loaded_state.recent_repositories;
                    this.state.settings = loaded_state.settings;
                    this.cursor_client = CursorClient::new(cursor_executable);
                    for (repository_id, path_accessible) in restored {
                        let draft = commit_drafts.get(&repository_id).cloned();
                        this.create_commit_input(repository_id, draft, cx);
                        if path_accessible {
                            if this.state.active_repository_id == Some(repository_id) {
                                this.refresh_repository(repository_id, cx);
                            } else {
                                this.pending_refreshes.insert(repository_id);
                            }
                        }
                    }
                    this.save_state(cx);
                    this.global_status_message = "Sesión restaurada".to_owned();
                    if let Some(backup_path) = corruption_backup {
                        this.global_error = Some(format!(
                            "El estado guardado estaba dañado; se empezó de cero y se conservó una copia en {}",
                            backup_path.display()
                        ));
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        let git_client = self.git_client.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.detect_version(&CancellationToken::default())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(version) => {
                        this.git_version = Some(version);
                        this.global_status_message = "Git detectado".to_owned();
                    }
                    Err(error) => {
                        this.global_error = Some(format!(
                            "{error}. Instala Git for Windows y pulsa Actualizar."
                        ));
                        this.global_status_message = "Git no está disponible".to_owned();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn active_repository(&self) -> Option<&RepositorySession> {
        let active_id = self.state.active_repository_id?;
        self.state
            .repositories
            .iter()
            .find(|repository| repository.id == active_id)
    }

    fn active_repository_mut(&mut self) -> Option<&mut RepositorySession> {
        let active_id = self.state.active_repository_id?;
        self.state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == active_id)
    }

    fn create_commit_input(
        &mut self,
        repository_id: RepositoryId,
        draft: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(CommitInput::new);
        if let Some(draft) = draft {
            input.update(cx, |input, cx| input.set_content(draft, cx));
        }
        let subscription = cx.subscribe(&input, |this, _input, _: &CommitMessageChanged, cx| {
            this.save_state(cx);
            cx.notify();
        });
        self.commit_inputs.insert(repository_id, input);
        self.commit_input_subscriptions
            .insert(repository_id, subscription);
    }

    fn commit_drafts(&self, cx: &App) -> HashMap<RepositoryId, String> {
        self.commit_inputs
            .iter()
            .map(|(repository_id, input)| (*repository_id, input.read(cx).content().to_owned()))
            .collect()
    }

    fn build_persisted_state(&self, cx: &App) -> PersistedAppState {
        PersistedAppState::from_app_state(
            &self.state,
            &self.commit_drafts(cx),
            self.window_placement,
        )
    }

    fn save_state(&mut self, cx: &mut Context<Self>) {
        let Some(writer) = self.state_writer.clone() else {
            return;
        };
        let state = self.build_persisted_state(cx);
        writer.schedule(state, SAVE_DEBOUNCE_DURATION);
    }

    fn flush_state(&self, cx: &App) -> Result<(), crate::persistence::PersistenceError> {
        let Some(writer) = self.state_writer.clone() else {
            return Ok(());
        };
        let state = self.build_persisted_state(cx);
        writer.flush(state)
    }

    fn retry_repository_access(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        repository.path_accessible = repository.root_path.is_dir();
        if repository.path_accessible {
            repository.error = None;
            repository.status_message = "Preparando repositorio…".to_owned();
            self.pending_refreshes.insert(repository_id);
            self.refresh_repository(repository_id, cx);
        } else {
            repository.error = Some(
                "La ruta del repositorio no responde. Puedes reintentar o cerrar la pestaña."
                    .to_owned(),
            );
            repository.status_message =
                "Repositorio no disponible; comprueba la ruta o el disco".to_owned();
        }
        self.save_state(cx);
        cx.notify();
    }

    fn open_recent_repository(&mut self, root_path: PathBuf, cx: &mut Context<Self>) {
        let git_client = self.git_client.clone();
        self.global_status_message = "Abriendo repositorio reciente…".to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.discover_repository(&root_path, &CancellationToken::default())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(discovered_path) => this.finish_open_repository(discovered_path, cx),
                    Err(error) => {
                        this.global_error = Some(error.to_string());
                        this.global_status_message =
                            "No se pudo abrir el repositorio reciente".to_owned();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_repository(&mut self, _: &OpenRepository, _: &mut Window, cx: &mut Context<Self>) {
        let path_receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Abrir repositorio Git".into()),
        });
        let git_client = self.git_client.clone();
        self.global_status_message = "Seleccionando repositorio…".to_owned();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = path_receiver.await else {
                return;
            };
            let Some(selected_path) = paths.into_iter().next() else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    git_client.discover_repository(&selected_path, &CancellationToken::default())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(root_path) => this.finish_open_repository(root_path, cx),
                    Err(error) => {
                        this.global_error = Some(error.to_string());
                        this.global_status_message = "No se pudo abrir el repositorio".to_owned();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn finish_open_repository(&mut self, root_path: PathBuf, cx: &mut Context<Self>) {
        let path_key = normalized_path_key(&root_path);
        if let Some(repository_id) = self
            .state
            .repositories
            .iter()
            .find(|repository| normalized_path_key(&repository.root_path) == path_key)
            .map(|repository| repository.id)
        {
            self.state.active_repository_id = Some(repository_id);
            self.global_status_message = "El repositorio ya estaba abierto".to_owned();
            self.save_state(cx);
            return;
        }
        let repository = RepositorySession::new(root_path.clone());
        let repository_id = repository.id;
        self.state.repositories.push(repository);
        self.state.active_repository_id = Some(repository_id);
        self.state
            .recent_repositories
            .retain(|recent| normalized_path_key(recent) != path_key);
        self.state.recent_repositories.insert(0, root_path);
        self.state.recent_repositories.truncate(10);
        self.create_commit_input(repository_id, None, cx);
        self.global_status_message = "Repositorio abierto".to_owned();
        self.global_error = None;
        self.save_state(cx);
        self.refresh_repository(repository_id, cx);
    }

    fn close_active_repository(
        &mut self,
        _: &CloseActiveRepository,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_id) = self.state.active_repository_id else {
            return;
        };
        self.close_repository(active_id, cx);
    }

    fn close_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let was_active = self.state.active_repository_id == Some(repository_id);
        if let Some(cancellation) = self.active_refresh_cancellations.remove(&repository_id) {
            cancellation.cancel();
        }
        if let Some(cancellation) = self.active_mutation_cancellations.remove(&repository_id) {
            cancellation.cancel();
        }
        let Some(index) = self
            .state
            .repositories
            .iter()
            .position(|repository| repository.id == repository_id)
        else {
            return;
        };
        self.state.repositories.remove(index);
        self.commit_inputs.remove(&repository_id);
        self.commit_input_subscriptions.remove(&repository_id);
        self.generation_requests.remove(&repository_id);
        self.selected_commit_details.remove(&repository_id);
        self.repository_watchers.remove(&repository_id);
        self.pending_refreshes.remove(&repository_id);
        if self.global_refresh_in_flight == Some(repository_id) {
            self.global_refresh_in_flight = None;
        }
        self.collapsed_groups.retain(|(id, _)| *id != repository_id);
        self.change_rows.remove(&repository_id);
        if was_active {
            self.state.active_repository_id = self
                .state
                .repositories
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|index| self.state.repositories.get(index))
                })
                .map(|repository| repository.id);
            if let Some(active_id) = self.state.active_repository_id {
                self.pending_refreshes.insert(active_id);
                self.refresh_repository(active_id, cx);
            }
        }
        self.global_status_message = "Pestaña cerrada".to_owned();
        self.save_state(cx);
        cx.notify();
    }

    fn next_repository(&mut self, _: &NextRepository, _: &mut Window, cx: &mut Context<Self>) {
        self.move_active_repository(1, cx);
    }

    fn previous_repository(
        &mut self,
        _: &PreviousRepository,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active_repository(-1, cx);
    }

    fn move_active_repository(&mut self, direction: isize, cx: &mut Context<Self>) {
        if self.state.repositories.is_empty() {
            return;
        }
        let current = self
            .state
            .active_repository_id
            .and_then(|active_id| {
                self.state
                    .repositories
                    .iter()
                    .position(|repository| repository.id == active_id)
            })
            .unwrap_or_default();
        let count = self.state.repositories.len().cast_signed();
        let next = (current.cast_signed() + direction)
            .rem_euclid(count)
            .cast_unsigned();
        let repository_id = self.state.repositories[next].id;
        self.state.active_repository_id = Some(repository_id);
        if self.pending_refreshes.contains(&repository_id) {
            self.force_refresh_repository(repository_id, cx);
        }
        self.save_state(cx);
        cx.notify();
    }

    fn refresh_active_repository(
        &mut self,
        _: &RefreshRepository,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(repository_id) = self.state.active_repository_id {
            self.force_refresh_repository(repository_id, cx);
        }
    }

    fn force_refresh_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        if let Some(root_path) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .map(|repository| repository.root_path.clone())
        {
            self.git_client.invalidate_remotes(&root_path);
        }
        let history_is_visible = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .is_some_and(|repository| repository.selected_view == RepositoryView::History);
        if history_is_visible {
            self.invalidate_history(repository_id);
        }
        self.refresh_repository(repository_id, cx);
    }

    fn refresh_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(refresh) = self.prepare_refresh(repository_id) else {
            return;
        };
        let PreparedRefresh {
            generation,
            include_history,
            root_path,
            cancellation,
        } = refresh;
        let git_client = self.git_client.clone();
        // La transición a carga debe ser visible aunque el snapshot aún no cambie.
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    if include_history {
                        git_client.snapshot_with_history(
                            &root_path,
                            INITIAL_HISTORY_LIMIT,
                            &cancellation,
                        )
                    } else {
                        git_client.snapshot(&root_path, &cancellation)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let outcome =
                    this.finish_refresh(repository_id, generation, include_history, result);
                if outcome.succeeded {
                    this.ensure_watcher(repository_id, cx);
                }
                if outcome.continuation == RefreshContinuation::Repeat {
                    this.pending_refreshes.insert(repository_id);
                }
                let next_refresh = this
                    .state
                    .active_repository_id
                    .filter(|active_id| this.pending_refreshes.contains(active_id));
                if let Some(next_refresh) = next_refresh {
                    this.refresh_repository(next_refresh, cx);
                } else if outcome.reload_history
                    && outcome.continuation == RefreshContinuation::Complete
                {
                    this.ensure_history_loaded(repository_id, cx);
                }
                if outcome.should_notify {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn prepare_refresh(&mut self, repository_id: RepositoryId) -> Option<PreparedRefresh> {
        if self
            .global_refresh_in_flight
            .is_some_and(|in_flight| in_flight != repository_id)
        {
            self.pending_refreshes.insert(repository_id);
            return None;
        }
        let repository = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)?;
        if !repository.path_accessible {
            return None;
        }
        if repository.is_refreshing() || repository.is_mutating() {
            repository.refresh_coordinator.mark_dirty();
            self.pending_refreshes.insert(repository_id);
            return None;
        }
        if !repository.refresh_coordinator.request() {
            self.pending_refreshes.insert(repository_id);
            return None;
        }
        self.pending_refreshes.remove(&repository_id);
        self.global_refresh_in_flight = Some(repository_id);
        repository.refresh_generation = repository.refresh_generation.saturating_add(1);
        let generation = repository.refresh_generation;
        repository.refresh_state = RefreshState::Running { generation };
        repository.status_message = "Actualizando estado…".to_owned();
        repository.error = None;
        let include_history =
            repository.selected_view == RepositoryView::History && !repository.history_loaded;
        if include_history {
            repository.history_generation = repository.history_generation.saturating_add(1);
            repository.history_loading = true;
        }
        let root_path = repository.root_path.clone();
        let cancellation = CancellationToken::default();
        self.active_refresh_cancellations
            .insert(repository_id, cancellation.clone());
        Some(PreparedRefresh {
            generation,
            include_history,
            root_path,
            cancellation,
        })
    }

    fn ensure_watcher(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        if self.repository_watchers.contains_key(&repository_id) {
            return;
        }
        let Some(root_path) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .map(|repository| repository.root_path.clone())
        else {
            return;
        };
        let directory_client = self.git_client.clone();
        let common_directory_client = self.git_client.clone();
        let ignored_paths_client = self.git_client.clone();
        cx.spawn(async move |this, cx| {
            let git_directory_task =
                cx.background_spawn({
                    let root_path = root_path.clone();
                    async move {
                        directory_client.git_directory(&root_path, &CancellationToken::default())
                    }
                });
            let common_git_directory_task = cx.background_spawn({
                let root_path = root_path.clone();
                async move {
                    common_directory_client
                        .git_common_directory(&root_path, &CancellationToken::default())
                }
            });
            let ignored_paths_task = cx.background_spawn({
                let root_path = root_path.clone();
                async move {
                    ignored_paths_client.ignored_paths(&root_path, &CancellationToken::default())
                }
            });
            let git_directory_result = git_directory_task.await;
            let common_git_directory = common_git_directory_task.await.ok();
            let ignored_paths = ignored_paths_task.await.unwrap_or_default();
            let Ok(git_directory) = git_directory_result else {
                return;
            };
            let (sender, receiver) = async_channel::unbounded();
            let watcher_result = RepositoryWatcher::start(
                &root_path,
                &git_directory,
                common_git_directory.as_deref(),
                ignored_paths,
                Arc::new(move |change| {
                    let _ = sender.try_send(change);
                }),
            );
            let Ok(watcher) = watcher_result else {
                return;
            };
            let installed = this
                .update(cx, |this, _| {
                    if !this.repository_session_is_open(repository_id) {
                        return false;
                    }
                    this.repository_watchers.insert(repository_id, watcher);
                    true
                })
                .unwrap_or(false);
            if !installed {
                return;
            }
            while let Ok(change) = receiver.recv().await {
                let should_continue = this
                    .update(cx, |this, cx| {
                        if !this.repository_session_is_open(repository_id) {
                            return false;
                        }
                        if change.git_config_changed {
                            this.git_client.invalidate_remotes(&root_path);
                        }
                        if change.history_changed {
                            this.git_client.invalidate_branches(&root_path);
                            this.invalidate_history(repository_id);
                        }
                        if change.refresh_ignored_paths() || change.watcher_error {
                            this.repository_watchers.remove(&repository_id);
                        }
                        if change.watcher_error {
                            this.pending_refreshes.insert(repository_id);
                            if let Some(repository) = this
                                .state
                                .repositories
                                .iter_mut()
                                .find(|repository| repository.id == repository_id)
                            {
                                repository.status_message =
                                    "Watcher detenido; pulsa F5 para reconciliar".to_owned();
                                repository.error = Some(
                                    "La vigilancia falló; el estado visible se conserva y se intentará recuperar tras el próximo refresh."
                                        .to_owned(),
                                );
                            }
                        } else if change.refresh_ignored_paths() {
                            this.ensure_watcher(repository_id, cx);
                        }
                        if this.state.active_repository_id != Some(repository_id) {
                            this.pending_refreshes.insert(repository_id);
                            return true;
                        }
                        this.refresh_repository(repository_id, cx);
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        })
        .detach();
    }

    fn repository_session_is_open(&self, repository_id: RepositoryId) -> bool {
        self.state
            .repositories
            .iter()
            .any(|repository| repository.id == repository_id)
    }

    fn finish_refresh(
        &mut self,
        repository_id: RepositoryId,
        generation: u64,
        history_included: bool,
        result: Result<RepositorySnapshot, GitError>,
    ) -> RefreshOutcome {
        if self.global_refresh_in_flight == Some(repository_id) {
            self.global_refresh_in_flight = None;
        }
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return RefreshOutcome::default();
        };
        if repository.refresh_generation != generation {
            return RefreshOutcome::default();
        }
        self.active_refresh_cancellations.remove(&repository_id);
        let continuation = if repository.refresh_coordinator.finish() {
            RefreshContinuation::Repeat
        } else {
            RefreshContinuation::Complete
        };
        let history_invalidated_during_refresh = repository.history_invalidated_during_refresh;
        repository.history_invalidated_during_refresh = false;
        match result {
            Ok(mut snapshot) => {
                let history_changed = repository.snapshot.head != snapshot.head
                    || repository.snapshot.upstream != snapshot.upstream;
                if history_included {
                    repository.history_loaded = !history_invalidated_during_refresh;
                    repository.history_loading = false;
                } else if history_changed || history_invalidated_during_refresh {
                    repository.history_generation = repository.history_generation.saturating_add(1);
                    repository.history_loaded = false;
                    repository.history_loading = false;
                } else {
                    snapshot.commits.clone_from(&repository.snapshot.commits);
                    snapshot.has_more_commits = repository.snapshot.has_more_commits;
                    snapshot
                        .history_reference
                        .clone_from(&repository.snapshot.history_reference);
                    snapshot
                        .history_oid
                        .clone_from(&repository.snapshot.history_oid);
                }
                let snapshot_changed = *repository.snapshot != snapshot;
                if snapshot_changed {
                    repository.snapshot = Arc::new(snapshot);
                    self.change_rows.remove(&repository_id);
                }
                repository.refresh_state = RefreshState::Succeeded {
                    message: "Estado actualizado".to_owned(),
                };
                repository.status_message = "Estado actualizado".to_owned();
                repository.error = None;
                if history_changed {
                    self.selected_commit_details.remove(&repository_id);
                }
                RefreshOutcome {
                    succeeded: true,
                    // El fin de la operación también es una transición visible si
                    // Git devolvió exactamente el mismo snapshot.
                    should_notify: true,
                    reload_history: !repository.history_loaded
                        && repository.selected_view == RepositoryView::History,
                    continuation,
                }
            }
            Err(error) => {
                let details = error.technical_details();
                if history_included {
                    repository.history_loading = false;
                }
                let is_cancelled = is_cancelled_error(&error);
                repository.refresh_state = if is_cancelled {
                    RefreshState::Cancelled {
                        message: "Actualización cancelada".to_owned(),
                    }
                } else {
                    RefreshState::Failed {
                        message: error.to_string(),
                        details: details.clone(),
                    }
                };
                repository.status_message = if is_cancelled {
                    "Actualización cancelada".to_owned()
                } else {
                    "Error al actualizar".to_owned()
                };
                repository.error = (!is_cancelled).then_some(details);
                RefreshOutcome {
                    should_notify: true,
                    continuation,
                    ..RefreshOutcome::default()
                }
            }
        }
    }

    fn select_view(&mut self, view: RepositoryView, cx: &mut Context<Self>) {
        let Some(repository) = self.active_repository_mut() else {
            return;
        };
        let repository_id = repository.id;
        repository.selected_view = view;
        self.save_state(cx);
        if view == RepositoryView::History {
            self.ensure_history_loaded(repository_id, cx);
        }
        cx.notify();
    }

    fn invalidate_history(&mut self, repository_id: RepositoryId) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        repository.history_generation = repository.history_generation.saturating_add(1);
        repository.history_loaded = false;
        repository.history_loading = false;
        if repository.refresh_coordinator.in_flight() {
            repository.history_invalidated_during_refresh = true;
        }
        let snapshot = Arc::make_mut(&mut repository.snapshot);
        snapshot.commits.clear();
        snapshot.has_more_commits = false;
        snapshot.history_reference = None;
        snapshot.history_oid = None;
        self.selected_commit_details.remove(&repository_id);
    }

    fn ensure_history_loaded(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if repository.selected_view != RepositoryView::History
            || repository.history_loaded
            || repository.history_loading
        {
            return;
        }
        let Some((reference, expected_oid)) = history_target_for_head(&repository.snapshot.head)
        else {
            repository.history_loaded = true;
            return;
        };
        repository.history_generation = repository.history_generation.saturating_add(1);
        let generation = repository.history_generation;
        repository.history_loading = true;
        repository.status_message = "Cargando historial…".to_owned();
        repository.error = None;
        let snapshot = Arc::make_mut(&mut repository.snapshot);
        snapshot.history_reference = Some(reference.clone());
        snapshot.history_oid = Some(expected_oid.clone());
        let root_path = repository.root_path.clone();
        let git_client = self.git_client.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.history_for_oid(
                        &root_path,
                        &reference,
                        &expected_oid,
                        INITIAL_HISTORY_LIMIT + 1,
                        0,
                        &CancellationToken::default(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(repository) = this
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                else {
                    return;
                };
                if repository.history_generation != generation {
                    return;
                }
                repository.history_loading = false;
                match result {
                    Ok(page)
                        if history_target_is_current(repository, &page.reference, &page.oid) =>
                    {
                        let snapshot = Arc::make_mut(&mut repository.snapshot);
                        snapshot.has_more_commits = page.commits.len() > INITIAL_HISTORY_LIMIT;
                        snapshot.commits = page
                            .commits
                            .into_iter()
                            .take(INITIAL_HISTORY_LIMIT)
                            .map(|commit| commit.summary)
                            .collect();
                        snapshot.history_reference = Some(page.reference);
                        snapshot.history_oid = Some(page.oid);
                        repository.history_loaded = true;
                        repository.status_message = "Historial actualizado".to_owned();
                        repository.error = None;
                    }
                    Ok(_) => {
                        repository.history_loaded = false;
                        repository.status_message = "La selección de historial cambió".to_owned();
                    }
                    Err(error) => {
                        repository.status_message = "No se pudo cargar el historial".to_owned();
                        repository.error = Some(error.technical_details());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_history(&mut self, _: &ShowHistory, _: &mut Window, cx: &mut Context<Self>) {
        self.select_view(RepositoryView::History, cx);
    }

    fn show_changes(&mut self, _: &ShowChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.select_view(RepositoryView::Changes, cx);
    }

    fn stage_path(&mut self, repository_id: RepositoryId, path: PathBuf, cx: &mut Context<Self>) {
        self.run_mutation(
            repository_id,
            OperationKind::Stage,
            move |client, root, cancellation| client.stage(&root, &path, &cancellation),
            false,
            cx,
        );
    }

    fn unstage_path(&mut self, repository_id: RepositoryId, path: PathBuf, cx: &mut Context<Self>) {
        self.run_mutation(
            repository_id,
            OperationKind::Unstage,
            move |client, root, cancellation| client.unstage(&root, &path, &cancellation),
            false,
            cx,
        );
    }

    fn stage_all(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        self.run_mutation(
            repository_id,
            OperationKind::Stage,
            |client, root, cancellation| client.stage_all(&root, &cancellation),
            false,
            cx,
        );
    }

    fn unstage_all(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        self.run_mutation(
            repository_id,
            OperationKind::Unstage,
            |client, root, cancellation| client.unstage_all(&root, &cancellation),
            false,
            cx,
        );
    }

    fn confirm_discard(
        &mut self,
        repository_id: RepositoryId,
        change: &FileChange,
        discard_staged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        let has_head = !matches!(repository.snapshot.head, HeadState::Unborn);
        let is_directory = repository.root_path.join(&change.path).is_dir();
        let plan = match plan_discard(change, discard_staged, has_head, is_directory) {
            Ok(plan) => plan,
            Err(error) => {
                if let Some(repository) = self
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                {
                    repository.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        let details = format!(
            "Se descartarán los cambios de:\n{}\n\nEsta acción no se puede deshacer desde Git Helper.",
            plan.path.display()
        );
        let prompt = window.prompt(
            PromptLevel::Critical,
            "¿Descartar cambios?",
            Some(&details),
            &["Descartar", "Cancelar"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Ok(choice) = prompt.await else {
                return;
            };
            if choice == 0 {
                this.update(cx, |this, cx| {
                    this.execute_discard(repository_id, plan, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn execute_discard(
        &mut self,
        repository_id: RepositoryId,
        plan: DiscardPlan,
        cx: &mut Context<Self>,
    ) {
        self.run_mutation(
            repository_id,
            OperationKind::Discard,
            move |client, root, cancellation| client.discard(&root, &plan, &cancellation),
            false,
            cx,
        );
    }

    fn confirm_discard_all(
        &mut self,
        repository_id: RepositoryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        let has_head = !matches!(repository.snapshot.head, HeadState::Unborn);
        let mut planned_paths = HashSet::new();
        let mut plans = Vec::new();
        for change in &repository.snapshot.changes {
            if change.is_conflicted || !planned_paths.insert(change.path.clone()) {
                continue;
            }
            let discard_staged = change.has_staged_change();
            let is_directory = repository.root_path.join(&change.path).is_dir();
            if let Ok(plan) = plan_discard(change, discard_staged, has_head, is_directory) {
                plans.push(plan);
            }
        }
        if plans.is_empty() {
            if let Some(repository) = self
                .state
                .repositories
                .iter_mut()
                .find(|repository| repository.id == repository_id)
            {
                repository.error = Some(
                    "No hay cambios descartables. Los conflictos y cambios staged sin HEAD deben resolverse o quitarse del stage manualmente."
                        .to_owned(),
                );
            }
            cx.notify();
            return;
        }
        let details = plans
            .iter()
            .map(|plan| plan.path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let first_prompt = window.prompt(
            PromptLevel::Warning,
            "¿Descartar todos los cambios enumerados?",
            Some(&details),
            &["Continuar", "Cancelar"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if !matches!(first_prompt.await, Ok(0)) {
                return;
            }
            let second_prompt = cx.prompt(
                PromptLevel::Critical,
                "Confirmación final: esta acción no se puede deshacer",
                Some("Solo se afectarán las rutas enumeradas en el diálogo anterior."),
                &["Descartar definitivamente", "Cancelar"],
            );
            if !matches!(second_prompt.await, Ok(0)) {
                return;
            }
            this.update_in(cx, |this, _, cx| {
                this.execute_discard_all(repository_id, plans, cx);
            })
            .ok();
        })
        .detach();
    }

    fn execute_discard_all(
        &mut self,
        repository_id: RepositoryId,
        plans: Vec<DiscardPlan>,
        cx: &mut Context<Self>,
    ) {
        self.run_mutation(
            repository_id,
            OperationKind::Discard,
            move |client, root, cancellation| {
                let mut failures = Vec::new();
                for plan in plans {
                    if let Err(error) = client.discard(&root, &plan, &cancellation) {
                        failures.push(format!("{}: {error}", plan.path.display()));
                    }
                }
                if failures.is_empty() {
                    Ok(())
                } else {
                    Err(GitError::InvalidStatus {
                        message: format!(
                            "Resultado parcial del descarte:\n{}",
                            failures.join("\n")
                        ),
                    })
                }
            },
            false,
            cx,
        );
    }

    fn create_commit(&mut self, _: &CreateCommit, _: &mut Window, cx: &mut Context<Self>) {
        let Some(repository_id) = self.state.active_repository_id else {
            return;
        };
        let Some(input) = self.commit_inputs.get(&repository_id) else {
            return;
        };
        let message = input.read(cx).content().to_owned();
        self.run_mutation(
            repository_id,
            OperationKind::Commit,
            move |client, root, cancellation| client.commit(&root, &message, &cancellation),
            true,
            cx,
        );
    }

    fn generate_commit_message(
        &mut self,
        _: &GenerateCommitMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_commit_message_generation(window, cx);
    }

    fn start_commit_message_generation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repository) = self.active_repository() else {
            return;
        };
        let repository_id = repository.id;
        let root_path = repository.root_path.clone();
        if !repository
            .snapshot
            .changes
            .iter()
            .any(FileChange::has_staged_change)
        {
            if let Some(repository) = self
                .state
                .repositories
                .iter_mut()
                .find(|repository| repository.id == repository_id)
            {
                repository.error = Some("No hay cambios staged para generar un mensaje".to_owned());
            }
            cx.notify();
            return;
        }
        if !repository.can_mutate() {
            let message = if repository.is_refreshing() {
                "Espera a que termine la actualización del repositorio"
            } else {
                "Ya hay otra mutación activa en este repositorio"
            };
            if let Some(repository) = self
                .state
                .repositories
                .iter_mut()
                .find(|repository| repository.id == repository_id)
            {
                repository.error = Some(message.to_owned());
            }
            cx.notify();
            return;
        }
        let draft_version = self
            .commit_inputs
            .get(&repository_id)
            .map(|input| input.read(cx).content_version())
            .unwrap_or_default();
        self.next_generation_request_id = self.next_generation_request_id.saturating_add(1);
        let request = CommitMessageRequest {
            session_id: repository_id,
            request_id: self.next_generation_request_id,
            draft_version,
        };
        self.generation_requests
            .insert(repository_id, request.clone());
        if let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        {
            repository.mutation_state = MutationState::Running {
                kind: OperationKind::GenerateCommitMessage,
                generation: repository.refresh_generation,
            };
            repository.status_message = "Preparando contexto staged…".to_owned();
            repository.error = None;
        }
        let git_client = self.git_client.clone();
        let cursor_client = self.cursor_client.clone();
        let cancellation = CancellationToken::default();
        self.active_mutation_cancellations
            .insert(repository_id, cancellation.clone());
        if let Some(input) = self.commit_inputs.get(&repository_id) {
            input.update(cx, |input, cx| input.set_generating(true, cx));
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let context_root = root_path.clone();
            let context_cancellation = cancellation.clone();
            let context_git_client = git_client.clone();
            let context_result = cx
                .background_spawn({
                    let context_cancellation = context_cancellation.clone();
                    async move {
                        let data = context_git_client
                            .staged_context(&context_root, &context_cancellation)
                            .map_err(|error| match error {
                                GitError::StagedStateChanged => {
                                    GenerationPreparationError::StagedChanged
                                }
                                error => GenerationPreparationError::Message(error.to_string()),
                            })?;
                        build_cursor_context(&data)
                            .map_err(|error| GenerationPreparationError::Message(error.to_string()))
                            .map(|context| (context, data.index_identity))
                    }
                })
                .await;
            let (context, expected_index_identity) = match context_result {
                Ok(context) => context,
                Err(GenerationPreparationError::StagedChanged) => {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(
                            GenerationCompletion {
                                request: request.clone(),
                                expected_index_identity: None,
                                result: Err(
                                    "El staging area cambió mientras se preparaba el contexto"
                                        .to_owned(),
                                ),
                                current_index_identity: None,
                                was_cancelled: false,
                                staged_changed: true,
                            },
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
                Err(GenerationPreparationError::Message(error)) => {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(
                            GenerationCompletion {
                                request: request.clone(),
                                expected_index_identity: None,
                                result: Err(error),
                                current_index_identity: None,
                                was_cancelled: context_cancellation.is_cancelled(),
                                staged_changed: false,
                            },
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            };
            let identity_before_send = cx
                .background_spawn({
                    let git_client = git_client.clone();
                    let root_path = root_path.clone();
                    let cancellation = context_cancellation.clone();
                    async move { git_client.staged_identity(&root_path, &cancellation) }
                })
                .await;
            match identity_before_send {
                Ok(identity) if identity == expected_index_identity => {}
                Ok(_) => {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(
                            GenerationCompletion {
                                request: request.clone(),
                                expected_index_identity: Some(expected_index_identity.clone()),
                                result: Err(
                                    "La propuesta quedó obsoleta porque cambió el staging area"
                                        .to_owned(),
                                ),
                                current_index_identity: None,
                                was_cancelled: false,
                                staged_changed: true,
                            },
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(
                            GenerationCompletion {
                                request: request.clone(),
                                expected_index_identity: Some(expected_index_identity.clone()),
                                result: Err(error.to_string()),
                                current_index_identity: None,
                                was_cancelled: context_cancellation.is_cancelled(),
                                staged_changed: false,
                            },
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            }
            this.update_in(cx, |this, _, _| {
                if let Some(repository) = this
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                {
                    repository.status_message =
                        "Generando mensaje con Cursor (máximo 60 s)…".to_owned();
                }
            })
            .ok();
            if !context.sensitive_patterns.is_empty() {
                let details = format!(
                    "La detección básica encontró {} tipos de patrón sensible. Esta detección no es completa. Revisa los cambios antes de enviarlos.",
                    context.sensitive_patterns.len()
                );
                let prompt = cx.prompt(
                    PromptLevel::Critical,
                    "Posibles secretos en los cambios staged",
                    Some(&details),
                    &["Enviar de todos modos", "Cancelar"],
                );
                if !matches!(prompt.await, Ok(0)) {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(
                            GenerationCompletion {
                                request: request.clone(),
                                expected_index_identity: Some(expected_index_identity.clone()),
                                result: Err(
                                    "Generación cancelada antes de enviar el contexto".to_owned(),
                                ),
                                current_index_identity: None,
                                was_cancelled: true,
                                staged_changed: false,
                            },
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            }
            let cursor_root = root_path.clone();
            let cursor_cancellation = cancellation.clone();
            let result = cx
                .background_spawn(async move {
                    cursor_client
                        .generate_commit_message(
                            &cursor_root,
                            context.prompt,
                            &cursor_cancellation,
                        )
                        .map_err(|error| error.to_string())
                })
                .await;
            let was_cancelled = context_cancellation.is_cancelled();
            let current_index_identity = if result.is_ok() && !was_cancelled {
                Some(
                    cx.background_spawn({
                        let git_client = git_client.clone();
                        let root_path = root_path.clone();
                        let cancellation = context_cancellation.clone();
                        async move {
                            git_client
                                .staged_identity(&root_path, &cancellation)
                                .map_err(|error| error.to_string())
                        }
                    })
                    .await,
                )
            } else {
                None
            };
            this.update_in(cx, |this, _, cx| {
                this.finish_message_generation(
                    GenerationCompletion {
                        request,
                        expected_index_identity: Some(expected_index_identity),
                        result,
                        current_index_identity,
                        was_cancelled,
                        staged_changed: false,
                    },
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    fn finish_message_generation(
        &mut self,
        mut completion: GenerationCompletion,
        cx: &mut Context<Self>,
    ) {
        let repository_id = completion.request.session_id;
        if self.generation_requests.get(&repository_id) != Some(&completion.request) {
            return;
        }
        self.active_mutation_cancellations.remove(&repository_id);
        let input = self.commit_inputs.get(&repository_id).cloned();
        if let Some(input) = &input {
            input.update(cx, |input, cx| input.set_generating(false, cx));
        }
        let mut status_message = String::new();
        let mut error_message = None;
        let mut mutation_state = MutationState::Cancelled {
            kind: OperationKind::GenerateCommitMessage,
            message: "Generación cancelada".to_owned(),
        };
        match completion.result {
            Ok(message) => {
                let decision = match (
                    completion.expected_index_identity.as_deref(),
                    completion.current_index_identity.as_ref(),
                    input.as_ref(),
                ) {
                    (Some(expected), Some(Ok(current)), Some(input)) => validate_generation_result(
                        &completion.request,
                        self.generation_requests.get(&repository_id),
                        input.read(cx).content_version(),
                        expected,
                        current,
                    ),
                    (_, Some(Err(error)), _) => {
                        error_message = Some(error.clone());
                        GenerationApplyDecision::IndexChanged
                    }
                    _ => GenerationApplyDecision::IndexChanged,
                };
                match decision {
                    GenerationApplyDecision::Apply => {
                        if let Some(input) = input {
                            input.update(cx, |input, cx| input.set_content(message, cx));
                        }
                        status_message = "Mensaje generado; revísalo antes del commit".to_owned();
                        mutation_state = MutationState::Succeeded {
                            kind: OperationKind::GenerateCommitMessage,
                            message: "Mensaje generado".to_owned(),
                        };
                    }
                    GenerationApplyDecision::DraftChanged => {
                        status_message = "Propuesta no aplicada; el borrador cambió".to_owned();
                        error_message = Some(
                            "Se conserva tu borrador actual. Pulsa «Generar con Cursor» para reintentar."
                                .to_owned(),
                        );
                        mutation_state = MutationState::Cancelled {
                            kind: OperationKind::GenerateCommitMessage,
                            message: "El borrador cambió durante la generación".to_owned(),
                        };
                    }
                    GenerationApplyDecision::IndexChanged => completion.staged_changed = true,
                    GenerationApplyDecision::StaleRequest => return,
                }
            }
            Err(error) => {
                let cancelled = completion.was_cancelled
                    || completion.staged_changed
                    || error.to_lowercase().contains("cancel");
                status_message = if cancelled {
                    "Generación cancelada; se conserva el borrador".to_owned()
                } else {
                    "No se pudo generar el mensaje".to_owned()
                };
                error_message = Some(if completion.staged_changed {
                    "El staging area cambió; la propuesta quedó obsoleta. Revisa los cambios y pulsa «Generar con Cursor» para reintentar."
                        .to_owned()
                } else {
                    error
                });
                mutation_state = if cancelled {
                    MutationState::Cancelled {
                        kind: OperationKind::GenerateCommitMessage,
                        message: "Generación cancelada".to_owned(),
                    }
                } else {
                    MutationState::Failed {
                        kind: OperationKind::GenerateCommitMessage,
                        message: "No se pudo generar el mensaje".to_owned(),
                        details: error_message.clone().unwrap_or_default(),
                    }
                };
            }
        }
        self.generation_requests.remove(&repository_id);
        if completion.staged_changed {
            status_message = "Propuesta obsoleta; revisa el staging area".to_owned();
            error_message = Some(
                "El staging area cambió durante la generación. La propuesta no se aplicó; pulsa «Generar con Cursor» para reintentar."
                    .to_owned(),
            );
            mutation_state = MutationState::Cancelled {
                kind: OperationKind::GenerateCommitMessage,
                message: "El staging area cambió durante la generación".to_owned(),
            };
        }
        if let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        {
            repository.status_message = status_message;
            repository.error = error_message;
            repository.mutation_state = mutation_state;
        }
        self.refresh_repository(repository_id, cx);
        cx.notify();
    }

    fn fetch(&mut self, repository_id: RepositoryId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if !repository.can_mutate() {
            return;
        }
        let plan = plan_fetch(
            repository.snapshot.upstream.as_ref(),
            &repository.snapshot.remotes,
            None,
        );
        match plan {
            Err(GitError::RemoteSelectionRequired { remotes }) => {
                self.prompt_for_remote(repository_id, OperationKind::Fetch, remotes, window, cx);
            }
            plan => self.run_remote_plan(repository_id, OperationKind::Fetch, plan, cx),
        }
    }

    fn pull(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if !repository.can_mutate() {
            return;
        }
        let plan = plan_pull(repository.snapshot.upstream.as_ref());
        self.run_remote_plan(repository_id, OperationKind::Pull, plan, cx);
    }

    fn push(&mut self, repository_id: RepositoryId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if !repository.can_mutate() {
            return;
        }
        let plan = plan_push(
            &repository.snapshot.head,
            repository.snapshot.upstream.as_ref(),
            &repository.snapshot.remotes,
            None,
        );
        match plan {
            Err(GitError::RemoteSelectionRequired { remotes }) => {
                self.prompt_for_remote(repository_id, OperationKind::Push, remotes, window, cx);
            }
            Ok(plan) => self.confirm_first_push(repository_id, plan, window, cx),
            Err(error) => {
                if let Some(repository) = self
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                {
                    repository.error = Some(error.to_string());
                }
                cx.notify();
            }
        }
    }

    fn prompt_for_remote(
        &mut self,
        repository_id: RepositoryId,
        kind: OperationKind,
        remotes: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut buttons: Vec<PromptButton> =
            remotes.iter().cloned().map(PromptButton::new).collect();
        buttons.push(PromptButton::cancel("Cancelar"));
        let prompt = window.prompt(
            PromptLevel::Info,
            "Selecciona un remote",
            None,
            &buttons,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(selection) = prompt.await else {
                return;
            };
            let Some(remote_name) = remotes.get(selection).cloned() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let Some(repository) = this
                    .state
                    .repositories
                    .iter()
                    .find(|repository| repository.id == repository_id)
                else {
                    return;
                };
                let plan = match kind {
                    OperationKind::Fetch => plan_fetch(
                        repository.snapshot.upstream.as_ref(),
                        &repository.snapshot.remotes,
                        Some(&remote_name),
                    ),
                    OperationKind::Push => plan_push(
                        &repository.snapshot.head,
                        repository.snapshot.upstream.as_ref(),
                        &repository.snapshot.remotes,
                        Some(&remote_name),
                    ),
                    _ => return,
                };
                if kind == OperationKind::Fetch {
                    this.run_remote_plan(repository_id, kind, plan, cx);
                } else {
                    match plan {
                        Ok(plan) => {
                            this.confirm_first_push(repository_id, plan, window, cx);
                        }
                        Err(error) => {
                            if let Some(repository) = this
                                .state
                                .repositories
                                .iter_mut()
                                .find(|repository| repository.id == repository_id)
                            {
                                repository.error = Some(error.to_string());
                            }
                            cx.notify();
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn confirm_first_push(
        &mut self,
        repository_id: RepositoryId,
        plan: crate::domain::RemoteOperationPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let crate::domain::RemoteOperationPlan::SetUpstreamAndPush {
            remote_name,
            branch_name,
        } = &plan
        else {
            self.run_remote_plan(repository_id, OperationKind::Push, Ok(plan), cx);
            return;
        };
        let details = format!(
            "Se publicará la rama {branch_name} y se configurará {remote_name}/{branch_name} como upstream."
        );
        let prompt = window.prompt(
            PromptLevel::Warning,
            "¿Publicar esta rama por primera vez?",
            Some(&details),
            &["Publicar", "Cancelar"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if matches!(prompt.await, Ok(0)) {
                this.update(cx, |this, cx| {
                    this.run_remote_plan(repository_id, OperationKind::Push, Ok(plan), cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn run_remote_plan(
        &mut self,
        repository_id: RepositoryId,
        kind: OperationKind,
        plan: Result<crate::domain::RemoteOperationPlan, GitError>,
        cx: &mut Context<Self>,
    ) {
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                if let Some(repository) = self
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                {
                    repository.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        self.run_mutation(
            repository_id,
            kind,
            move |client, root, cancellation| client.execute_remote(&root, &plan, &cancellation),
            false,
            cx,
        );
    }

    fn cancel_active_operation(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let mut cancelled = false;
        if let Some(cancellation) = self.active_refresh_cancellations.get(&repository_id) {
            cancellation.cancel();
            cancelled = true;
        }
        if let Some(cancellation) = self.active_mutation_cancellations.get(&repository_id) {
            cancellation.cancel();
            cancelled = true;
        }
        if cancelled {
            if let Some(repository) = self
                .state
                .repositories
                .iter_mut()
                .find(|repository| repository.id == repository_id)
            {
                repository.status_message = "Cancelando operación…".to_owned();
            }
            cx.notify();
        }
    }

    fn run_mutation<F>(
        &mut self,
        repository_id: RepositoryId,
        kind: OperationKind,
        work: F,
        clear_commit_message: bool,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce(GitClient, PathBuf, CancellationToken) -> Result<(), GitError> + Send + 'static,
    {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if !repository.can_mutate() {
            repository.error = Some(if repository.is_refreshing() {
                "Espera a que termine la actualización del repositorio".to_owned()
            } else {
                "Ya hay otra mutación activa en este repositorio".to_owned()
            });
            cx.notify();
            return;
        }
        let root_path = repository.root_path.clone();
        repository.mutation_state = MutationState::Running {
            kind,
            generation: repository.refresh_generation,
        };
        repository.status_message = operation_running_message(kind).to_owned();
        repository.error = None;
        let git_client = self.git_client.clone();
        let cancellation = CancellationToken::default();
        self.active_mutation_cancellations
            .insert(repository_id, cancellation.clone());
        // El usuario debe ver de inmediato que la mutación ha comenzado.
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { work(git_client, root_path, cancellation) })
                .await;
            this.update(cx, |this, cx| {
                this.active_mutation_cancellations.remove(&repository_id);
                let mut refresh_immediately = false;
                match result {
                    Ok(()) => {
                        if clear_commit_message
                            && let Some(input) = this.commit_inputs.get(&repository_id)
                        {
                            input.update(cx, CommitInput::clear);
                        }
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.mutation_state = MutationState::Succeeded {
                                kind,
                                message: operation_success_message(kind).to_owned(),
                            };
                            repository.status_message = operation_success_message(kind).to_owned();
                            repository.error = None;
                        }
                        if matches!(
                            kind,
                            OperationKind::Commit
                                | OperationKind::Fetch
                                | OperationKind::Pull
                                | OperationKind::Push
                        ) {
                            this.invalidate_history(repository_id);
                        }
                        let feedback_timer =
                            cx.background_executor().timer(Duration::from_millis(350));
                        cx.spawn(async move |this, cx| {
                            feedback_timer.await;
                            this.update(cx, |this, cx| {
                                this.refresh_repository(repository_id, cx);
                            })
                            .ok();
                        })
                        .detach();
                    }
                    Err(error) => {
                        refresh_immediately = true;
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            let is_cancelled = is_cancelled_error(&error);
                            repository.mutation_state = if is_cancelled {
                                MutationState::Cancelled {
                                    kind,
                                    message: "Operación cancelada".to_owned(),
                                }
                            } else {
                                MutationState::Failed {
                                    kind,
                                    message: error.to_string(),
                                    details: error.technical_details(),
                                }
                            };
                            repository.status_message = if is_cancelled {
                                "Operación cancelada".to_owned()
                            } else {
                                "La operación falló".to_owned()
                            };
                            repository.error = (!is_cancelled).then(|| error.technical_details());
                        }
                    }
                }
                // Reconciliar siempre: el estado real puede haber cambiado
                // antes, durante o después de una operación fallida/cancelada.
                if refresh_immediately {
                    this.refresh_repository(repository_id, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        self.state.active_repository_id = Some(repository_id);
        if self.pending_refreshes.contains(&repository_id) {
            self.force_refresh_repository(repository_id, cx);
        }
        self.save_state(cx);
        cx.notify();
    }

    fn render_repository_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let active_id = self.state.active_repository_id;
        div()
            .flex()
            .items_center()
            .h(px(38.0))
            .border_b_1()
            .border_color(BORDER_COLOR)
            .bg(ELEVATED_BACKGROUND_COLOR)
            .children(self.state.repositories.iter().map(|repository| {
                let repository_id = repository.id;
                let is_active = active_id == Some(repository_id);
                let name = repository
                    .root_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Repositorio")
                    .to_owned();
                let full_path = repository.root_path.display().to_string();
                let change_count = repository.snapshot.changes.len();
                div()
                    .id(format!("repository-tab-{repository_id:?}"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h_full()
                    .px_3()
                    .border_r_1()
                    .border_color(BORDER_COLOR)
                    .aria_label(full_path)
                    .when(is_active, |tab| tab.bg(SELECTED_BACKGROUND_COLOR))
                    .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_repository(repository_id, cx);
                    }))
                    .child(name)
                    .when(change_count > 0, |tab| {
                        tab.child(
                            div()
                                .text_xs()
                                .text_color(WARNING_COLOR)
                                .child(change_count.to_string()),
                        )
                    })
                    .when(
                        matches!(repository.refresh_state, RefreshState::Running { .. }),
                        |tab| tab.child(div().text_xs().text_color(ACCENT_COLOR).child("⟳")),
                    )
                    .when(!repository.path_accessible, |tab| {
                        tab.child(div().text_xs().text_color(WARNING_COLOR).child("⛔"))
                    })
                    .when(repository.error.is_some(), |tab| {
                        tab.child(div().text_xs().text_color(ERROR_COLOR).child("!"))
                    })
                    .child(
                        div()
                            .id(format!("close-repository-tab-{repository_id:?}"))
                            .px_1()
                            .text_color(MUTED_TEXT_COLOR)
                            .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_repository(repository_id, cx);
                            }))
                            .child("×"),
                    )
            }))
            .child(
                div()
                    .id("open-repository-tab")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(38.0))
                    .h_full()
                    .text_lg()
                    .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_repository(&OpenRepository, window, cx);
                    }))
                    .child("+"),
            )
            .into_any_element()
    }

    fn render_toolbar(&self, repository: &RepositorySession, cx: &mut Context<Self>) -> AnyElement {
        let repository_id = repository.id;
        let branch = match &repository.snapshot.head {
            HeadState::Branch { name, .. } => name.clone(),
            HeadState::Detached { .. } => "HEAD separado".to_owned(),
            HeadState::Unborn => "Sin commit inicial".to_owned(),
        };
        let upstream = repository.snapshot.upstream.as_ref().map(|upstream| {
            format!(
                "{}  ↑{} ↓{}",
                upstream.full_name, upstream.ahead, upstream.behind
            )
        });
        let can_mutate = repository.can_mutate();
        let is_refreshing = repository.is_refreshing();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .min_h(px(42.0))
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(BORDER_COLOR)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .text_sm()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(branch),
                    )
                    .when_some(upstream, |row, upstream| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(MUTED_TEXT_COLOR)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(upstream),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        action_button("fetch", "Fetch", can_mutate).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.fetch(repository_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        action_button("pull", "Pull", can_mutate).on_click(
                            cx.listener(move |this, _, _, cx| this.pull(repository_id, cx)),
                        ),
                    )
                    .child(
                        action_button("push", "Push", can_mutate).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.push(repository_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        action_button("refresh", "Actualizar", can_mutate && !is_refreshing)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.force_refresh_repository(repository_id, cx);
                            })),
                    )
                    .when(!can_mutate, |row| {
                        row.child(
                            action_button("cancel", "Cancelar", true).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.cancel_active_operation(repository_id, cx);
                                },
                            )),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_internal_tabs(
        &self,
        repository: &RepositorySession,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = repository.snapshot.changes.len();
        div()
            .flex()
            .h(px(36.0))
            .border_b_1()
            .border_color(BORDER_COLOR)
            .child(
                tab_button(
                    "changes-tab",
                    format!("Cambios ({count})"),
                    repository.selected_view == RepositoryView::Changes,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.select_view(RepositoryView::Changes, cx);
                })),
            )
            .child(
                tab_button(
                    "history-tab",
                    "Historial",
                    repository.selected_view == RepositoryView::History,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.select_view(RepositoryView::History, cx);
                })),
            )
            .into_any_element()
    }

    fn render_changes(
        &mut self,
        repository: &RepositorySession,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.change_rows.contains_key(&repository.id) {
            let rows = Arc::new(build_change_rows(
                repository.id,
                &repository.snapshot.changes,
                &self.collapsed_groups,
            ));
            self.change_rows.insert(repository.id, rows);
        }
        let rows = self
            .change_rows
            .get(&repository.id)
            .cloned()
            .unwrap_or_default();
        let row_count = rows.len();
        let repository_id = repository.id;
        let input = self.commit_inputs.get(&repository_id).cloned();
        let staged_count = repository
            .snapshot
            .changes
            .iter()
            .filter(|change| change.has_staged_change())
            .count();
        let can_mutate = repository.can_mutate();
        let message_is_empty = input
            .as_ref()
            .is_none_or(|input| input.read(cx).content().trim().is_empty());
        let commit_enabled = staged_count > 0 && !message_is_empty && can_mutate;

        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .child(
                uniform_list(
                    "change-list",
                    row_count,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        rows[range]
                            .iter()
                            .cloned()
                            .map(|row| this.render_change_row(repository_id, row, window, cx))
                            .collect()
                    }),
                )
                .w_full()
                .flex_1(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(BORDER_COLOR)
                    .when_some(input, gpui::ParentElement::child)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(MUTED_TEXT_COLOR)
                                    .flex_shrink_0()
                                    .child(format!("{staged_count} archivos staged")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .justify_end()
                                    .flex_shrink_0()
                                    .child(
                                        action_button(
                                            "discard-all",
                                            "Descartar todo",
                                            !repository.snapshot.changes.is_empty() && can_mutate,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.confirm_discard_all(repository_id, window, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            "generate-message",
                                            "Generar con Cursor",
                                            staged_count > 0 && can_mutate,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.generate_commit_message(
                                                    &GenerateCommitMessage,
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    )
                                    .child(action_button("commit", "Commit", commit_enabled).when(
                                        commit_enabled,
                                        |button| {
                                            button.on_click(cx.listener(|this, _, window, cx| {
                                                this.create_commit(&CreateCommit, window, cx);
                                            }))
                                        },
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn toggle_change_group(
        &mut self,
        repository_id: RepositoryId,
        representation: ChangeRepresentation,
        cx: &mut Context<Self>,
    ) {
        if !self
            .collapsed_groups
            .insert((repository_id, representation))
        {
            self.collapsed_groups
                .remove(&(repository_id, representation));
        }
        self.change_rows.remove(&repository_id);
        cx.notify();
    }

    fn render_change_row(
        &mut self,
        repository_id: RepositoryId,
        row: ChangeListRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_height = px(f32::from(row.height_px()));
        let can_mutate = self
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .is_some_and(RepositorySession::can_mutate);
        match row {
            ChangeListRow::Group {
                title,
                count,
                action,
                representation,
                is_collapsed,
            } => div()
                .id(format!("change-group-{repository_id:?}-{title}"))
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                .h(row_height)
                .px_3()
                .bg(ELEVATED_BACKGROUND_COLOR)
                .text_xs()
                .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_change_group(repository_id, representation, cx);
                }))
                .child(format!(
                    "{} {title} ({count})",
                    if is_collapsed { "▸" } else { "▾" }
                ))
                .when_some(action, |row, action| {
                    let label = match action {
                        GroupAction::StageAll => "Stage todo",
                        GroupAction::UnstageAll => "Unstage todo",
                    };
                    row.child(
                        action_button(format!("group-action-{title}"), label, can_mutate).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                match action {
                                    GroupAction::StageAll => this.stage_all(repository_id, cx),
                                    GroupAction::UnstageAll => this.unstage_all(repository_id, cx),
                                }
                            }),
                        ),
                    )
                })
                .into_any_element(),
            ChangeListRow::File {
                change,
                representation,
            } => {
                let path = change.path.clone();
                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();
                let parent = path
                    .parent()
                    .map(|parent| parent.display().to_string())
                    .unwrap_or_default();
                let discard_change = change.clone();
                let can_discard = !matches!(representation, ChangeRepresentation::Conflict);
                let is_staged = matches!(representation, ChangeRepresentation::Staged);
                let action_path = path.clone();
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .gap_x_2()
                    .h(row_height)
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(BORDER_COLOR)
                    .hover(|style| style.bg(HOVER_BACKGROUND_COLOR))
                    .child(
                        div()
                            .w(px(18.0))
                            .flex_shrink_0()
                            .text_color(status_color(representation))
                            .child(status_code(&change, representation)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_sm()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(file_name),
                            )
                            .when(!parent.is_empty(), |column| {
                                column.child(
                                    div()
                                        .text_xs()
                                        .text_color(MUTED_TEXT_COLOR)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(parent),
                                )
                            }),
                    )
                    .when(can_discard, |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .ml_auto()
                                .flex_shrink_0()
                                .child(
                                    action_button(
                                        change_row_action_id("discard", &path, representation),
                                        "Descartar",
                                        can_mutate,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.confirm_discard(
                                                repository_id,
                                                &discard_change,
                                                is_staged,
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                                .child(
                                    action_button(
                                        change_row_action_id("stage-toggle", &path, representation),
                                        if is_staged { "Unstage" } else { "Stage" },
                                        can_mutate,
                                    )
                                    .flex_shrink_0()
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            if is_staged {
                                                this.unstage_path(
                                                    repository_id,
                                                    action_path.clone(),
                                                    cx,
                                                );
                                            } else {
                                                this.stage_path(
                                                    repository_id,
                                                    action_path.clone(),
                                                    cx,
                                                );
                                            }
                                        },
                                    )),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    fn select_branch(
        &mut self,
        repository_id: RepositoryId,
        branch: BranchReference,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        let Some(current_branch) = repository
            .snapshot
            .branches
            .iter()
            .find(|current| current.full_name == branch.full_name)
        else {
            repository.error = Some("La rama ya no existe; actualiza el repositorio.".to_owned());
            cx.notify();
            return;
        };
        if current_branch.oid != branch.oid {
            repository.error = Some("La rama cambió; actualiza el repositorio.".to_owned());
            cx.notify();
            return;
        }
        repository.selected_view = RepositoryView::History;
        repository.history_generation = repository.history_generation.saturating_add(1);
        let generation = repository.history_generation;
        repository.history_loaded = branch.oid.is_none();
        repository.history_loading = branch.oid.is_some();
        repository.selected_commit = None;
        let snapshot = Arc::make_mut(&mut repository.snapshot);
        snapshot.commits.clear();
        snapshot.has_more_commits = false;
        snapshot.history_reference = Some(branch.full_name.clone());
        snapshot.history_oid.clone_from(&branch.oid);
        self.selected_commit_details.remove(&repository_id);
        let Some(expected_oid) = branch.oid else {
            repository.status_message = "La rama no tiene commits".to_owned();
            repository.error = None;
            cx.notify();
            return;
        };
        let root_path = repository.root_path.clone();
        let reference = branch.full_name;
        let branch_name = branch.name;
        let git_client = self.git_client.clone();
        repository.status_message = format!("Cargando historial de {branch_name}…");
        repository.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.history_for_ref(
                        &root_path,
                        &reference,
                        INITIAL_HISTORY_LIMIT + 1,
                        0,
                        &CancellationToken::default(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(repository) = this
                    .state
                    .repositories
                    .iter_mut()
                    .find(|repository| repository.id == repository_id)
                else {
                    return;
                };
                if repository.history_generation != generation {
                    return;
                }
                repository.history_loading = false;
                match result {
                    Ok(page)
                        if page.oid == expected_oid
                            && history_target_is_current(
                                repository,
                                &page.reference,
                                &page.oid,
                            ) =>
                    {
                        let snapshot = Arc::make_mut(&mut repository.snapshot);
                        snapshot.has_more_commits = page.commits.len() > INITIAL_HISTORY_LIMIT;
                        snapshot.commits = page
                            .commits
                            .into_iter()
                            .take(INITIAL_HISTORY_LIMIT)
                            .map(|commit| commit.summary)
                            .collect();
                        snapshot.history_reference = Some(page.reference);
                        snapshot.history_oid = Some(page.oid);
                        repository.history_loaded = true;
                        repository.status_message = "Historial actualizado".to_owned();
                        repository.error = None;
                    }
                    Ok(_) => {
                        repository.history_loaded = false;
                        repository.status_message = "La rama cambió durante la carga".to_owned();
                        repository.error = Some(
                            "Se descartó el resultado obsoleto; vuelve a seleccionar la rama."
                                .to_owned(),
                        );
                    }
                    Err(error) => {
                        repository.history_loaded = false;
                        repository.status_message = "No se pudo cargar la rama".to_owned();
                        repository.error = Some(error.technical_details());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_commit(
        &mut self,
        repository_id: RepositoryId,
        commit_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        repository.selected_commit = Some(commit_id.clone());
        let root_path = repository.root_path.clone();
        let git_client = self.git_client.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.commit_details(&root_path, &commit_id, &CancellationToken::default())
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(details) => {
                        this.selected_commit_details.insert(repository_id, details);
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_more_history(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if repository.history_loading {
            return;
        }
        let (Some(reference), Some(oid)) = (
            repository.snapshot.history_reference.clone(),
            repository.snapshot.history_oid.clone(),
        ) else {
            repository.error = Some("No hay una referencia de historial seleccionada".to_owned());
            cx.notify();
            return;
        };
        repository.history_loading = true;
        let generation = repository.history_generation;
        let root_path = repository.root_path.clone();
        let offset = repository.snapshot.commits.len();
        let git_client = self.git_client.clone();
        repository.status_message = "Cargando más commits…".to_owned();
        repository.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.history_for_oid(
                        &root_path,
                        &reference,
                        &oid,
                        INITIAL_HISTORY_LIMIT + 1,
                        offset,
                        &CancellationToken::default(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(page) => {
                        let has_more = page.commits.len() > INITIAL_HISTORY_LIMIT;
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            if repository.history_generation != generation
                                || !history_target_is_current(
                                    repository,
                                    &page.reference,
                                    &page.oid,
                                )
                            {
                                return;
                            }
                            Arc::make_mut(&mut repository.snapshot).commits.extend(
                                page.commits
                                    .into_iter()
                                    .take(INITIAL_HISTORY_LIMIT)
                                    .map(|commit| commit.summary),
                            );
                            Arc::make_mut(&mut repository.snapshot).has_more_commits = has_more;
                            repository.history_loaded = true;
                            repository.history_loading = false;
                            repository.status_message = "Historial actualizado".to_owned();
                            repository.error = None;
                        }
                    }
                    Err(error) => {
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                            && repository.history_generation == generation
                        {
                            repository.history_loading = false;
                            repository.status_message = "No se pudo cargar el historial".to_owned();
                            repository.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_history(&self, repository: &RepositorySession, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = Arc::clone(&repository.snapshot);
        let count = snapshot.commits.len();
        let repository_id = repository.id;
        let selected_commit = repository.selected_commit.clone();
        let has_more = repository.snapshot.has_more_commits;
        let selected_reference = repository.snapshot.history_reference.clone();
        let branches = repository.snapshot.branches.clone();
        let details = self.selected_commit_details.get(&repository_id).cloned();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .child(
                div()
                    .id("branch-inventory")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(150.0))
                    .overflow_y_scroll()
                    .p_2()
                    .border_b_1()
                    .border_color(BORDER_COLOR)
                    .child(
                        div()
                            .text_xs()
                            .text_color(MUTED_TEXT_COLOR)
                            .child("Ramas locales y referencias remotas"),
                    )
                    .children(branches.into_iter().map(|branch| {
                        let is_selected = selected_reference.as_ref() == Some(&branch.full_name);
                        let branch_for_click = branch.clone();
                        div()
                            .id(format!("branch-{}", branch.full_name))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .when(is_selected, |row| row.bg(SELECTED_BACKGROUND_COLOR))
                            .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_branch(repository_id, branch_for_click.clone(), cx);
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if branch.is_active {
                                        SUCCESS_COLOR
                                    } else {
                                        MUTED_TEXT_COLOR
                                    })
                                    .child(branch_kind_label(branch.kind)),
                            )
                            .child(div().text_sm().child(branch.name.clone()))
                            .child(
                                div()
                                    .ml_auto()
                                    .text_xs()
                                    .text_color(MUTED_TEXT_COLOR)
                                    .child(branch_upstream_label(&branch.upstream)),
                            )
                    })),
            )
            .child(
                uniform_list(
                    "history-list",
                    count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                        snapshot.commits[range]
                            .iter()
                            .map(|commit| {
                                let commit_id = commit.id.clone();
                                let is_selected = selected_commit.as_ref() == Some(&commit.id);
                                div()
                                    .id(format!("commit-{}", commit.id))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .h(px(44.0))
                                    .px_3()
                                    .border_b_1()
                                    .border_color(BORDER_COLOR)
                                    .when(is_selected, |row| row.bg(SELECTED_BACKGROUND_COLOR))
                                    .hover(|style| {
                                        style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_commit(repository_id, commit_id.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .w(px(70.0))
                                            .text_xs()
                                            .text_color(ACCENT_COLOR)
                                            .child(commit.short_id.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .overflow_hidden()
                                            .child(div().text_sm().child(commit.subject.clone()))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(MUTED_TEXT_COLOR)
                                                    .child(commit.author_name.clone()),
                                            ),
                                    )
                                    .when(!commit.references.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .text_color(SUCCESS_COLOR)
                                                .child(format!("{} refs", commit.references.len())),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
            .when(has_more, |history| {
                history.child(
                    div()
                        .flex()
                        .justify_center()
                        .p_2()
                        .border_t_1()
                        .border_color(BORDER_COLOR)
                        .child(
                            action_button("load-more-history", "Cargar más", true).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.load_more_history(repository_id, cx);
                                }),
                            ),
                        ),
                )
            })
            .when_some(details, |history, details| {
                history.child(
                    div()
                        .id("commit-details")
                        .flex()
                        .flex_col()
                        .gap_1()
                        .max_h(px(150.0))
                        .overflow_y_scroll()
                        .p_3()
                        .border_t_1()
                        .border_color(BORDER_COLOR)
                        .text_xs()
                        .child(format!("Commit {}", details.summary.id))
                        .child(format!(
                            "Autor: {} <{}> · {}",
                            details.summary.author_name,
                            details.summary.author_email,
                            details.summary.authored_at
                        ))
                        .child(format!("Padres: {}", details.parent_ids.join(" ")))
                        .when(!details.body.is_empty(), |panel| panel.child(details.body)),
                )
            })
            .into_any_element()
    }

    fn render_status_bar(&self, repository: Option<&RepositorySession>) -> AnyElement {
        let path = repository
            .map(|repository| repository.root_path.display().to_string())
            .unwrap_or_default();
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(26.0))
            .px_3()
            .border_t_1()
            .border_color(BORDER_COLOR)
            .bg(ELEVATED_BACKGROUND_COLOR)
            .text_xs()
            .text_color(MUTED_TEXT_COLOR)
            .child(path)
            .child(repository.map_or_else(
                || self.global_status_message.clone(),
                |repository| repository.status_message.clone(),
            ))
            .child(
                self.git_version
                    .clone()
                    .unwrap_or_else(|| "Git no detectado".to_owned()),
            )
            .into_any_element()
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(div().text_xl().child("No hay repositorios abiertos"))
            .child(
                div()
                    .text_sm()
                    .text_color(MUTED_TEXT_COLOR)
                    .child("Abre una carpeta que contenga un repositorio Git."),
            )
            .child(
                action_button("open-empty", "Abrir repositorio", true).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.open_repository(&OpenRepository, window, cx);
                    },
                )),
            )
            .when(!self.state.recent_repositories.is_empty(), |panel| {
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .mt_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(MUTED_TEXT_COLOR)
                                .child("Recientes"),
                        )
                        .children(self.state.recent_repositories.iter().enumerate().map(
                            |(index, recent)| {
                                let label = recent
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("Repositorio")
                                    .to_owned();
                                let path = recent.clone();
                                action_button(format!("recent-{index}"), label, true).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_recent_repository(path.clone(), cx);
                                    }),
                                )
                            },
                        )),
                )
            })
            .into_any_element()
    }

    fn render_inaccessible_repository(
        &self,
        repository: &RepositorySession,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let repository_id = repository.id;
        let path = repository.root_path.display().to_string();
        div()
            .flex()
            .flex_1()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_4()
            .child(div().text_xl().child("Repositorio no disponible"))
            .child(div().text_sm().text_color(MUTED_TEXT_COLOR).child(path))
            .child(div().text_sm().text_color(MUTED_TEXT_COLOR).child(
                repository.error.clone().unwrap_or_else(|| {
                    "La ruta no responde. Comprueba el disco o la red.".to_owned()
                }),
            ))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        action_button("retry-repository", "Reintentar", true).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.retry_repository_access(repository_id, cx);
                            }),
                        ),
                    )
                    .child(
                        action_button("close-inaccessible", "Cerrar pestaña", true).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.close_repository(repository_id, cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_repository = self.active_repository().cloned();
        div()
            .key_context("GitHelper")
            .on_action(cx.listener(Self::open_repository))
            .on_action(cx.listener(Self::close_active_repository))
            .on_action(cx.listener(Self::next_repository))
            .on_action(cx.listener(Self::previous_repository))
            .on_action(cx.listener(Self::refresh_active_repository))
            .on_action(cx.listener(Self::show_history))
            .on_action(cx.listener(Self::show_changes))
            .on_action(cx.listener(Self::create_commit))
            .on_action(cx.listener(Self::generate_commit_message))
            .flex()
            .flex_col()
            .size_full()
            .bg(BACKGROUND_COLOR)
            .text_color(PRIMARY_TEXT_COLOR)
            .child(self.render_repository_tabs(cx))
            .when_some(active_repository.clone(), |root, repository| {
                if repository.path_accessible {
                    root.child(self.render_toolbar(&repository, cx))
                        .child(self.render_internal_tabs(&repository, cx))
                        .child(match repository.selected_view {
                            RepositoryView::Changes => self.render_changes(&repository, cx),
                            RepositoryView::History => self.render_history(&repository, cx),
                        })
                } else {
                    root.child(self.render_inaccessible_repository(&repository, cx))
                }
            })
            .when(active_repository.is_none(), |root| {
                root.child(self.render_empty_state(cx))
            })
            .when_some(
                active_repository
                    .as_ref()
                    .and_then(|repository| repository.error.clone()),
                |root, error| {
                    root.child(
                        div()
                            .px_3()
                            .py_2()
                            .border_t_1()
                            .border_color(ERROR_COLOR)
                            .bg(ELEVATED_BACKGROUND_COLOR)
                            .text_sm()
                            .text_color(ERROR_COLOR)
                            .child(error),
                    )
                },
            )
            .when_some(self.global_error.clone(), |root, error| {
                root.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(ERROR_COLOR)
                        .bg(ELEVATED_BACKGROUND_COLOR)
                        .text_sm()
                        .text_color(ERROR_COLOR)
                        .child(error),
                )
            })
            .child(self.render_status_bar(active_repository.as_ref()))
    }
}

fn history_target_for_head(head: &HeadState) -> Option<(String, String)> {
    match head {
        HeadState::Branch {
            name,
            oid: Some(oid),
        } => Some((format!("refs/heads/{name}"), oid.clone())),
        HeadState::Detached { oid } if !oid.is_empty() => Some((oid.clone(), oid.clone())),
        HeadState::Branch { oid: None, .. } | HeadState::Detached { .. } | HeadState::Unborn => {
            None
        }
    }
}

fn history_target_is_current(repository: &RepositorySession, reference: &str, oid: &str) -> bool {
    repository.snapshot.history_reference.as_deref() == Some(reference)
        && repository.snapshot.history_oid.as_deref() == Some(oid)
}

const fn branch_kind_label(kind: BranchKind) -> &'static str {
    match kind {
        BranchKind::Local => "Local",
        BranchKind::RemoteTracking => "Remota",
    }
}

fn branch_upstream_label(upstream: &BranchUpstream) -> String {
    match upstream {
        BranchUpstream::Configured {
            full_name,
            ahead,
            behind,
        } => format!(
            "{} · ↑{ahead} ↓{behind}",
            full_name.strip_prefix("refs/remotes/").unwrap_or(full_name)
        ),
        BranchUpstream::NoUpstream => "Sin upstream".to_owned(),
        BranchUpstream::Gone { full_name } => format!(
            "Upstream ausente: {}",
            full_name.strip_prefix("refs/remotes/").unwrap_or(full_name)
        ),
    }
}

fn build_change_rows(
    repository_id: RepositoryId,
    changes: &[FileChange],
    collapsed_groups: &HashSet<(RepositoryId, ChangeRepresentation)>,
) -> Vec<ChangeListRow> {
    let groups = [
        (
            "Conflictos",
            ChangeRepresentation::Conflict,
            None,
            changes
                .iter()
                .filter(|change| change.is_conflicted)
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            "Cambios staged",
            ChangeRepresentation::Staged,
            Some(GroupAction::UnstageAll),
            changes
                .iter()
                .filter(|change| change.has_staged_change())
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            "Cambios",
            ChangeRepresentation::Worktree,
            Some(GroupAction::StageAll),
            changes
                .iter()
                .filter(|change| change.has_worktree_change())
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            "Sin seguimiento",
            ChangeRepresentation::Untracked,
            Some(GroupAction::StageAll),
            changes
                .iter()
                .filter(|change| change.is_untracked())
                .cloned()
                .collect::<Vec<_>>(),
        ),
    ];
    let mut rows = Vec::new();
    for (title, representation, action, group_changes) in groups {
        if group_changes.is_empty() {
            continue;
        }
        let is_collapsed = collapsed_groups.contains(&(repository_id, representation));
        rows.push(ChangeListRow::Group {
            title,
            count: group_changes.len(),
            action,
            representation,
            is_collapsed,
        });
        if !is_collapsed {
            rows.extend(group_changes.into_iter().map(|change| ChangeListRow::File {
                change,
                representation,
            }));
        }
    }
    rows
}

fn status_code(change: &FileChange, representation: ChangeRepresentation) -> &'static str {
    let kind = match representation {
        ChangeRepresentation::Conflict => ChangeKind::Unmerged,
        ChangeRepresentation::Staged => change.index_status,
        ChangeRepresentation::Worktree | ChangeRepresentation::Untracked => change.worktree_status,
    };
    match kind {
        ChangeKind::Unmodified => "",
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::Unmerged => "U",
        ChangeKind::Untracked => "?",
    }
}

fn status_color(representation: ChangeRepresentation) -> gpui::Rgba {
    match representation {
        ChangeRepresentation::Conflict => ERROR_COLOR,
        ChangeRepresentation::Staged => SUCCESS_COLOR,
        ChangeRepresentation::Worktree => WARNING_COLOR,
        ChangeRepresentation::Untracked => ACCENT_COLOR,
    }
}

fn capture_window_placement(window: &Window) -> WindowPlacement {
    let bounds = window.bounds();
    WindowPlacement {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    }
}

fn normalized_path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn change_row_action_id(action: &str, path: &Path, representation: ChangeRepresentation) -> String {
    let representation = match representation {
        ChangeRepresentation::Conflict => "conflict",
        ChangeRepresentation::Staged => "staged",
        ChangeRepresentation::Worktree => "worktree",
        ChangeRepresentation::Untracked => "untracked",
    };
    format!("{action}-{representation}-{}", path.display())
}

fn operation_running_message(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Refresh => "Actualizando estado…",
        OperationKind::Stage => "Añadiendo al staging area…",
        OperationKind::Unstage => "Quitando del staging area…",
        OperationKind::Discard => "Descartando cambios…",
        OperationKind::Commit => "Creando commit…",
        OperationKind::Fetch => "Ejecutando fetch…",
        OperationKind::Pull => "Ejecutando pull --ff-only…",
        OperationKind::Push => "Ejecutando push…",
        OperationKind::GenerateCommitMessage => "Generando mensaje…",
    }
}

fn operation_success_message(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Refresh => "Estado actualizado",
        OperationKind::Stage => "Cambios añadidos al staging area",
        OperationKind::Unstage => "Cambios retirados del staging area",
        OperationKind::Discard => "Cambios descartados",
        OperationKind::Commit => "Commit creado",
        OperationKind::Fetch => "Fetch completado",
        OperationKind::Pull => "Pull completado",
        OperationKind::Push => "Push completado",
        OperationKind::GenerateCommitMessage => "Mensaje generado",
    }
}

fn is_cancelled_error(error: &GitError) -> bool {
    matches!(
        error,
        GitError::Process(crate::process::ProcessError::Cancelled)
            | GitError::NotInstalled {
                source: crate::process::ProcessError::Cancelled
            }
    )
}

fn action_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .border_1()
        .border_color(BORDER_COLOR)
        .text_xs()
        .when(enabled, |button| {
            button.hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
        })
        .when(!enabled, |button| {
            button.text_color(MUTED_TEXT_COLOR).opacity(0.55)
        })
        .child(label.into())
}

fn tab_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h_full()
        .px_3()
        .text_sm()
        .when(selected, |tab| {
            tab.bg(SELECTED_BACKGROUND_COLOR)
                .border_b_1()
                .border_color(ACCENT_COLOR)
        })
        .hover(|style| style.bg(HOVER_BACKGROUND_COLOR).cursor_pointer())
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        process::ExitStatus,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc::{Receiver, Sender, channel},
        },
        thread,
    };

    use crate::process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};

    use super::*;

    struct ControlledSnapshotRunner {
        status_calls: AtomicUsize,
        first_status_started: Sender<()>,
        release_first_status: Mutex<Receiver<()>>,
    }

    impl ProcessRunner for ControlledSnapshotRunner {
        fn run(
            &self,
            request: ProcessRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProcessOutput, ProcessError> {
            let stdout = match request.label {
                "git-status" => {
                    let call = self.status_calls.fetch_add(1, Ordering::SeqCst);
                    if call == 0 {
                        self.first_status_started.send(()).unwrap();
                        self.release_first_status.lock().unwrap().recv().unwrap();
                        b"# branch.oid old\0# branch.head main\0".to_vec()
                    } else {
                        b"# branch.oid new\0# branch.head main\0? changed.txt\0".to_vec()
                    }
                }
                "git-remotes" | "git-branches" => Vec::new(),
                label => panic!("petición Git inesperada: {label}"),
            };
            Ok(ProcessOutput {
                status: success_status(),
                stdout,
                stderr: Vec::new(),
            })
        }
    }

    fn test_window(git_client: GitClient, repositories: Vec<RepositorySession>) -> MainWindow {
        MainWindow {
            loaded_state: None,
            state: AppState {
                active_repository_id: repositories.first().map(|repository| repository.id),
                repositories,
                ..AppState::default()
            },
            git_client,
            cursor_client: CursorClient::new(PathBuf::from("agent")),
            state_writer: None,
            window_placement: None,
            commit_inputs: HashMap::new(),
            selected_commit_details: HashMap::new(),
            commit_input_subscriptions: HashMap::new(),
            window_subscriptions: Vec::new(),
            active_refresh_cancellations: HashMap::new(),
            active_mutation_cancellations: HashMap::new(),
            generation_requests: HashMap::new(),
            next_generation_request_id: 0,
            repository_watchers: HashMap::new(),
            pending_refreshes: HashSet::new(),
            global_refresh_in_flight: None,
            collapsed_groups: HashSet::new(),
            change_rows: HashMap::new(),
            git_version: None,
            global_status_message: String::new(),
            global_error: None,
        }
    }

    #[test]
    fn refresh_invalidated_during_status_runs_one_follow_up_and_converges() {
        let (started_sender, started_receiver) = channel();
        let (release_sender, release_receiver) = channel();
        let runner = Arc::new(ControlledSnapshotRunner {
            status_calls: AtomicUsize::new(0),
            first_status_started: started_sender,
            release_first_status: Mutex::new(release_receiver),
        });
        let git_client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let repository = RepositorySession::new(PathBuf::from("repo"));
        let repository_id = repository.id;
        let mut window = test_window(git_client.clone(), vec![repository]);

        let first = window.prepare_refresh(repository_id).unwrap();
        let first_generation = first.generation;
        let first_include_history = first.include_history;
        let first_snapshot =
            thread::spawn(move || git_client.snapshot(&first.root_path, &first.cancellation));
        started_receiver.recv().unwrap();

        assert!(window.prepare_refresh(repository_id).is_none());
        assert!(window.prepare_refresh(repository_id).is_none());
        release_sender.send(()).unwrap();

        let first_result = first_snapshot.join().unwrap();
        let first_outcome = window.finish_refresh(
            repository_id,
            first_generation,
            first_include_history,
            first_result,
        );
        assert_eq!(first_outcome.continuation, RefreshContinuation::Repeat);

        let follow_up = window.prepare_refresh(repository_id).unwrap();
        let follow_up_result = window
            .git_client
            .snapshot(&follow_up.root_path, &follow_up.cancellation);
        let follow_up_outcome = window.finish_refresh(
            repository_id,
            follow_up.generation,
            follow_up.include_history,
            follow_up_result,
        );

        assert_eq!(
            follow_up_outcome.continuation,
            RefreshContinuation::Complete
        );
        assert_eq!(runner.status_calls.load(Ordering::SeqCst), 2);
        let repository = window
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == repository_id)
            .unwrap();
        assert_eq!(
            repository.snapshot.head,
            HeadState::Branch {
                name: "main".to_owned(),
                oid: Some("new".to_owned()),
            }
        );
        assert_eq!(repository.snapshot.changes.len(), 1);
    }

    #[test]
    fn closed_session_rejects_late_refresh_and_watcher_installation() {
        let old_repository = RepositorySession::new(PathBuf::from("repo"));
        let old_id = old_repository.id;
        let mut window = test_window(GitClient::default(), vec![old_repository]);
        let pending = window.prepare_refresh(old_id).unwrap();

        window.state.repositories.clear();
        let reopened_repository = RepositorySession::new(PathBuf::from("repo"));
        let reopened_id = reopened_repository.id;
        window.state.repositories.push(reopened_repository);

        let outcome = window.finish_refresh(
            old_id,
            pending.generation,
            pending.include_history,
            Ok(RepositorySnapshot {
                head: HeadState::Branch {
                    name: "stale".to_owned(),
                    oid: Some("old".to_owned()),
                },
                ..RepositorySnapshot::default()
            }),
        );

        assert!(!outcome.succeeded);
        assert!(!window.repository_session_is_open(old_id));
        assert!(window.repository_session_is_open(reopened_id));
        assert_eq!(
            window.state.repositories[0].snapshot.head,
            HeadState::Unborn
        );
    }

    #[test]
    fn invalidation_during_mutation_starts_reconciliation_after_cancellation() {
        let repository = RepositorySession::new(PathBuf::from("repo"));
        let repository_id = repository.id;
        let mut window = test_window(GitClient::default(), vec![repository]);
        window.state.repositories[0].mutation_state = MutationState::Running {
            kind: OperationKind::Stage,
            generation: 0,
        };

        assert!(window.prepare_refresh(repository_id).is_none());
        window.state.repositories[0].mutation_state = MutationState::Cancelled {
            kind: OperationKind::Stage,
            message: "cancelada".to_owned(),
        };

        assert!(window.prepare_refresh(repository_id).is_some());
        assert!(matches!(
            window.state.repositories[0].refresh_state,
            RefreshState::Running { .. }
        ));
    }

    #[test]
    fn refresh_error_is_scoped_to_the_repository_that_failed() {
        let first = RepositorySession::new(PathBuf::from("first"));
        let second = RepositorySession::new(PathBuf::from("second"));
        let first_id = first.id;
        let second_id = second.id;
        let mut window = test_window(GitClient::default(), vec![first, second]);
        let pending = window.prepare_refresh(second_id).unwrap();

        let outcome = window.finish_refresh(
            second_id,
            pending.generation,
            pending.include_history,
            Err(GitError::InvalidStatus {
                message: "respuesta rota".to_owned(),
            }),
        );

        assert!(outcome.should_notify);
        let first = window
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == first_id)
            .unwrap();
        let second = window
            .state
            .repositories
            .iter()
            .find(|repository| repository.id == second_id)
            .unwrap();
        assert!(first.error.is_none());
        assert!(matches!(first.refresh_state, RefreshState::Idle));
        assert!(second.error.is_some());
        assert!(matches!(second.refresh_state, RefreshState::Failed { .. }));
    }

    #[test]
    fn rapid_branch_selection_rejects_the_previous_response() {
        let mut repository = RepositorySession::new(PathBuf::from("repo"));
        {
            let snapshot = Arc::make_mut(&mut repository.snapshot);
            snapshot.history_reference = Some("refs/heads/first".to_owned());
            snapshot.history_oid = Some("1111".to_owned());
        }
        assert!(history_target_is_current(
            &repository,
            "refs/heads/first",
            "1111"
        ));

        {
            let snapshot = Arc::make_mut(&mut repository.snapshot);
            snapshot.history_reference = Some("refs/heads/second".to_owned());
            snapshot.history_oid = Some("2222".to_owned());
        }
        assert!(!history_target_is_current(
            &repository,
            "refs/heads/first",
            "1111"
        ));
        assert!(history_target_is_current(
            &repository,
            "refs/heads/second",
            "2222"
        ));
    }

    #[test]
    fn current_branch_history_is_tied_to_its_oid() {
        assert_eq!(
            history_target_for_head(&HeadState::Branch {
                name: "feature/test".to_owned(),
                oid: Some("abcd".to_owned()),
            }),
            Some(("refs/heads/feature/test".to_owned(), "abcd".to_owned()))
        );
        assert_eq!(
            history_target_for_head(&HeadState::Detached {
                oid: "deadbeef".to_owned(),
            }),
            Some(("deadbeef".to_owned(), "deadbeef".to_owned()))
        );
        assert_eq!(history_target_for_head(&HeadState::Unborn), None);
    }

    #[test]
    fn active_repository_waits_for_the_single_global_refresh_slot() {
        let first = RepositorySession::new(PathBuf::from("first"));
        let second = RepositorySession::new(PathBuf::from("second"));
        let first_id = first.id;
        let second_id = second.id;
        let mut window = test_window(GitClient::default(), vec![first, second]);

        let first_refresh = window.prepare_refresh(first_id).unwrap();
        window.state.active_repository_id = Some(second_id);
        assert!(window.prepare_refresh(second_id).is_none());
        assert!(window.pending_refreshes.contains(&second_id));

        window.finish_refresh(
            first_id,
            first_refresh.generation,
            first_refresh.include_history,
            Ok(RepositorySnapshot::default()),
        );
        let second_refresh = window.prepare_refresh(second_id).unwrap();

        assert_eq!(window.global_refresh_in_flight, Some(second_id));
        assert!(!window.pending_refreshes.contains(&second_id));
        assert!(second_refresh.generation > 0);
    }

    #[test]
    fn virtualized_change_rows_have_uniform_height() {
        let group = ChangeListRow::Group {
            title: "Cambios staged",
            count: 1,
            action: Some(GroupAction::UnstageAll),
            representation: ChangeRepresentation::Staged,
            is_collapsed: false,
        };
        let file = ChangeListRow::File {
            change: FileChange {
                path: PathBuf::from("src/main.rs"),
                original_path: None,
                index_status: ChangeKind::Modified,
                worktree_status: ChangeKind::Modified,
                is_conflicted: false,
            },
            representation: ChangeRepresentation::Staged,
        };

        assert_eq!(group.height_px(), file.height_px());
    }

    #[test]
    fn staged_and_worktree_rows_have_distinct_action_ids() {
        let path = Path::new("src/main.rs");

        for action in ["discard", "stage-toggle"] {
            assert_ne!(
                change_row_action_id(action, path, ChangeRepresentation::Staged),
                change_row_action_id(action, path, ChangeRepresentation::Worktree),
                "las filas staged y worktree no pueden compartir el id de {action}"
            );
        }
    }

    #[cfg(windows)]
    fn success_status() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    #[cfg(unix)]
    fn success_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    #[test]
    fn cancellation_is_classified_separately_from_git_failure() {
        let cancelled = GitError::Process(crate::process::ProcessError::Cancelled);
        let failed = GitError::CommandFailed {
            exit_code: Some(1),
            stderr: "hook rejected the commit".to_owned(),
        };

        assert!(is_cancelled_error(&cancelled));
        assert!(!is_cancelled_error(&failed));
    }
}
