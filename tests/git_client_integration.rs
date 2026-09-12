use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use git_helper::{
    domain::{BranchUpstream, ChangeKind, HeadState},
    git::{
        GitClient, classify_remote_failure, parse_ssh_url, plan_clone_destination, plan_discard,
        plan_pull, plan_push,
    },
    process::CancellationToken,
};
use tempfile::tempdir;

fn run_git(repository: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("Git for Windows debe estar instalado para las pruebas de integración")
}

fn require_git(repository: &Path, arguments: &[&str]) {
    let output = run_git(repository, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} falló: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn initialize_repository(repository: &Path) {
    require_git(repository, &["init", "-b", "main"]);
    require_git(repository, &["config", "core.autocrlf", "false"]);
    require_git(repository, &["config", "user.name", "Git Helper Tests"]);
    require_git(
        repository,
        &["config", "user.email", "git-helper-tests@example.invalid"],
    );
}

fn commit_file(
    client: &GitClient,
    repository: &Path,
    file_name: &str,
    content: &str,
    message: &str,
) {
    fs::write(repository.join(file_name), content).expect("debe escribir el archivo");
    let cancellation = CancellationToken::default();
    client
        .stage_all(repository, &cancellation)
        .expect("stage debe funcionar");
    client
        .commit(repository, message, &cancellation)
        .expect("commit debe funcionar");
}

#[test]
fn handles_unborn_unstage_without_removing_worktree_file() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    let file_path = temporary.path().join("archivo con ñ.txt");
    fs::write(&file_path, "contenido").expect("debe crear el archivo");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    client
        .stage(
            temporary.path(),
            Path::new("archivo con ñ.txt"),
            &cancellation,
        )
        .expect("stage debe funcionar");
    client
        .unstage(
            temporary.path(),
            Path::new("archivo con ñ.txt"),
            &cancellation,
        )
        .expect("unstage unborn debe funcionar");
    let status = client
        .status(temporary.path(), &cancellation)
        .expect("debe leer status");
    let history = client
        .history(temporary.path(), 200, 0, &cancellation)
        .expect("un repositorio unborn debe tener historial vacío");

    assert!(file_path.exists());
    assert!(history.is_empty());
    assert_eq!(status.changes[0].index_status, ChangeKind::Unmodified);
    assert_eq!(status.changes[0].worktree_status, ChangeKind::Untracked);
}

#[test]
fn stages_commits_and_preserves_dual_index_worktree_state() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    let file_path = temporary.path().join("tracked.txt");
    fs::write(&file_path, "inicial\n").expect("debe crear el archivo");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    client
        .stage_all(temporary.path(), &cancellation)
        .expect("stage all debe funcionar");
    client
        .commit(temporary.path(), "test: crea commit inicial", &cancellation)
        .expect("commit debe funcionar");
    fs::write(&file_path, "staged\n").expect("debe modificar el archivo");
    client
        .stage(temporary.path(), Path::new("tracked.txt"), &cancellation)
        .expect("stage debe funcionar");
    fs::write(&file_path, "staged\ny worktree\n").expect("debe modificarlo otra vez");

    let status = client
        .status(temporary.path(), &cancellation)
        .expect("debe leer status");
    let change = status
        .changes
        .iter()
        .find(|change| change.path == Path::new("tracked.txt"))
        .expect("debe contener tracked.txt");

    assert_eq!(change.index_status, ChangeKind::Modified);
    assert_eq!(change.worktree_status, ChangeKind::Modified);
    assert!(change.has_staged_change());
    assert!(change.has_worktree_change());

    let history = client
        .history(temporary.path(), 200, 0, &cancellation)
        .expect("debe leer historial");
    assert_eq!(history[0].summary.subject, "test: crea commit inicial");
}

#[test]
fn staged_identity_tracks_index_content_but_ignores_unstaged_only_changes() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    let file_path = temporary.path().join("tracked.txt");
    fs::write(&file_path, "inicial\n").expect("debe crear el archivo");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    client
        .stage_all(temporary.path(), &cancellation)
        .expect("stage inicial debe funcionar");
    client
        .commit(temporary.path(), "test: base", &cancellation)
        .expect("commit inicial debe funcionar");

    fs::write(&file_path, "staged-a\n").expect("debe modificar el archivo");
    client
        .stage(temporary.path(), Path::new("tracked.txt"), &cancellation)
        .expect("stage a debe funcionar");
    let identity_a = client
        .staged_identity(temporary.path(), &cancellation)
        .expect("debe leer la identidad staged");

    fs::write(&file_path, "staged-b\n").expect("debe cambiar el contenido staged");
    client
        .stage(temporary.path(), Path::new("tracked.txt"), &cancellation)
        .expect("stage b debe funcionar");
    let identity_b = client
        .staged_identity(temporary.path(), &cancellation)
        .expect("debe releer la identidad staged");
    assert_ne!(identity_a, identity_b);

    fs::write(&file_path, "staged-b\nworktree-only\n")
        .expect("debe crear un cambio solo en el working tree");
    let identity_after_unstaged_change = client
        .staged_identity(temporary.path(), &cancellation)
        .expect("debe conservar la identidad del index");
    assert_eq!(identity_b, identity_after_unstaged_change);
}

#[test]
fn discards_tracked_and_revalidates_untracked_files() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    let tracked_path = temporary.path().join("tracked.txt");
    fs::write(&tracked_path, "inicial\n").expect("debe crear tracked");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    client
        .stage_all(temporary.path(), &cancellation)
        .expect("stage debe funcionar");
    client
        .commit(temporary.path(), "test: base", &cancellation)
        .expect("commit debe funcionar");

    fs::write(&tracked_path, "modificado\n").expect("debe modificar tracked");
    let status = client
        .status(temporary.path(), &cancellation)
        .expect("debe leer status");
    let tracked_change = status
        .changes
        .iter()
        .find(|change| change.path == Path::new("tracked.txt"))
        .expect("debe contener tracked");
    let tracked_plan =
        plan_discard(tracked_change, false, true, false).expect("debe crear plan tracked");
    client
        .discard(temporary.path(), &tracked_plan, &cancellation)
        .expect("debe descartar tracked");
    assert_eq!(
        fs::read_to_string(&tracked_path).expect("debe leer tracked"),
        "inicial\n"
    );

    let untracked_path = temporary.path().join("nuevo.txt");
    fs::write(&untracked_path, "temporal").expect("debe crear untracked");
    let status = client
        .status(temporary.path(), &cancellation)
        .expect("debe leer status");
    let untracked_change = status
        .changes
        .iter()
        .find(|change| change.path == Path::new("nuevo.txt"))
        .expect("debe contener untracked");
    let untracked_plan =
        plan_discard(untracked_change, false, true, false).expect("debe crear plan untracked");
    client
        .discard(temporary.path(), &untracked_plan, &cancellation)
        .expect("debe limpiar untracked");

    assert!(!untracked_path.exists());
}

#[test]
fn pushes_first_branch_pulls_fast_forward_and_rejects_divergence() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    let bare = temporary.path().join("remote bare");
    let first = temporary.path().join("primer repositorio");
    let second = temporary.path().join("segundo repositorio");
    fs::create_dir_all(&bare).expect("debe crear remote");
    fs::create_dir_all(&first).expect("debe crear primer repo");
    require_git(&bare, &["init", "--bare"]);
    require_git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    initialize_repository(&first);
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    commit_file(&client, &first, "base.txt", "base\n", "test: base");
    require_git(
        &first,
        &["remote", "add", "origin", &bare.display().to_string()],
    );

    let first_snapshot = client
        .snapshot(&first, &cancellation)
        .expect("debe leer snapshot");
    let first_push = plan_push(
        &first_snapshot.head,
        first_snapshot.upstream.as_ref(),
        &first_snapshot.remotes,
        Some("origin"),
    )
    .expect("debe planificar primera publicación");
    client
        .execute_remote(&first, &first_push, &cancellation)
        .expect("debe publicar y configurar upstream");

    let clone_output = Command::new("git")
        .args([
            "clone",
            &bare.display().to_string(),
            &second.display().to_string(),
        ])
        .output()
        .expect("debe ejecutar clone para preparar el fixture");
    assert!(
        clone_output.status.success(),
        "clone falló: {}",
        String::from_utf8_lossy(&clone_output.stderr)
    );
    require_git(&second, &["config", "core.autocrlf", "false"]);
    require_git(&second, &["config", "user.name", "Git Helper Tests"]);
    require_git(
        &second,
        &["config", "user.email", "git-helper-tests@example.invalid"],
    );
    commit_file(
        &client,
        &second,
        "remote.txt",
        "remote\n",
        "test: cambio remoto",
    );
    require_git(&second, &["push"]);

    let first_snapshot = client
        .snapshot(&first, &cancellation)
        .expect("debe refrescar snapshot");
    let pull = plan_pull(first_snapshot.upstream.as_ref()).expect("debe planificar pull");
    client
        .execute_remote(&first, &pull, &cancellation)
        .expect("pull fast-forward debe funcionar");
    assert!(first.join("remote.txt").exists());

    commit_file(
        &client,
        &first,
        "local.txt",
        "local\n",
        "test: cambio local",
    );
    commit_file(
        &client,
        &second,
        "otro-remoto.txt",
        "otro\n",
        "test: otro cambio remoto",
    );
    require_git(&second, &["push"]);
    let head_before = run_git(&first, &["rev-parse", "HEAD"]).stdout;
    let pull_error = client
        .execute_remote(&first, &pull, &cancellation)
        .expect_err("pull divergente debe rechazarse");
    let head_after = run_git(&first, &["rev-parse", "HEAD"]).stdout;

    assert!(
        matches!(
            pull_error,
            git_helper::git::GitError::DivergentBranches { .. }
        ),
        "un pull divergente debe clasificarse como divergencia: {pull_error:?}"
    );
    assert_eq!(head_before, head_after);
}

#[test]
#[allow(clippy::too_many_lines)]
fn enumerates_branches_and_reads_selected_history_without_checkout() {
    let temporary = tempdir().expect("debe crear el temporal");
    let repository = temporary.path().join("repositorio principal");
    fs::create_dir_all(&repository).expect("debe crear el repositorio");
    initialize_repository(&repository);
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    commit_file(&client, &repository, "base.txt", "base\n", "test: base");
    require_git(&repository, &["branch", "feature/ñ/rama"]);
    require_git(&repository, &["branch", "sin-upstream"]);
    require_git(&repository, &["branch", "upstream-eliminado"]);

    let origin = temporary.path().join("origin.git");
    fs::create_dir_all(&origin).expect("debe crear origin");
    require_git(&origin, &["init", "--bare"]);
    require_git(
        &repository,
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    require_git(&repository, &["config", "branch.main.remote", "origin"]);
    require_git(
        &repository,
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    require_git(
        &repository,
        &["config", "branch.upstream-eliminado.remote", "origin"],
    );
    require_git(
        &repository,
        &[
            "config",
            "branch.upstream-eliminado.merge",
            "refs/heads/eliminada",
        ],
    );
    let head_oid = String::from_utf8(run_git(&repository, &["rev-parse", "HEAD"]).stdout)
        .expect("OID UTF-8")
        .trim()
        .to_owned();
    require_git(
        &repository,
        &["update-ref", "refs/remotes/origin/main", &head_oid],
    );
    require_git(
        &repository,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );

    let tree_before = run_git(&repository, &["write-tree"]).stdout;
    let head_before = run_git(&repository, &["rev-parse", "HEAD"]).stdout;
    let snapshot = client
        .snapshot(&repository, &cancellation)
        .expect("debe enumerar ramas");

    let main = snapshot
        .branches
        .iter()
        .find(|branch| branch.name == "main")
        .expect("debe aparecer la rama actual");
    assert!(main.is_active);
    assert!(matches!(
        main.upstream,
        git_helper::domain::BranchUpstream::Configured {
            ahead: 0,
            behind: 0,
            ..
        }
    ));
    assert!(
        snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "feature/ñ/rama")
    );
    assert!(matches!(
        snapshot
            .branches
            .iter()
            .find(|branch| branch.name == "sin-upstream")
            .expect("debe listar ramas locales sin upstream")
            .upstream,
        BranchUpstream::NoUpstream
    ));
    assert!(matches!(
        snapshot
            .branches
            .iter()
            .find(|branch| branch.name == "upstream-eliminado")
            .expect("debe conservar el upstream configurado")
            .upstream,
        git_helper::domain::BranchUpstream::Gone { .. }
    ));
    assert!(
        !snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "origin/HEAD")
    );

    let history = client
        .history_for_ref(
            &repository,
            "refs/heads/feature/ñ/rama",
            20,
            0,
            &cancellation,
        )
        .expect("debe leer historial de la rama sin checkout");
    assert_eq!(history.oid, head_oid);
    assert_eq!(history.commits[0].summary.subject, "test: base");
    assert_eq!(run_git(&repository, &["write-tree"]).stdout, tree_before);
    assert_eq!(
        run_git(&repository, &["rev-parse", "HEAD"]).stdout,
        head_before
    );

    require_git(&repository, &["branch", "externa"]);
    client.invalidate_branches(&repository);
    let refreshed = client
        .branches(&repository, &cancellation)
        .expect("debe invalidar la caché de referencias");
    assert!(refreshed.iter().any(|branch| branch.name == "externa"));
}

#[test]
fn unborn_and_detached_heads_keep_branch_inventory_consistent() {
    let temporary = tempdir().expect("debe crear el temporal");
    initialize_repository(temporary.path());
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    let unborn = client
        .branches(temporary.path(), &cancellation)
        .expect("debe listar la rama activa en un repositorio unborn");
    let main = unborn
        .iter()
        .find(|branch| branch.name == "main")
        .expect("debe exponer main como rama activa sin commits");
    assert!(main.is_active);
    assert!(main.oid.is_none());
    assert_eq!(main.upstream, BranchUpstream::NoUpstream);

    commit_file(
        &client,
        temporary.path(),
        "base.txt",
        "base\n",
        "test: base",
    );
    client.invalidate_branches(temporary.path());
    require_git(temporary.path(), &["checkout", "--detach", "HEAD"]);
    let detached_status = client
        .status(temporary.path(), &cancellation)
        .expect("debe leer HEAD separado");
    assert!(matches!(detached_status.head, HeadState::Detached { .. }));

    let detached = client
        .branches(temporary.path(), &cancellation)
        .expect("debe conservar el inventario con HEAD separado");
    assert!(detached.iter().all(|branch| !branch.is_active));
    assert!(detached.iter().any(|branch| branch.name == "main"));
}

#[test]
fn lists_remote_tracking_refs_from_multiple_remotes() {
    let temporary = tempdir().expect("debe crear el temporal");
    initialize_repository(temporary.path());
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    commit_file(
        &client,
        temporary.path(),
        "base.txt",
        "base\n",
        "test: base",
    );
    let head_oid = String::from_utf8(run_git(temporary.path(), &["rev-parse", "HEAD"]).stdout)
        .expect("OID UTF-8")
        .trim()
        .to_owned();

    let origin = temporary.path().join("origin.git");
    let team = temporary.path().join("team.git");
    fs::create_dir_all(&origin).expect("debe crear origin");
    fs::create_dir_all(&team).expect("debe crear team");
    require_git(&origin, &["init", "--bare"]);
    require_git(&team, &["init", "--bare"]);
    require_git(
        temporary.path(),
        &["remote", "add", "origin", &origin.display().to_string()],
    );
    require_git(
        temporary.path(),
        &["remote", "add", "team", &team.display().to_string()],
    );
    require_git(
        temporary.path(),
        &["update-ref", "refs/remotes/origin/main", &head_oid],
    );
    require_git(
        temporary.path(),
        &["update-ref", "refs/remotes/team/main", &head_oid],
    );

    let snapshot = client
        .snapshot(temporary.path(), &cancellation)
        .expect("debe enumerar refs de varios remotes");
    assert_eq!(snapshot.remotes.len(), 2);
    assert!(
        snapshot
            .remotes
            .iter()
            .any(|remote| remote.name == "origin")
    );
    assert!(snapshot.remotes.iter().any(|remote| remote.name == "team"));
    assert!(
        snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "origin/main")
    );
    assert!(
        snapshot
            .branches
            .iter()
            .any(|branch| branch.name == "team/main")
    );
}

#[test]
fn pagination_stays_on_the_selected_oid_when_the_branch_moves() {
    let temporary = tempdir().expect("debe crear el temporal");
    initialize_repository(temporary.path());
    let client = GitClient::default();
    let cancellation = CancellationToken::default();

    commit_file(&client, temporary.path(), "one.txt", "1\n", "test: one");
    commit_file(&client, temporary.path(), "two.txt", "2\n", "test: two");
    commit_file(&client, temporary.path(), "three.txt", "3\n", "test: three");
    require_git(temporary.path(), &["branch", "selected"]);

    let first = client
        .history_for_ref(temporary.path(), "refs/heads/selected", 2, 0, &cancellation)
        .expect("debe cargar la primera página");
    assert_eq!(first.commits.len(), 2);

    commit_file(&client, temporary.path(), "four.txt", "4\n", "test: four");
    require_git(temporary.path(), &["branch", "-f", "selected", "HEAD"]);
    let second = client
        .history_for_oid(
            temporary.path(),
            "refs/heads/selected",
            &first.oid,
            2,
            2,
            &cancellation,
        )
        .expect("debe paginar desde el OID original");
    assert_eq!(second.oid, first.oid);
    assert_eq!(second.commits.len(), 1);
    assert_eq!(second.commits[0].summary.subject, "test: one");

    let detached = client
        .history_for_oid(
            temporary.path(),
            &first.oid,
            &first.oid,
            1,
            0,
            &cancellation,
        )
        .expect("debe aceptar un OID fijado como referencia para HEAD separado");
    assert_eq!(detached.reference, first.oid);
    assert_eq!(detached.commits.len(), 1);

    let moved = client
        .history_for_ref(temporary.path(), "refs/heads/selected", 1, 0, &cancellation)
        .expect("debe resolver la rama movida");
    assert_ne!(moved.oid, first.oid);
    assert_eq!(moved.commits[0].summary.subject, "test: four");
}

#[test]
fn clones_from_a_local_bare_remote() {
    let temporary = tempdir().expect("debe crear el temporal");
    let bare = temporary.path().join("origin.git");
    Command::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .status()
        .expect("git init --bare debe funcionar");
    let clone_destination = temporary.path().join("working-copy");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    let remote_url = format!("file://{}", bare.display());

    client
        .clone_repository(&remote_url, &clone_destination, &cancellation)
        .expect("git clone debe funcionar contra un bare local");
    let origin = client
        .remote_url(&clone_destination, "origin", &cancellation)
        .expect("debe leer origin");

    assert!(origin.contains("origin.git"));
}

#[test]
fn reuses_destination_when_origin_matches_requested_ssh_url() {
    let temporary = tempdir().expect("debe crear el temporal");
    initialize_repository(temporary.path());
    require_git(
        temporary.path(),
        &["remote", "add", "origin", "git@github.com:org/demo.git"],
    );
    let parsed = parse_ssh_url("ssh://git@github.com/org/demo").expect("url ssh de prueba");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    let plan = plan_clone_destination(&client, &parsed, temporary.path(), &cancellation)
        .expect("debe reutilizar el clon existente");

    assert!(matches!(
        plan,
        git_helper::git::CloneDestinationPlan::OpenExisting(_)
    ));
}

#[test]
fn classifies_common_ssh_failures() {
    let auth = classify_remote_failure("Permission denied (publickey).", Some(128));
    assert!(matches!(
        auth,
        git_helper::git::GitError::SshAuthenticationFailed { .. }
    ));
    let host = classify_remote_failure("Host key verification failed.", Some(128));
    assert!(matches!(
        host,
        git_helper::git::GitError::SshHostKeyVerificationFailed { .. }
    ));
}

#[test]
fn failed_clone_leaves_no_partial_destination() {
    let temporary = tempdir().expect("debe crear el temporal");
    let missing_remote = temporary.path().join("no-existe.git");
    let clone_destination = temporary.path().join("working-copy");
    let client = GitClient::default();

    let error = client
        .clone_repository(
            &format!("file://{}", missing_remote.display()),
            &clone_destination,
            &CancellationToken::default(),
        )
        .expect_err("clonar un remoto inexistente debe fallar");

    assert!(!matches!(
        error,
        git_helper::git::GitError::InvalidSshUrl { .. }
    ));
    assert!(
        !clone_destination.exists(),
        "un clonado fallido no debe dejar el destino a medias"
    );
}

#[test]
fn rejects_clone_arguments_that_git_would_read_as_options() {
    let temporary = tempdir().expect("debe crear el temporal");
    let client = GitClient::default();

    let error = client
        .clone_repository(
            "--upload-pack=payload",
            &temporary.path().join("destino"),
            &CancellationToken::default(),
        )
        .expect_err("una URL con guion inicial debe rechazarse antes de ejecutar git");

    assert!(matches!(
        error,
        git_helper::git::GitError::InvalidSshUrl { .. }
    ));
}

#[test]
fn a_locked_index_is_reported_without_being_deleted() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    fs::write(temporary.path().join("a.txt"), "contenido").expect("debe escribir el archivo");
    let lock_path = temporary.path().join(".git").join("index.lock");
    fs::write(&lock_path, "").expect("debe simular el bloqueo");
    let client = GitClient::default();

    let error = client
        .stage(
            temporary.path(),
            Path::new("a.txt"),
            &CancellationToken::default(),
        )
        .expect_err("stage debe fallar con el índice bloqueado");

    assert!(
        matches!(error, git_helper::git::GitError::IndexLocked { .. }),
        "un índice bloqueado debe clasificarse como tal: {error:?}"
    );
    assert!(
        error
            .recommended_action()
            .expect("debe recomendar una acción")
            .contains("Git Helper no lo elimina")
    );
    assert!(
        lock_path.exists(),
        "Git Helper no debe borrar index.lock automáticamente"
    );
}

#[test]
fn a_rejected_hook_keeps_the_repository_intact_and_names_the_hook() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    initialize_repository(temporary.path());
    let hooks = temporary.path().join(".git").join("hooks");
    fs::create_dir_all(&hooks).expect("debe crear la carpeta de hooks");
    let hook_path = hooks.join("pre-commit");
    fs::write(
        &hook_path,
        "#!/bin/sh\necho 'pre-commit: la comprobación de formato falló' >&2\nexit 1\n",
    )
    .expect("debe escribir el hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .expect("debe hacer ejecutable el hook");
    }
    fs::write(temporary.path().join("a.txt"), "contenido").expect("debe escribir el archivo");
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    client
        .stage_all(temporary.path(), &cancellation)
        .expect("stage debe funcionar");

    let error = client
        .commit(temporary.path(), "mensaje de prueba", &cancellation)
        .expect_err("el hook debe rechazar el commit");

    let git_helper::git::GitError::HookRejected { hook, .. } = &error else {
        panic!("un hook que rechaza debe clasificarse como tal: {error:?}");
    };
    assert_eq!(hook.as_deref(), Some("pre-commit"));
    assert!(
        error
            .technical_details()
            .contains("la comprobación de formato falló")
    );
    assert!(
        !run_git(temporary.path(), &["log", "--oneline"])
            .status
            .success(),
        "el commit rechazado no debe crear historial"
    );
}

#[test]
fn rejected_push_and_divergent_pull_get_next_steps_without_forcing_anything() {
    let temporary = tempdir().expect("debe crear el directorio temporal");
    let origin = temporary.path().join("origin.git");
    require_git(
        temporary.path(),
        &["init", "--bare", "-b", "main", "origin.git"],
    );
    let first = temporary.path().join("first");
    let second = temporary.path().join("second");
    for clone in [&first, &second] {
        let output = Command::new("git")
            .arg("clone")
            .arg(&origin)
            .arg(clone)
            .output()
            .expect("Git debe poder clonar el repositorio local");
        assert!(output.status.success(), "clonar debe funcionar");
        require_git(clone, &["config", "user.name", "Git Helper Tests"]);
        require_git(
            clone,
            &["config", "user.email", "git-helper-tests@example.invalid"],
        );
    }
    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    commit_file(&client, &first, "a.txt", "uno", "primer commit");
    require_git(&first, &["push", "-u", "origin", "main"]);
    commit_file(&client, &second, "b.txt", "dos", "commit divergente");

    let rejected = client
        .execute_remote(
            &second,
            &git_helper::domain::RemoteOperationPlan::SetUpstreamAndPush {
                remote_name: "origin".to_owned(),
                branch_name: "main".to_owned(),
            },
            &cancellation,
        )
        .expect_err("el push sin integrar debe ser rechazado");

    assert!(
        matches!(rejected, git_helper::git::GitError::PushRejected { .. }),
        "un push rechazado debe clasificarse como tal: {rejected:?}"
    );
    let action = rejected
        .recommended_action()
        .expect("debe recomendar una acción");
    assert!(action.contains("Fetch"));
    assert!(action.to_lowercase().contains("no uses push forzado"));

    require_git(&second, &["fetch", "origin"]);
    require_git(
        &second,
        &["branch", "--set-upstream-to", "origin/main", "main"],
    );
    let divergent = client
        .execute_remote(
            &second,
            &git_helper::domain::RemoteOperationPlan::PullFastForward,
            &cancellation,
        )
        .expect_err("un pull fast-forward sobre ramas divergentes debe fallar");

    assert!(
        matches!(
            divergent,
            git_helper::git::GitError::DivergentBranches { .. }
        ),
        "una divergencia debe clasificarse como tal: {divergent:?}"
    );
    assert!(
        divergent
            .recommended_action()
            .expect("debe recomendar una acción")
            .contains("fuera de Git Helper")
    );
    let local_head = run_git(&second, &["rev-parse", "HEAD"]);
    let commit_message = run_git(&second, &["log", "-1", "--pretty=%s"]);
    assert!(local_head.status.success());
    assert_eq!(
        String::from_utf8_lossy(&commit_message.stdout).trim(),
        "commit divergente",
        "ni el push rechazado ni el pull fallido deben mover la rama local"
    );
}
