use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{
    domain::{CommitDetails, Remote, RemoteOperationPlan, RepositorySnapshot},
    process::{
        CancellationToken, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner,
        SystemProcessRunner,
    },
};

use super::{
    DiscardMode, DiscardPlan, GitError, LOG_FORMAT, StagedContextData, parse_log, parse_status,
    resolve_upstream, validate_existing_path_inside_repository, validate_relative_path,
};

const LOCAL_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_OPERATION_TIMEOUT: Duration = Duration::from_mins(15);

/// Única puerta de entrada a `git.exe`.
#[derive(Clone)]
pub struct GitClient {
    executable: PathBuf,
    runner: Arc<dyn ProcessRunner>,
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
        }
    }

    /// Crea un cliente con ejecución inyectada para pruebas.
    #[must_use]
    pub fn with_runner(executable: PathBuf, runner: Arc<dyn ProcessRunner>) -> Self {
        Self { executable, runner }
    }

    /// Comprueba que Git está disponible y devuelve su versión.
    pub fn detect_version(&self, cancellation: &CancellationToken) -> Result<String, GitError> {
        let output = self
            .run_process(
                "git-version",
                vec![OsString::from("--version")],
                None,
                None,
                LOCAL_OPERATION_TIMEOUT,
                cancellation,
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
        let output = self.run_git(
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
        let output = self.run_git(
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

    /// Lee status, remotes e historial como un snapshot coherente para la UI.
    pub fn snapshot(
        &self,
        repository_root: &Path,
        history_limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<RepositorySnapshot, GitError> {
        let status = self.status(repository_root, cancellation)?;
        let remotes = self.remotes(repository_root, cancellation)?;
        let upstream = resolve_upstream(&status, &remotes);
        let commits = self.history(repository_root, history_limit + 1, 0, cancellation)?;
        let has_more_commits = commits.len() > history_limit;
        let commits = commits
            .into_iter()
            .take(history_limit)
            .map(|commit| commit.summary)
            .collect();

        Ok(RepositorySnapshot {
            head: status.head,
            upstream,
            remotes,
            changes: status.changes,
            commits,
            has_more_commits,
        })
    }

    /// Lee el estado machine-readable del repositorio.
    pub fn status(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<crate::domain::StatusSnapshot, GitError> {
        let output = self.run_git(
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
        let output = self.run_git(
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
        Ok(text
            .lines()
            .map(str::trim_end)
            .filter(|name| !name.is_empty())
            .map(|name| Remote {
                name: name.to_owned(),
            })
            .collect())
    }

    /// Carga una página de historial, incluyendo detalles para la selección.
    pub fn history(
        &self,
        repository_root: &Path,
        limit: usize,
        offset: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<CommitDetails>, GitError> {
        if !self.has_head(repository_root, cancellation)? {
            return Ok(Vec::new());
        }
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
        let output = self.run_git_os(
            "git-history",
            repository_root,
            arguments,
            None,
            LOCAL_OPERATION_TIMEOUT,
            cancellation,
        )?;
        parse_log(&require_success(output)?.stdout)
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
        let output = self.run_git_os(
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

        Ok(StagedContextData {
            name_status,
            numstat,
            textual_diff,
            recent_subjects,
        })
    }

    /// Indica si existe un commit inicial.
    pub fn has_head(
        &self,
        repository_root: &Path,
        cancellation: &CancellationToken,
    ) -> Result<bool, GitError> {
        let output = self.run_git(
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
        let output = self.run_git(
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
        self.run_process(label, full_arguments, None, stdin, timeout, cancellation)
            .map_err(GitError::Process)
    }

    fn run_process(
        &self,
        label: &'static str,
        arguments: Vec<OsString>,
        current_directory: Option<PathBuf>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        self.runner.run(
            ProcessRequest {
                label,
                program: self.executable.clone(),
                arguments,
                environment: vec![(OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0"))],
                removed_environment: Vec::new(),
                current_directory,
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
        Err(GitError::CommandFailed {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn decode_trimmed_stdout(
    output: &ProcessOutput,
    context: &'static str,
) -> Result<String, GitError> {
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| GitError::InvalidUtf8 { context })?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}
