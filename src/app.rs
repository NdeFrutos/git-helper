use std::{path::PathBuf, sync::OnceLock};

use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    actions::{
        CloneRepository, CloseActiveRepository, CreateCommit, GenerateCommitMessage,
        NextRepository, OpenRepository, PreviousRepository, RefreshRepository, ShowChanges,
        ShowHistory,
    },
    ui::{CommitInput, MainWindow},
};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Inicia la aplicación y crea su ventana principal.
pub fn run() {
    initialize_logging();

    gpui_platform::application().run(|cx: &mut App| {
        CommitInput::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-o", OpenRepository, Some("GitHelper")),
            KeyBinding::new("ctrl-shift-o", CloneRepository, Some("GitHelper")),
            KeyBinding::new("ctrl-w", CloseActiveRepository, Some("GitHelper")),
            KeyBinding::new("ctrl-tab", NextRepository, Some("GitHelper")),
            KeyBinding::new("ctrl-shift-tab", PreviousRepository, Some("GitHelper")),
            KeyBinding::new("f5", RefreshRepository, Some("GitHelper")),
            KeyBinding::new("ctrl-1", ShowHistory, Some("GitHelper")),
            KeyBinding::new("ctrl-2", ShowChanges, Some("GitHelper")),
            KeyBinding::new("ctrl-enter", CreateCommit, Some("GitHelper")),
            KeyBinding::new("ctrl-shift-g", GenerateCommitMessage, Some("GitHelper")),
        ]);
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);
        let window_result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(MainWindow::new),
        );

        match window_result {
            Ok(window) => {
                if let Err(update_error) = window.update(cx, |main_window, window, cx| {
                    main_window.initialize(window, cx);
                }) {
                    error!(error = %update_error, "No se pudo inicializar la ventana principal");
                    cx.quit();
                    return;
                }
            }
            Err(window_error) => {
                error!(error = %window_error, "No se pudo abrir la ventana principal");
                cx.quit();
                return;
            }
        }

        info!("Git Helper iniciado");
        cx.activate(true);
    });
}

/// Configura consola y rotación diaria sin registrar contenido del repositorio.
fn initialize_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let log_directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("GitHelper").join("logs"));

    if let Some(log_directory) = log_directory
        && std::fs::create_dir_all(&log_directory).is_ok()
    {
        let appender = tracing_appender::rolling::daily(log_directory, "git-helper.log");
        let (file_writer, guard) = tracing_appender::non_blocking(appender);
        let result = tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(file_writer),
            )
            .try_init();
        if let Err(logging_error) = result {
            eprintln!("No se pudo inicializar el sistema de logs: {logging_error}");
        } else {
            let _ = LOG_GUARD.set(guard);
        }
        return;
    }

    if let Err(logging_error) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        eprintln!("No se pudo inicializar el sistema de logs: {logging_error}");
    }
}
