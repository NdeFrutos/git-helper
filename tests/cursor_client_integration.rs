#![cfg(windows)]

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{Arc, Mutex},
};

use git_helper::{
    cursor::{CursorAuthentication, CursorClient},
    process::{CancellationToken, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner},
};
use std::os::windows::process::ExitStatusExt;

#[derive(Default)]
struct FakeProcessRunner {
    requests: Mutex<Vec<ProcessRequest>>,
    outputs: Mutex<VecDeque<ProcessOutput>>,
}

impl FakeProcessRunner {
    fn with_outputs(outputs: Vec<ProcessOutput>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into()),
        }
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn run(
        &self,
        request: ProcessRequest,
        _: &CancellationToken,
    ) -> Result<ProcessOutput, ProcessError> {
        self.requests
            .lock()
            .expect("el mutex no debe estar contaminado")
            .push(request);
        self.outputs
            .lock()
            .expect("el mutex no debe estar contaminado")
            .pop_front()
            .ok_or_else(|| ProcessError::Spawn(std::io::Error::other("falta salida simulada")))
    }
}

fn successful_output(stdout: &str) -> ProcessOutput {
    ProcessOutput {
        status: ExitStatus::from_raw(0),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

#[test]
fn validates_and_generates_in_restricted_ask_mode() {
    let runner = Arc::new(FakeProcessRunner::with_outputs(vec![
        successful_output("agent 2.0"),
        successful_output(r#"{"authenticated":true}"#),
        successful_output(r#"{"type":"result","is_error":false,"result":"feat: añade historial"}"#),
    ]));
    let client = CursorClient::with_runner(PathBuf::from("agent"), runner.clone());
    let cancellation = CancellationToken::default();

    let availability = client
        .validate(&cancellation)
        .expect("la validación simulada debe funcionar");
    let message = client
        .generate_commit_message(
            Path::new(r"C:\repositorio con espacios"),
            "prompt privado".to_owned(),
            &cancellation,
        )
        .expect("la generación simulada debe funcionar");

    assert_eq!(
        availability.authentication,
        CursorAuthentication::Authenticated
    );
    assert_eq!(message, "feat: añade historial");
    let requests = runner
        .requests
        .lock()
        .expect("el mutex no debe estar contaminado");
    let generation = &requests[2];
    let arguments: Vec<String> = generation
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        arguments,
        [
            "-p",
            "--mode",
            "ask",
            "--model",
            "composer-2.5",
            "--workspace",
            r"C:\repositorio con espacios",
            "--output-format",
            "json",
        ]
    );
    assert_eq!(
        generation.stdin.as_deref(),
        Some(b"prompt privado".as_slice())
    );
    assert!(
        generation
            .removed_environment
            .iter()
            .any(|name| name == "CURSOR_API_KEY")
    );
    assert!(!arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--force" | "--yolo" | "--trust" | "--api-key"
        )
    }));
}
