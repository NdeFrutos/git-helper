use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use crate::{
    domain::{
        BranchKind, BranchReference, BranchUpstream, CommitDetails, HeadState, HistoryPage, Remote,
        RemoteOperationPlan, RepositorySnapshot,
    },
    process::{
        CancellationToken, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner,
        SystemProcessRunner,
    },
};

use super::{
    BRANCH_FORMAT, DiscardMode, DiscardPlan, GitError, LOG_FORMAT, PathFailure, StagedContextData,
    command_line_cost, parse_branch_refs, parse_log, parse_status, plan_pathspec_batches,
    resolve_upstream, validate_existing_path_inside_repository, validate_pathspecs,
    validate_relative_path,
};

const LOCAL_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_OPERATION_TIMEOUT: Duration = Duration::from_mins(15);

/// Única puerta de entrada a `git.exe`.
#[derive(Clone)]
pub struct GitClient {
    executable: PathBuf,
    runner: Arc<dyn ProcessRunner>,
    remote_cache: Arc<Mutex<HashMap<PathBuf, Vec<Remote>>>>,
    branch_cache: Arc<Mutex<HashMap<PathBuf, Vec<BranchReference>>>>,
}

impl Default for GitClient {
    fn default() -> Self {
        Self::new(PathBuf::from("git"))
    }
}

impl GitClient {
    /// Crea un cliente de producción.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            runner: Arc::new(SystemProcessRunner),
            remote_cache: Arc::new(Mutex::new(HashMap::new())),
            branch_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Crea un cliente con ejecución inyectada para pruebas.
    #[must_use]
    pub fn with_runner(executable: PathBuf, runner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            executable,
            runner,
            remote_cache: Arc::new(Mutex::new(HashMap::new())),
            branch_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Comprueba que Git está disponible y devuelve su versión.
    pub fn detect_version(&self, cancellation: &CancellationToken) -> Result<String, GitError> {
        let output = self
            .run_process(
                "git-version",
                vec![OsString::from("--version")],
                None,
                LOCAL_OPERATION_TIMEOUT,
                cancellation,
                true,
            )
            .map_err(|source| GitError::NotInstalled { source })?;
        let output = require_success(output)?;
        decode_trimmed_stdout(&output, "git --version")
    }

    /// Resuelve una carpeta cualquiera a la raíz canónica del repositorio.
    pub fn discover_repository(
        &self,
        selected_path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, GitError> {
        let output = self.run_git_read_only(
            "git-discover-repository",
            selected_path,
            ["rev-parse", "--show-toplevel"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        let root = PathBuf::from(decode_trimmed_stdout(
            &require_success(output)?,
            "git rev-parse --show-toplevel",
        )?);
        root.canonicalize()
            .map_err(|source| GitError::Io { path: root, source })
    }

    /// Obtiene la ubicación física de los metadatos Git para configurar watchers.
    pub fn git_directory(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, GitError> {
        let output = self.run_git_read_only(
            "git-directory",
            repository_root,
            ["rev-parse", "--absolute-git-dir"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        Ok(PathBuf::from(decode_trimmed_stdout(
            &require_success(output)?,
            "git rev-parse --absolute-git-dir",
        )?))
    }

    /// Obtiene el directorio común de Git, que contiene refs compartidas por worktrees.
    pub fn git_common_directory(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<PathBuf, GitError> {
        let output = self.run_git_read_only(
            "git-common-directory",
            repository_root,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        Ok(PathBuf::from(decode_trimmed_stdout(
            &require_success(output)?,
            "git rev-parse --git-common-dir",
        )?))
    }

    /// Lee status, remotes y referencias de ramas como un snapshot ligero para la UI.
    pub fn snapshot(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<RepositorySnapshot, GitError> {
        let (status, remotes, branches) = thread::scope(|scope| {
            let remotes_worker = scope.spawn(|| self.remotes(repository_root, cancellation));
            let branches_worker = scope.spawn(|| self.branch_refs(repository_root, cancellation));
            let status = self.status(repository_root, cancellation);
            let remotes = remotes_worker
                .join()
                .map_err(|_| GitError::WorkerPanicked {
                    operation: "git remote",
                })?;
            let branches = branches_worker
                .join()
                .map_err(|_| GitError::WorkerPanicked {
                    operation: "git for-each-ref",
                })?;
            Ok::<_, GitError>((status?, remotes?, branches?))
        })?;
        let upstream = resolve_upstream(&status, &remotes);
        let branches = mark_active_branch(branches, &status.head);

        Ok(RepositorySnapshot {
            head: status.head,
            upstream,
            remotes,
            branches,
            changes: status.changes,
            commits: Vec::new(),
            has_more_commits: false,
            history_reference: None,
            history_oid: None,
        })
    }

    /// Lee el snapshot y una primera página de historial de forma concurrente.
    pub fn snapshot_with_history(
        &self,
        repository_root: &Path,
        history_limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<RepositorySnapshot, GitError> {
        let mut snapshot = self.snapshot(repository_root, cancellation)?;
        let page = match history_target(&snapshot.head) {
            Some((reference, oid)) if reference.starts_with("refs/") => self.history_for_oid(
                repository_root,
                &reference,
                &oid,
                history_limit + 1,
                0,
                cancellation,
            )?,
            Some((_, oid)) => self.history_for_revision(
                repository_root,
                &oid,
                history_limit + 1,
                0,
                cancellation,
            )?,
            None => HistoryPage {
                reference: String::new(),
                oid: String::new(),
                commits: Vec::new(),
            },
        };
        let has_more_commits = page.commits.len() > history_limit;
        let commits = page
            .commits
            .into_iter()
            .take(history_limit)
            .map(|commit| commit.summary)
            .collect();
        snapshot.history_reference = (!page.reference.is_empty()).then_some(page.reference);
        snapshot.history_oid = (!page.oid.is_empty()).then_some(page.oid);

        Ok(RepositorySnapshot {
            commits,
            has_more_commits,
            ..snapshot
        })
    }

    /// Lee el estado machine-readable del repositorio.
    pub fn status(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<crate::domain::StatusSnapshot, GitError> {
        let output = self.run_git_read_only(
            "git-status",
            repository_root,
            [
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=all",
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        parse_status(&require_success(output)?.stdout)
    }

    /// Enumera remotes configurados sin consultar la red.
    pub fn remotes(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Remote>, GitError> {
        if let Some(remotes) = self
            .remote_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(repository_root)
            .cloned()
        {
            return Ok(remotes);
        }
        let output = self.run_git_read_only(
            "git-remotes",
            repository_root,
            ["remote"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        let output = require_success(output)?;
        let text = std::str::from_utf8(&output.stdout).map_err(|_| GitError::InvalidUtf8 {
            context: "git remote",
        })?;
        let remotes = text
            .lines()
            .map(str::trim_end)
            .filter(|name| !name.is_empty())
            .map(|name| Remote {
                name: name.to_owned(),
            })
            .collect::<Vec<_>>();
        self.remote_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(repository_root.to_path_buf(), remotes.clone());
        Ok(remotes)
    }

    /// Enumera ramas locales y referencias remote-tracking en una sola lectura cacheable.
    pub fn branches(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<BranchReference>, GitError> {
        let status = self.status(repository_root, cancellation)?;
        let branches = self.branch_refs(repository_root, cancellation)?;
        Ok(mark_active_branch(branches, &status.head))
    }

    fn branch_refs(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<BranchReference>, GitError> {
        if let Some(branches) = self
            .branch_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(repository_root)
            .cloned()
        {
            return Ok(branches);
        }
        let output = self.run_git_read_only(
            "git-branches",
            repository_root,
            [
                "for-each-ref",
                &format!("--format={BRANCH_FORMAT}"),
                "refs/heads",
                "refs/remotes",
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        let branches = parse_branch_refs(&require_success(output)?.stdout)?;
        self.branch_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(repository_root.to_path_buf(), branches.clone());
        Ok(branches)
    }

    /// Invalida los remotes cacheados cuando cambia la configuración local.
    pub fn invalidate_remotes(&self, repository_root: &Path) {
        self.remote_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(repository_root);
        self.branch_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(repository_root);
    }

    /// Invalida el inventario cuando el watcher detecta cambios en refs.
    pub fn invalidate_branches(&self, repository_root: &Path) {
        self.branch_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(repository_root);
    }

    /// Enumera rutas ignoradas para que el watcher descarte árboles ruidosos.
    pub fn ignored_paths(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<PathBuf>, GitError> {
        let output = self.run_git_read_only(
            "git-ignored-paths",
            repository_root,
            [
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        let output = require_success(output)?;
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                std::str::from_utf8(path)
                    .map(|path| repository_root.join(path.trim_end_matches('/')))
                    .map_err(|_| GitError::InvalidUtf8 {
                        context: "git ls-files --ignored",
                    })
            })
            .collect()
    }

    /// Carga una página de historial, incluyendo detalles para la selección.
    pub fn history(
        &self,
        repository_root: &Path,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CommitDetails>, GitError> {
        let arguments = vec![
            OsString::from("log"),
            OsString::from("--all"),
            OsString::from("--date-order"),
            OsString::from("--decorate=full"),
            OsString::from("-z"),
            OsString::from(format!("--format={LOG_FORMAT}")),
            OsString::from(format!("--max-count={limit}")),
            OsString::from(format!("--skip={offset}")),
        ];
        let output = self.run_git_os_read_only(
            "git-history",
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        if output.status.success() {
            parse_log(&output.stdout)
        } else if is_empty_history_error(&output.stderr) {
            Ok(Vec::new())
        } else {
            Err(command_failed(&output))
        }
    }

    /// Resuelve una rama/ref y carga su historial sin cambiar el checkout.
    pub fn history_for_ref(
        &self,
        repository_root: &Path,
        reference: &str,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<HistoryPage, GitError> {
        validate_history_ref(reference)?;
        let oid = self.resolve_ref_oid(repository_root, reference, cancellation)?;
        self.history_for_oid(
            repository_root,
            reference,
            &oid,
            limit,
            offset,
            cancellation,
        )
    }

    /// Pagina desde el OID fijado al seleccionar la referencia.
    ///
    /// Aunque la rama se mueva mientras la petición está en vuelo, todas las
    /// páginas pertenecen al mismo grafo hasta que la UI seleccione de nuevo.
    pub fn history_for_oid(
        &self,
        repository_root: &Path,
        reference: &str,
        oid: &str,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<HistoryPage, GitError> {
        let oid = validate_oid(oid)?;
        if reference != oid {
            validate_history_ref(reference)?;
        }
        let commits =
            self.history_from_revision(repository_root, &oid, limit, offset, cancellation)?;
        Ok(HistoryPage {
            reference: reference.to_owned(),
            oid,
            commits,
        })
    }

    fn history_for_revision(
        &self,
        repository_root: &Path,
        revision: &str,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<HistoryPage, GitError> {
        let oid = validate_oid(revision)?;
        let commits =
            self.history_from_revision(repository_root, &oid, limit, offset, cancellation)?;
        Ok(HistoryPage {
            reference: oid.clone(),
            oid,
            commits,
        })
    }

    fn history_from_revision(
        &self,
        repository_root: &Path,
        revision: &str,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CommitDetails>, GitError> {
        let arguments = vec![
            OsString::from("log"),
            OsString::from(revision),
            OsString::from("--date-order"),
            OsString::from("--decorate=full"),
            OsString::from("-z"),
            OsString::from(format!("--format={LOG_FORMAT}")),
            OsString::from(format!("--max-count={limit}")),
            OsString::from(format!("--skip={offset}")),
            OsString::from("--"),
        ];
        let output = self.run_git_os_read_only(
            "git-history-ref",
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        if output.status.success() {
            parse_log(&output.stdout)
        } else if is_empty_history_error(&output.stderr) {
            Ok(Vec::new())
        } else {
            Err(command_failed(&output))
        }
    }

    fn resolve_ref_oid(
        &self,
        repository_root: &Path,
        reference: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, GitError> {
        let revision = OsString::from(format!("{reference}^{{commit}}"));
        let output = self.run_git_os_read_only(
            "git-resolve-history-ref",
            repository_root,
            vec![
                OsString::from("rev-parse"),
                OsString::from("--verify"),
                OsString::from("--quiet"),
                revision,
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        if !output.status.success() {
            return Err(GitError::ReferenceNotFound {
                value: reference.to_owned(),
            });
        }
        decode_trimmed_stdout(&output, "git rev-parse de referencia")
    }

    /// Carga un commit concreto por su hash validado.
    pub fn commit_details(
        &self,
        repository_root: &Path,
        commit_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<CommitDetails, GitError> {
        if !(4..=64).contains(&commit_id.len())
            || !commit_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GitError::InvalidReferenceName {
                value: commit_id.to_owned(),
            });
        }
        let output = self.run_git_os_read_only(
            "git-commit-details",
            repository_root,
            vec![
                OsString::from("show"),
                OsString::from("--no-patch"),
                OsString::from("--decorate=full"),
                OsString::from("-z"),
                OsString::from(format!("--format={LOG_FORMAT}")),
                OsString::from(commit_id),
                OsString::from("--"),
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        parse_log(&require_success(output)?.stdout)?
            .into_iter()
            .next()
            .ok_or_else(|| GitError::InvalidLog {
                message: format!("Git no devolvió detalles para {commit_id}"),
            })
    }

    /// Reúne exclusivamente metadatos y contenido staged para Cursor.
    pub fn staged_context(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<StagedContextData, GitError> {
        // Estas lecturas no forman una transacción. Comparar la identidad
        // antes y después evita construir un prompt mezclando dos índices.
        for _attempt in 0..2 {
            let initial_identity = self.staged_identity(repository_root, cancellation)?;
            let name_status = self.run_checked_text(
                "git-staged-name-status",
                repository_root,
                ["diff", "--cached", "--name-status", "-z"],
                cancellation,
            )?;
            let numstat = self.run_checked_text(
                "git-staged-numstat",
                repository_root,
                ["diff", "--cached", "--numstat", "-z"],
                cancellation,
            )?;
            let textual_diff = self.run_checked_text(
                "git-staged-diff",
                repository_root,
                ["diff", "--cached", "--no-ext-diff", "--no-textconv", "--"],
                cancellation,
            )?;
            let recent_subjects = if self.has_head(repository_root, cancellation)? {
                self.run_checked_text(
                    "git-recent-subjects",
                    repository_root,
                    ["log", "--max-count=20", "--format=%s"],
                    cancellation,
                )?
                .lines()
                .map(ToOwned::to_owned)
                .collect()
            } else {
                Vec::new()
            };
            let final_identity = self.staged_identity(repository_root, cancellation)?;
            if initial_identity == final_identity {
                return Ok(StagedContextData {
                    name_status,
                    numstat,
                    textual_diff,
                    recent_subjects,
                    index_identity: final_identity,
                });
            }
        }

        Err(GitError::StagedStateChanged)
    }

    /// Devuelve una identidad machine-readable del contenido staged.
    pub fn staged_identity(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, GitError> {
        let output = self.run_git_read_only(
            "git-staged-identity",
            repository_root,
            [
                "diff",
                "--cached",
                "--raw",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--",
            ],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        Ok(require_success(output)?.stdout)
    }

    /// Indica si existe un commit inicial.
    pub fn has_head(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<bool, GitError> {
        let output = self.run_git_read_only(
            "git-has-head",
            repository_root,
            ["rev-parse", "--verify", "--quiet", "HEAD"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        Ok(output.status.success())
    }

    /// Añade una ruta concreta al índice.
    pub fn stage(
        &self,
        repository_root: &Path,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let path = validate_relative_path(path)?;
        self.run_path_command(
            "git-stage",
            repository_root,
            ["add", "--"],
            &path,
            cancellation,
        )
    }

    /// Añade al índice un conjunto concreto de rutas.
    ///
    /// Git no ofrece atomicidad entre rutas, así que el resultado se informa
    /// tal cual: si alguna falla se devuelve [`GitError::PartialBatch`] con lo
    /// que sí se aplicó y el motivo por ruta.
    pub fn stage_paths(
        &self,
        repository_root: &Path,
        paths: &[PathBuf],
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let paths = validate_pathspecs(paths)?;
        if paths.is_empty() {
            return Ok(());
        }
        self.run_pathspec_batches(
            "git-stage-batch",
            repository_root,
            &["add", "--"],
            &paths,
            cancellation,
        )
    }

    /// Quita del índice un conjunto concreto de rutas sin tocar el working tree.
    pub fn unstage_paths(
        &self,
        repository_root: &Path,
        paths: &[PathBuf],
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        // Validar antes de consultar HEAD: un pathspec inseguro debe abortar el
        // lote sin ejecutar ningún proceso.
        let paths = validate_pathspecs(paths)?;
        if paths.is_empty() {
            return Ok(());
        }
        if self.has_head(repository_root, cancellation)? {
            self.run_pathspec_batches(
                "git-unstage-batch",
                repository_root,
                &["restore", "--staged", "--"],
                &paths,
                cancellation,
            )
        } else {
            self.run_pathspec_batches(
                "git-unstage-batch-unborn",
                repository_root,
                &["rm", "--cached", "--"],
                &paths,
                cancellation,
            )
        }
    }

    /// Añade al índice todos los cambios visibles para Git.
    pub fn stage_all(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        self.run_checked_git(
            "git-stage-all",
            repository_root,
            ["add", "-A"],
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )
    }

    /// Quita una ruta del índice sin tocar el working tree.
    pub fn unstage(
        &self,
        repository_root: &Path,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let path = validate_relative_path(path)?;
        if self.has_head(repository_root, cancellation)? {
            self.run_path_command(
                "git-unstage",
                repository_root,
                ["restore", "--staged", "--"],
                &path,
                cancellation,
            )
        } else {
            self.run_path_command(
                "git-unstage-unborn",
                repository_root,
                ["rm", "--cached", "--"],
                &path,
                cancellation,
            )
        }
    }

    /// Vacía el índice sin borrar archivos del working tree.
    pub fn unstage_all(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        if self.has_head(repository_root, cancellation)? {
            self.run_checked_git(
                "git-unstage-all",
                repository_root,
                ["restore", "--staged", "."],
                None,
                LOCAL_OPERATION_TIMEOUT,
                cancellation,
            )
        } else {
            self.run_checked_git(
                "git-unstage-all-unborn",
                repository_root,
                ["rm", "-r", "--cached", "--", "."],
                None,
                LOCAL_OPERATION_TIMEOUT,
                cancellation,
            )
        }
    }

    /// Ejecuta un descarte que la UI ya presentó y confirmó.
    pub fn discard(
        &self,
        repository_root: &Path,
        plan: &DiscardPlan,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let path = validate_relative_path(&plan.path)?;
        match plan.mode {
            DiscardMode::Worktree => self.run_path_command(
                "git-discard-worktree",
                repository_root,
                ["restore", "--worktree", "--"],
                &path,
                cancellation,
            ),
            DiscardMode::StagedAndWorktree => self.run_path_command(
                "git-discard-staged",
                repository_root,
                ["restore", "--source=HEAD", "--staged", "--worktree", "--"],
                &path,
                cancellation,
            ),
            DiscardMode::UntrackedFile | DiscardMode::UntrackedDirectory => {
                validate_existing_path_inside_repository(repository_root, &path)?;
                let status = self.status(repository_root, cancellation)?;
                let remains_untracked = status
                    .changes
                    .iter()
                    .any(|change| change.path == path && change.is_untracked());
                if !remains_untracked {
                    return Err(GitError::UntrackedStateChanged { path });
                }
                let clean_flag = if plan.mode == DiscardMode::UntrackedDirectory {
                    "-fd"
                } else {
                    "-f"
                };
                self.run_path_command(
                    "git-discard-untracked",
                    repository_root,
                    ["clean", clean_flag, "--"],
                    &path,
                    cancellation,
                )
            }
        }
    }

    /// Crea un commit usando stdin para conservar texto y saltos de línea.
    pub fn commit(
        &self,
        repository_root: &Path,
        message: &str,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        if message.trim().is_empty() {
            return Err(GitError::EmptyCommitMessage);
        }
        self.run_checked_git(
            "git-commit",
            repository_root,
            ["commit", "--file=-"],
            Some(message.as_bytes().to_vec()),
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )
    }

    /// Ejecuta exclusivamente planes remotos creados por la capa tipada.
    pub fn execute_remote(
        &self,
        repository_root: &Path,
        plan: &RemoteOperationPlan,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let arguments = match plan {
            RemoteOperationPlan::Fetch { remote_name } => {
                vec!["fetch", "--", remote_name.as_str()]
            }
            RemoteOperationPlan::PullFastForward => vec!["pull", "--ff-only"],
            RemoteOperationPlan::Push => vec!["push"],
            RemoteOperationPlan::SetUpstreamAndPush {
                remote_name,
                branch_name,
            } => vec![
                "push",
                "--set-upstream",
                "--",
                remote_name.as_str(),
                branch_name.as_str(),
            ],
        };
        self.run_checked_git(
            "git-remote-operation",
            repository_root,
            arguments,
            None,
            REMOTE_OPERATION_TIMEOUT,
            cancellation,
        )
    }

    fn run_path_command<const N: usize>(
        &self,
        label: &'static str,
        repository_root: &Path,
        prefix: [&str; N],
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let mut arguments: Vec<OsString> = prefix.into_iter().map(OsString::from).collect();
        arguments.push(path.as_os_str().to_os_string());
        let output = self.run_git_os(
            label,
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        require_success(output).map(|_| ())
    }

    /// Ejecuta un comando de rutas repartido en tantas invocaciones como
    /// exija el límite de línea de comandos de Windows.
    ///
    /// Recibe rutas ya validadas por [`validate_pathspecs`]. Cuando Git rechaza
    /// un lote no dice qué ruta lo provocó, así que ese lote se repite ruta a
    /// ruta para poder atribuir cada fallo.
    fn run_pathspec_batches(
        &self,
        label: &'static str,
        repository_root: &Path,
        prefix: &[&str],
        paths: &[PathBuf],
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let mut reserved = command_line_cost(self.executable.as_os_str())
            + command_line_cost(OsStr::new("-C"))
            + command_line_cost(repository_root.as_os_str());
        for argument in prefix {
            reserved += command_line_cost(OsStr::new(argument));
        }

        let mut applied = 0;
        let mut failures = Vec::new();
        for batch in plan_pathspec_batches(paths, reserved) {
            match self.run_pathspec_batch(label, repository_root, prefix, &batch, cancellation) {
                Ok(()) => applied += batch.len(),
                // Solo un rechazo de Git puede atribuirse a una ruta concreta.
                // Una cancelación, un timeout o un fallo al lanzar el proceso
                // afectan al lote entero: repetirlo ruta a ruta multiplicaría la
                // espera —y el bloqueo del repositorio— sin aportar información.
                Err(error) if !matches!(error, GitError::CommandFailed { .. }) => {
                    return Err(error);
                }
                Err(_) => {
                    for path in batch {
                        match self.run_pathspec_batch(
                            label,
                            repository_root,
                            prefix,
                            std::slice::from_ref(&path),
                            cancellation,
                        ) {
                            Ok(()) => applied += 1,
                            Err(error) if !matches!(error, GitError::CommandFailed { .. }) => {
                                return Err(error);
                            }
                            Err(error) => failures.push(PathFailure {
                                reason: error.technical_details().trim().to_owned(),
                                path,
                            }),
                        }
                    }
                }
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(GitError::PartialBatch {
                applied,
                requested: paths.len(),
                failures,
            })
        }
    }

    fn run_pathspec_batch(
        &self,
        label: &'static str,
        repository_root: &Path,
        prefix: &[&str],
        paths: &[PathBuf],
        cancellation: &CancellationToken,
    ) -> Result<(), GitError> {
        let mut arguments: Vec<OsString> = prefix
            .iter()
            .copied()
            .map(OsString::from)
            .collect::<Vec<_>>();
        arguments.extend(paths.iter().map(|path| path.as_os_str().to_os_string()));
        let output = self.run_git_os(
            label,
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        require_success(output).map(|_| ())
    }

    fn run_checked_text<I, S>(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: I,
        cancellation: &CancellationToken,
    ) -> Result<String, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run_git_read_only(
            label,
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        let output = require_success(output)?;
        String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8 { context: label })
    }

    fn run_checked_git<I, S>(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: I,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<(), GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run_git(
            label,
            repository_root,
            arguments,
            stdin,
            timeout,
            cancellation,
        )?;
        require_success(output).map(|_| ())
    }

    fn run_git<I, S>(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: I,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_git_os(
            label,
            repository_root,
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string())
                .collect(),
            stdin,
            timeout,
            cancellation,
        )
    }

    fn run_git_read_only<I, S>(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: I,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_git_os_read_only(
            label,
            repository_root,
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_os_string())
                .collect(),
            stdin,
            timeout,
            cancellation,
        )
    }

    fn run_git_os(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: Vec<OsString>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, GitError> {
        let mut full_arguments = Vec::with_capacity(arguments.len() + 2);
        full_arguments.push(OsString::from("-C"));
        full_arguments.push(repository_root.as_os_str().to_os_string());
        full_arguments.extend(arguments);
        self.run_process(label, full_arguments, stdin, timeout, cancellation, false)
            .map_err(GitError::Process)
    }

    fn run_git_os_read_only(
        &self,
        label: &'static str,
        repository_root: &Path,
        arguments: Vec<OsString>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, GitError> {
        let mut full_arguments = Vec::with_capacity(arguments.len() + 2);
        full_arguments.push(OsString::from("-C"));
        full_arguments.push(repository_root.as_os_str().to_os_string());
        full_arguments.extend(arguments);
        self.run_process(label, full_arguments, stdin, timeout, cancellation, true)
            .map_err(GitError::Process)
    }

    fn run_process(
        &self,
        label: &'static str,
        arguments: Vec<OsString>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
        read_only: bool,
    ) -> Result<ProcessOutput, ProcessError> {
        let mut environment = vec![(OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0"))];
        if read_only {
            environment.push((OsString::from("GIT_OPTIONAL_LOCKS"), OsString::from("0")));
        }
        self.runner.run(
            ProcessRequest {
                label,
                program: self.executable.clone(),
                arguments,
                environment,
                removed_environment: Vec::new(),
                current_directory: None,
                stdin,
                timeout,
            },
            cancellation,
        )
    }
}

fn require_success(output: ProcessOutput) -> Result<ProcessOutput, GitError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_failed(&output))
    }
}

fn command_failed(output: &ProcessOutput) -> GitError {
    GitError::CommandFailed {
        exit_code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

fn is_empty_history_error(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    stderr.contains("does not have any commits")
        || stderr.contains("no commits yet")
        || stderr.contains("bad default revision")
}

fn decode_trimmed_stdout(
    output: &ProcessOutput,
    context: &'static str,
) -> Result<String, GitError> {
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| GitError::InvalidUtf8 { context })?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn history_target(head: &HeadState) -> Option<(String, String)> {
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

fn mark_active_branch(
    mut branches: Vec<BranchReference>,
    head: &HeadState,
) -> Vec<BranchReference> {
    let HeadState::Branch { name, oid } = head else {
        return branches;
    };
    let active_full_name = format!("refs/heads/{name}");
    if let Some(branch) = branches
        .iter_mut()
        .find(|branch| branch.full_name == active_full_name)
    {
        branch.is_active = true;
    } else {
        // Una rama unborn aún no existe bajo refs/heads, pero sigue siendo útil en el inventario.
        branches.push(BranchReference {
            name: name.clone(),
            full_name: active_full_name,
            oid: oid.clone(),
            kind: BranchKind::Local,
            is_active: true,
            upstream: BranchUpstream::NoUpstream,
        });
    }
    branches
}

fn validate_history_ref(reference: &str) -> Result<(), GitError> {
    if !(reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/"))
        || reference.ends_with('/')
        || reference.contains('\0')
        || reference.contains("..")
        || reference.contains("@{")
        || reference.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(GitError::InvalidReferenceName {
            value: reference.to_owned(),
        });
    }
    Ok(())
}

fn validate_oid(oid: &str) -> Result<String, GitError> {
    if !(4..=64).contains(&oid.len()) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::InvalidReferenceName {
            value: oid.to_owned(),
        });
    }
    Ok(oid.to_owned())
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
        process::ExitStatus,
        sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use crate::process::{
        CancellationToken, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner,
    };

    use super::GitClient;

    #[derive(Default)]
    struct RecordingRunner {
        requests: Mutex<Vec<ProcessRequest>>,
    }

    impl RecordingRunner {
        fn requests(&self) -> Vec<ProcessRequest> {
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(
            &self,
            request: ProcessRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProcessOutput, ProcessError> {
            let stdout = match request.label {
                "git-status" => {
                    b"# branch.oid 0123456789012345678901234567890123456789\0# branch.head main\0"
                        .to_vec()
                }
                "git-remotes" => b"origin\n".to_vec(),
                "git-branches" => concat!(
                    "refs/heads/main",
                    "\0",
                    "0123456789012345678901234567890123456789",
                    "\0\0\0\0\n"
                )
                .as_bytes()
                .to_vec(),
                _ => Vec::new(),
            };
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            Ok(ProcessOutput {
                status: success_status(),
                stdout,
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn snapshot_omits_history_and_reuses_cached_remotes() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let cancellation = CancellationToken::default();

        let first = client
            .snapshot(Path::new("repo"), &cancellation)
            .expect("debe crear el primer snapshot");
        let second = client
            .snapshot(Path::new("repo"), &cancellation)
            .expect("debe reutilizar la caché");
        let requests = runner.requests();

        assert!(first.commits.is_empty());
        assert!(second.commits.is_empty());
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.label == "git-status")
                .count(),
            2
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.label == "git-remotes")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.label == "git-branches")
                .count(),
            1
        );
        assert!(
            requests
                .iter()
                .all(|request| request.label != "git-history")
        );
    }

    #[test]
    fn invalidating_remotes_forces_a_reload() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let cancellation = CancellationToken::default();
        let root = Path::new("repo");

        client.remotes(root, &cancellation).unwrap();
        client.remotes(root, &cancellation).unwrap();
        client.invalidate_remotes(root);
        client.remotes(root, &cancellation).unwrap();

        assert_eq!(
            runner
                .requests()
                .iter()
                .filter(|request| request.label == "git-remotes")
                .count(),
            2
        );
    }

    #[test]
    fn history_snapshot_requests_each_independent_git_read_once() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());

        client
            .snapshot_with_history(Path::new("repo"), 200, &CancellationToken::default())
            .unwrap();
        let requests = runner.requests();

        for label in [
            "git-status",
            "git-remotes",
            "git-branches",
            "git-history-ref",
        ] {
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request.label == label)
                    .count(),
                1,
                "{label} debe ejecutarse exactamente una vez"
            );
        }
        assert!(
            requests
                .iter()
                .all(|request| request.label != "git-has-head")
        );
    }

    #[test]
    fn optional_locks_are_disabled_only_for_read_only_commands() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let cancellation = CancellationToken::default();

        client.status(Path::new("repo"), &cancellation).unwrap();
        client.stage_all(Path::new("repo"), &cancellation).unwrap();
        let requests = runner.requests();
        let status = requests
            .iter()
            .find(|request| request.label == "git-status")
            .unwrap();
        let stage = requests
            .iter()
            .find(|request| request.label == "git-stage-all")
            .unwrap();

        assert!(has_environment(status, "GIT_OPTIONAL_LOCKS", "0"));
        assert!(!has_environment(stage, "GIT_OPTIONAL_LOCKS", "0"));
    }

    /// Reproduce un Git que rechaza exactamente una ruta del lote.
    struct RejectingRunner {
        rejected: &'static str,
        requests: Mutex<Vec<ProcessRequest>>,
    }

    impl ProcessRunner for RejectingRunner {
        fn run(
            &self,
            request: ProcessRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProcessOutput, ProcessError> {
            let rejects = request
                .arguments
                .iter()
                .any(|argument| argument == OsStr::new(self.rejected));
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            if rejects {
                return Ok(ProcessOutput {
                    status: failure_status(),
                    stdout: Vec::new(),
                    stderr: b"error: pathspec did not match any files\n".to_vec(),
                });
            }
            Ok(ProcessOutput {
                status: success_status(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    fn pathspecs(request: &ProcessRequest) -> Vec<String> {
        let separator = request
            .arguments
            .iter()
            .position(|argument| argument == OsStr::new("--"))
            .expect("todo comando de rutas separa los pathspecs con --");
        request.arguments[separator + 1..]
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn batches_stay_within_the_windows_command_line_and_keep_every_path() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let paths: Vec<PathBuf> = (0..2_000)
            .map(|index| PathBuf::from(format!("directorio con ñ/archivo-{index:04}.txt")))
            .collect();

        client
            .stage_paths(Path::new("repo"), &paths, &CancellationToken::default())
            .expect("un lote grande debe aplicarse por completo");

        let requests = runner.requests();
        assert!(
            requests.len() > 1,
            "2000 rutas no caben en una sola invocación"
        );
        let sent: Vec<String> = requests.iter().flat_map(pathspecs).collect();
        let expected: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(sent, expected);
        for request in &requests {
            let width: usize = request
                .arguments
                .iter()
                .map(|argument| super::command_line_cost(argument))
                .sum();
            assert!(width <= 30_000, "cada invocación cabe en CreateProcessW");
        }
    }

    #[test]
    fn a_rejected_path_is_reported_alone_and_the_rest_is_applied() {
        let runner = Arc::new(RejectingRunner {
            rejected: "falla.txt",
            requests: Mutex::new(Vec::new()),
        });
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let paths = vec![
            PathBuf::from("ok-1.txt"),
            PathBuf::from("falla.txt"),
            PathBuf::from("ok-2.txt"),
        ];

        let error = client
            .stage_paths(Path::new("repo"), &paths, &CancellationToken::default())
            .expect_err("el lote debe informar del fallo parcial");

        match error {
            super::GitError::PartialBatch {
                applied,
                requested,
                failures,
            } => {
                assert_eq!(applied, 2);
                assert_eq!(requested, 3);
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].path, PathBuf::from("falla.txt"));
                assert!(failures[0].reason.contains("pathspec did not match"));
            }
            other => panic!("se esperaba un resultado parcial, no {other:?}"),
        }

        // El lote falla una vez y después se reintenta ruta a ruta.
        let requests = runner
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let sent: Vec<Vec<String>> = requests.iter().map(pathspecs).collect();
        assert_eq!(
            sent,
            vec![
                vec![
                    "ok-1.txt".to_owned(),
                    "falla.txt".to_owned(),
                    "ok-2.txt".to_owned()
                ],
                vec!["ok-1.txt".to_owned()],
                vec!["falla.txt".to_owned()],
                vec!["ok-2.txt".to_owned()],
            ]
        );
    }

    #[test]
    fn an_unsafe_pathspec_aborts_the_batch_without_touching_the_repository() {
        let paths = vec![PathBuf::from("ok.txt"), PathBuf::from("../secreto.txt")];

        // Incluido unstage, que además necesita consultar HEAD: la validación
        // va primero y no puede llegar a lanzarse ningún proceso.
        for unstage in [false, true] {
            let runner = Arc::new(RecordingRunner::default());
            let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
            let cancellation = CancellationToken::default();

            let error = if unstage {
                client.unstage_paths(Path::new("repo"), &paths, &cancellation)
            } else {
                client.stage_paths(Path::new("repo"), &paths, &cancellation)
            }
            .expect_err("una ruta insegura invalida el lote entero");

            assert!(matches!(error, super::GitError::UnsafePath { .. }));
            assert!(
                runner.requests().is_empty(),
                "unstage={unstage} no debe ejecutar ningún proceso"
            );
        }
    }

    /// Reproduce un Git cuyo proceso nunca llega a emitir un veredicto por ruta.
    struct TimingOutRunner {
        attempts: AtomicUsize,
    }

    impl ProcessRunner for TimingOutRunner {
        fn run(
            &self,
            _request: ProcessRequest,
            _cancellation: &CancellationToken,
        ) -> Result<ProcessOutput, ProcessError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(ProcessError::TimedOut(Duration::from_secs(30)))
        }
    }

    #[test]
    fn a_process_failure_aborts_the_batch_instead_of_retrying_every_path() {
        let runner = Arc::new(TimingOutRunner {
            attempts: AtomicUsize::new(0),
        });
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let paths: Vec<PathBuf> = (0..200)
            .map(|index| PathBuf::from(format!("archivo-{index}.txt")))
            .collect();

        let error = client
            .stage_paths(Path::new("repo"), &paths, &CancellationToken::default())
            .expect_err("un timeout debe propagarse");

        assert!(matches!(
            error,
            super::GitError::Process(ProcessError::TimedOut(_))
        ));
        // Un timeout no es atribuible a una ruta: reintentar las 200 una a una
        // multiplicaría la espera y mantendría el repositorio bloqueado.
        assert_eq!(runner.attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn duplicate_rows_of_the_same_path_are_sent_once() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("a.txt")];

        client
            .stage_paths(Path::new("repo"), &paths, &CancellationToken::default())
            .expect("stage debe funcionar");

        assert_eq!(pathspecs(&runner.requests()[0]), vec!["a.txt".to_owned()]);
    }

    #[test]
    fn an_empty_selection_does_not_run_git() {
        let runner = Arc::new(RecordingRunner::default());
        let client = GitClient::with_runner(PathBuf::from("git"), runner.clone());

        client
            .stage_paths(Path::new("repo"), &[], &CancellationToken::default())
            .expect("una selección vacía es un no-op");

        assert!(runner.requests().is_empty());
    }

    fn has_environment(request: &ProcessRequest, name: &str, value: &str) -> bool {
        request
            .environment
            .iter()
            .any(|(key, candidate)| key == OsStr::new(name) && candidate == OsStr::new(value))
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

    #[cfg(windows)]
    fn failure_status() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(1)
    }

    #[cfg(unix)]
    fn failure_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(256)
    }
}
