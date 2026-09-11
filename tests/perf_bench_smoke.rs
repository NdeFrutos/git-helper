//! Smoke test del benchmark headless: valida que el example compila y mide en un repo temporal.

use std::{fs, path::Path, process::Command};

use git_helper::{git::GitClient, process::CancellationToken};
use tempfile::tempdir;

fn run_git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .expect("Git debe estar instalado");
    assert!(
        output.status.success(),
        "git {arguments:?} falló: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn perf_bench_smoke_runs_on_temporary_repository() {
    let temporary = tempdir().expect("directorio temporal");
    let repository = temporary.path();
    run_git(repository, &["init", "-b", "main"]);
    run_git(repository, &["config", "user.name", "Perf Smoke"]);
    run_git(
        repository,
        &["config", "user.email", "perf-smoke@example.invalid"],
    );
    fs::write(repository.join("tracked.txt"), "initial").expect("escribir archivo");
    run_git(repository, &["add", "tracked.txt"]);
    run_git(repository, &["commit", "-m", "initial"]);
    fs::write(repository.join("tracked.txt"), "modified").expect("modificar archivo");

    let client = GitClient::default();
    let cancellation = CancellationToken::default();
    let _ = client.detect_version(&cancellation);

    let snapshot = client.snapshot(repository, &cancellation);
    assert!(
        snapshot.is_ok(),
        "snapshot debe completarse en repo temporal"
    );

    client
        .stage(repository, Path::new("tracked.txt"), &cancellation)
        .expect("stage debe funcionar");
    client
        .unstage(repository, Path::new("tracked.txt"), &cancellation)
        .expect("unstage debe funcionar");
}
