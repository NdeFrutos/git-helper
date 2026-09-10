use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use git_helper::{
    domain::ChangeKind,
    git::{GitClient, plan_discard, plan_pull, plan_push},
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

    assert!(file_path.exists());
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
        .snapshot(&first, 200, &cancellation)
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
        .snapshot(&first, 200, &cancellation)
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

    assert!(pull_error.to_string().contains("Git rechazó"));
    assert_eq!(head_before, head_after);
}
