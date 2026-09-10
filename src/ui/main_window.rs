use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, Context, Entity, IntoElement, PathPromptOptions, PromptButton, PromptLevel, Render,
    Subscription, Window, div, prelude::*, px, uniform_list,
};

use crate::{
    actions::{
        CloseActiveRepository, CreateCommit, GenerateCommitMessage, NextRepository, OpenRepository,
        PreviousRepository, RefreshRepository, ShowChanges, ShowHistory,
    },
    cursor::{CursorClient, build_cursor_context, resolve_cursor_executable},
    domain::{
        AppState, ChangeKind, CommitDetails, FileChange, HeadState, OperationKind, OperationState,
        RepositoryId, RepositorySession, RepositorySnapshot, RepositoryView,
    },
    git::{DiscardPlan, GitClient, GitError, plan_discard, plan_fetch, plan_pull, plan_push},
    persistence::{AppStateStore, PersistedAppState},
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

/// Modelo y presentación de la ventana principal.
pub struct MainWindow {
    state: AppState,
    git_client: GitClient,
    cursor_client: CursorClient,
    state_store: Option<AppStateStore>,
    commit_inputs: HashMap<RepositoryId, Entity<CommitInput>>,
    selected_commit_details: HashMap<RepositoryId, CommitDetails>,
    commit_input_subscriptions: Vec<Subscription>,
    window_subscriptions: Vec<Subscription>,
    active_cancellations: HashMap<RepositoryId, CancellationToken>,
    repository_watchers: HashMap<RepositoryId, RepositoryWatcher>,
    collapsed_groups: HashSet<(RepositoryId, ChangeRepresentation)>,
    git_version: Option<String>,
    status_message: String,
    global_error: Option<String>,
}

#[allow(
    clippy::assigning_clones,
    clippy::too_many_lines,
    clippy::unused_self,
    reason = "La entidad GPUI conserva métodos listener y render cohesionados; los mensajes cortos priorizan legibilidad"
)]
impl MainWindow {
    /// Restaura sesiones persistidas sin bloquear la creación de la ventana.
    #[must_use]
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state_store = AppStateStore::default_location().ok();
        let mut state = state_store
            .as_ref()
            .and_then(|store| store.load().ok())
            .map_or_else(AppState::default, |loaded| loaded.state.into_app_state());
        state
            .repositories
            .retain(|repository| repository.root_path.is_dir());
        if state
            .active_repository_id
            .is_some_and(|active_id| !state.repositories.iter().any(|repo| repo.id == active_id))
        {
            state.active_repository_id = state.repositories.first().map(|repo| repo.id);
        }
        let cursor_executable = resolve_cursor_executable(state.settings.cursor_cli_path.clone());
        let mut result = Self {
            state,
            git_client: GitClient::default(),
            cursor_client: CursorClient::new(cursor_executable),
            state_store,
            commit_inputs: HashMap::new(),
            selected_commit_details: HashMap::new(),
            commit_input_subscriptions: Vec::new(),
            window_subscriptions: Vec::new(),
            active_cancellations: HashMap::new(),
            repository_watchers: HashMap::new(),
            collapsed_groups: HashSet::new(),
            git_version: None,
            status_message: "Preparando Git Helper…".to_owned(),
            global_error: None,
        };
        let repository_ids: Vec<RepositoryId> = result
            .state
            .repositories
            .iter()
            .map(|repo| repo.id)
            .collect();
        for repository_id in repository_ids {
            result.create_commit_input(repository_id, cx);
        }
        result
    }

    /// Detecta Git y refresca todas las pestañas restauradas en background.
    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let activation_subscription = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() || this.git_version.is_none() {
                return;
            }
            let Some(repository_id) = this.state.active_repository_id else {
                return;
            };
            let can_refresh = this
                .state
                .repositories
                .iter()
                .find(|repository| repository.id == repository_id)
                .is_some_and(|repository| {
                    !matches!(repository.operation_state, OperationState::Running { .. })
                });
            if can_refresh {
                this.refresh_repository(repository_id, cx);
            }
        });
        self.window_subscriptions.push(activation_subscription);
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
                        this.status_message = "Git detectado".to_owned();
                        let repository_ids: Vec<RepositoryId> = this
                            .state
                            .repositories
                            .iter()
                            .map(|repository| repository.id)
                            .collect();
                        for repository_id in repository_ids {
                            this.refresh_repository(repository_id, cx);
                        }
                    }
                    Err(error) => {
                        this.global_error = Some(format!(
                            "{error}. Instala Git for Windows y pulsa Actualizar."
                        ));
                        this.status_message = "Git no está disponible".to_owned();
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

    fn create_commit_input(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let input = cx.new(CommitInput::new);
        let subscription = cx.subscribe(&input, |_this, _input, _: &CommitMessageChanged, cx| {
            cx.notify();
        });
        self.commit_inputs.insert(repository_id, input);
        self.commit_input_subscriptions.push(subscription);
    }

    fn save_state(&mut self) {
        let Some(store) = &self.state_store else {
            return;
        };
        if let Err(error) = store.save(&PersistedAppState::from_app_state(&self.state, None)) {
            self.global_error = Some(format!("No se pudo guardar el estado: {error}"));
        }
    }

    fn open_repository(&mut self, _: &OpenRepository, _: &mut Window, cx: &mut Context<Self>) {
        let path_receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Abrir repositorio Git".into()),
        });
        let git_client = self.git_client.clone();
        self.status_message = "Seleccionando repositorio…".to_owned();
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
                        this.status_message = "No se pudo abrir el repositorio".to_owned();
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
            self.status_message = "El repositorio ya estaba abierto".to_owned();
            self.save_state();
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
        self.create_commit_input(repository_id, cx);
        self.status_message = "Repositorio abierto".to_owned();
        self.global_error = None;
        self.save_state();
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
        if let Some(cancellation) = self.active_cancellations.remove(&repository_id) {
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
        self.selected_commit_details.remove(&repository_id);
        self.repository_watchers.remove(&repository_id);
        self.collapsed_groups.retain(|(id, _)| *id != repository_id);
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
        }
        self.status_message = "Pestaña cerrada".to_owned();
        self.save_state();
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
        self.state.active_repository_id = Some(self.state.repositories[next].id);
        self.save_state();
        cx.notify();
    }

    fn refresh_active_repository(
        &mut self,
        _: &RefreshRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_version.is_none() {
            self.initialize(window, cx);
        }
        if let Some(repository_id) = self.state.active_repository_id {
            self.refresh_repository(repository_id, cx);
        }
    }

    fn refresh_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        repository.refresh_generation = repository.refresh_generation.saturating_add(1);
        let generation = repository.refresh_generation;
        repository.operation_state = OperationState::Running {
            kind: OperationKind::Refresh,
            generation,
        };
        let root_path = repository.root_path.clone();
        let git_client = self.git_client.clone();
        let cancellation = CancellationToken::default();
        self.active_cancellations
            .insert(repository_id, cancellation.clone());
        self.status_message = "Actualizando estado…".to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.snapshot(&root_path, INITIAL_HISTORY_LIMIT, &cancellation)
                })
                .await;
            this.update(cx, |this, cx| {
                let succeeded = result.is_ok();
                this.finish_refresh(repository_id, generation, result);
                if succeeded {
                    this.ensure_watcher(repository_id, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let git_client = self.git_client.clone();
        cx.spawn(async move |this, cx| {
            let git_directory_result = cx
				.background_spawn({
					let root_path = root_path.clone();
					async move {
						git_client.git_directory(&root_path, &CancellationToken::default())
					}
				})
				.await;
            let Ok(git_directory) = git_directory_result else {
                return;
            };
            let (sender, receiver) = async_channel::bounded(1);
            let watcher_result = RepositoryWatcher::start(
                &root_path,
                &git_directory,
                Arc::new(move || {
                    let _ = sender.try_send(());
                }),
            );
            let Ok(watcher) = watcher_result else {
                return;
            };
            if this
                .update(cx, |this, _| {
                    this.repository_watchers.insert(repository_id, watcher);
                })
                .is_err()
            {
                return;
            }
            while receiver.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        let can_refresh = this
                            .state
                            .repositories
                            .iter()
                            .find(|repository| repository.id == repository_id)
                            .is_some_and(|repository| {
                                !matches!(
                                    repository.operation_state,
                                    OperationState::Running { .. }
                                )
                            });
                        if can_refresh {
                            this.refresh_repository(repository_id, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_refresh(
        &mut self,
        repository_id: RepositoryId,
        generation: u64,
        result: Result<RepositorySnapshot, GitError>,
    ) {
        let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        if repository.refresh_generation != generation {
            return;
        }
        self.active_cancellations.remove(&repository_id);
        match result {
            Ok(snapshot) => {
                repository.snapshot = snapshot;
                repository.operation_state = OperationState::Succeeded {
                    kind: OperationKind::Refresh,
                    message: "Estado actualizado".to_owned(),
                };
                self.status_message = "Estado actualizado".to_owned();
                self.global_error = None;
            }
            Err(error) => {
                let details = error.technical_details();
                repository.operation_state = OperationState::Failed {
                    kind: OperationKind::Refresh,
                    message: error.to_string(),
                    details: details.clone(),
                };
                self.status_message = "Error al actualizar".to_owned();
                self.global_error = Some(details);
            }
        }
    }

    fn select_view(&mut self, view: RepositoryView, cx: &mut Context<Self>) {
        let Some(repository) = self.active_repository_mut() else {
            return;
        };
        repository.selected_view = view;
        self.save_state();
        cx.notify();
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
                self.global_error = Some(error.to_string());
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
            self.global_error = Some(
                "No hay cambios descartables. Los conflictos y cambios staged sin HEAD deben resolverse o quitarse del stage manualmente."
                    .to_owned(),
            );
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
            self.global_error = Some("No hay cambios staged para generar un mensaje".to_owned());
            cx.notify();
            return;
        }
        if matches!(repository.operation_state, OperationState::Running { .. }) {
            self.global_error = Some("Ya hay otra operación activa en este repositorio".to_owned());
            cx.notify();
            return;
        }
        if let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        {
            repository.operation_state = OperationState::Running {
                kind: OperationKind::GenerateCommitMessage,
                generation: repository.refresh_generation,
            };
        }
        let git_client = self.git_client.clone();
        let cursor_client = self.cursor_client.clone();
        let cancellation = CancellationToken::default();
        self.active_cancellations
            .insert(repository_id, cancellation.clone());
        self.status_message = "Generando mensaje…".to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let context_root = root_path.clone();
            let context_cancellation = cancellation.clone();
            let context_result = cx
                .background_spawn(async move {
                    let data = git_client
                        .staged_context(&context_root, &context_cancellation)
                        .map_err(|error| error.to_string())?;
                    build_cursor_context(&data).map_err(|error| error.to_string())
                })
                .await;
            let context = match context_result {
                Ok(context) => context,
                Err(error) => {
                    this.update_in(cx, |this, _, cx| {
                        this.finish_message_generation(repository_id, Err(error), cx);
                    })
                    .ok();
                    return;
                }
            };
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
                            repository_id,
                            Err("Generación cancelada antes de enviar el contexto".to_owned()),
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            }
            let result = cx
                .background_spawn(async move {
                    cursor_client
                        .generate_commit_message(&root_path, context.prompt, &cancellation)
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update_in(cx, |this, _, cx| {
                this.finish_message_generation(repository_id, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn finish_message_generation(
        &mut self,
        repository_id: RepositoryId,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.active_cancellations.remove(&repository_id);
        match result {
            Ok(message) => {
                if let Some(input) = self.commit_inputs.get(&repository_id) {
                    input.update(cx, |input, cx| input.set_content(message, cx));
                }
                self.status_message = "Mensaje generado; revísalo antes del commit".to_owned();
                self.global_error = None;
            }
            Err(error) => {
                self.status_message = "No se pudo generar el mensaje".to_owned();
                self.global_error = Some(error);
            }
        }
        if let Some(repository) = self
            .state
            .repositories
            .iter_mut()
            .find(|repository| repository.id == repository_id)
        {
            repository.operation_state = OperationState::Idle;
        }
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
                self.global_error = Some(error.to_string());
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
                            this.global_error = Some(error.to_string());
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
                self.global_error = Some(error.to_string());
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
        if let Some(cancellation) = self.active_cancellations.get(&repository_id) {
            cancellation.cancel();
            self.status_message = "Cancelando operación…".to_owned();
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
        if matches!(repository.operation_state, OperationState::Running { .. }) {
            self.global_error = Some("Ya hay otra operación activa en este repositorio".to_owned());
            cx.notify();
            return;
        }
        let root_path = repository.root_path.clone();
        repository.operation_state = OperationState::Running {
            kind,
            generation: repository.refresh_generation,
        };
        let git_client = self.git_client.clone();
        let cancellation = CancellationToken::default();
        self.active_cancellations
            .insert(repository_id, cancellation.clone());
        self.status_message = operation_running_message(kind).to_owned();
        self.global_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { work(git_client, root_path, cancellation) })
                .await;
            this.update(cx, |this, cx| {
                this.active_cancellations.remove(&repository_id);
                match result {
                    Ok(()) => {
                        if clear_commit_message
                            && let Some(input) = this.commit_inputs.get(&repository_id)
                        {
                            input.update(cx, CommitInput::clear);
                        }
                        this.status_message = operation_success_message(kind).to_owned();
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.operation_state = OperationState::Idle;
                        }
                        this.refresh_repository(repository_id, cx);
                    }
                    Err(error) => {
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.operation_state = OperationState::Failed {
                                kind,
                                message: error.to_string(),
                                details: error.technical_details(),
                            };
                        }
                        this.status_message = "La operación falló".to_owned();
                        this.global_error = Some(error.technical_details());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_repository(&mut self, repository_id: RepositoryId, cx: &mut Context<Self>) {
        self.state.active_repository_id = Some(repository_id);
        self.save_state();
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
        let is_running = matches!(repository.operation_state, OperationState::Running { .. });
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
                        action_button("fetch", "Fetch", !is_running).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.fetch(repository_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        action_button("pull", "Pull", !is_running).on_click(
                            cx.listener(move |this, _, _, cx| this.pull(repository_id, cx)),
                        ),
                    )
                    .child(
                        action_button("push", "Push", !is_running).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.push(repository_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        action_button("refresh", "Actualizar", !is_running).on_click(cx.listener(
                            move |this, _, _, cx| this.refresh_repository(repository_id, cx),
                        )),
                    )
                    .when(is_running, |row| {
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

    fn render_changes(&self, repository: &RepositorySession, cx: &mut Context<Self>) -> AnyElement {
        let rows = Arc::new(build_change_rows(
            repository.id,
            &repository.snapshot.changes,
            &self.collapsed_groups,
        ));
        let row_count = rows.len();
        let repository_id = repository.id;
        let input = self.commit_inputs.get(&repository_id).cloned();
        let staged_count = repository
            .snapshot
            .changes
            .iter()
            .filter(|change| change.has_staged_change())
            .count();
        let is_running = matches!(repository.operation_state, OperationState::Running { .. });
        let message_is_empty = input
            .as_ref()
            .is_none_or(|input| input.read(cx).content().trim().is_empty());
        let commit_enabled = staged_count > 0 && !message_is_empty && !is_running;

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
                                            !repository.snapshot.changes.is_empty() && !is_running,
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
                                            staged_count > 0 && !is_running,
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
                        action_button(format!("group-action-{title}"), label, true).on_click(
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
                                        true,
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
                                        true,
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
                        this.global_error = None;
                    }
                    Err(error) => this.global_error = Some(error.to_string()),
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
            .iter()
            .find(|repository| repository.id == repository_id)
        else {
            return;
        };
        let root_path = repository.root_path.clone();
        let offset = repository.snapshot.commits.len();
        let git_client = self.git_client.clone();
        self.status_message = "Cargando más commits…".to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    git_client.history(
                        &root_path,
                        INITIAL_HISTORY_LIMIT + 1,
                        offset,
                        &CancellationToken::default(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(commits) => {
                        let has_more = commits.len() > INITIAL_HISTORY_LIMIT;
                        if let Some(repository) = this
                            .state
                            .repositories
                            .iter_mut()
                            .find(|repository| repository.id == repository_id)
                        {
                            repository.snapshot.commits.extend(
                                commits
                                    .into_iter()
                                    .take(INITIAL_HISTORY_LIMIT)
                                    .map(|commit| commit.summary),
                            );
                            repository.snapshot.has_more_commits = has_more;
                        }
                        this.status_message = "Historial actualizado".to_owned();
                    }
                    Err(error) => this.global_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_history(&self, repository: &RepositorySession, cx: &mut Context<Self>) -> AnyElement {
        let commits = Arc::new(repository.snapshot.commits.clone());
        let count = commits.len();
        let repository_id = repository.id;
        let selected_commit = repository.selected_commit.clone();
        let has_more = repository.snapshot.has_more_commits;
        let details = self.selected_commit_details.get(&repository_id).cloned();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .child(
                uniform_list(
                    "history-list",
                    count,
                    cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                        commits[range]
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
            .child(self.status_message.clone())
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
                root.child(self.render_toolbar(&repository, cx))
                    .child(self.render_internal_tabs(&repository, cx))
                    .child(match repository.selected_view {
                        RepositoryView::Changes => self.render_changes(&repository, cx),
                        RepositoryView::History => self.render_history(&repository, cx),
                    })
            })
            .when(active_repository.is_none(), |root| {
                root.child(self.render_empty_state(cx))
            })
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
    use std::path::PathBuf;

    use super::*;

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
}
