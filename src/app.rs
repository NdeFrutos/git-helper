use std::{path::PathBuf, sync::OnceLock};

use async_channel::unbounded;
use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, point, px, size};
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    actions::{
        CloneRepository, CloseActiveRepository, CreateCommit, GenerateCommitMessage,
        NextRepository, OpenInEditor, OpenRepository, OpenTerminalHere, PreviousRepository,
        RefreshRepository, RevealInFileManager, ShowChanges, ShowHistory,
    },
    cli::InstanceServer,
    persistence::{
        AppStateStore, DisplayBounds, WindowPlacement, default_window_placement,
        validate_window_placement,
    },
    ui::{CommitInput, MainWindow, StartupState},
};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Configuración de arranque de la aplicación gráfica.
#[derive(Clone, Debug, Default)]
pub struct AppStartup {
    /// Repositorio que debe abrirse al iniciar, si se invocó desde la CLI.
    pub open_repository: Option<PathBuf>,
}

/// Inicia la aplicación y crea su ventana principal.
pub fn run(startup: AppStartup) {
    initialize_logging();

    let (instance_sender, instance_receiver) = unbounded();
    let _instance_server = InstanceServer::start(instance_sender)
        .inspect_err(
            |error| error!(error = %error, "No se pudo iniciar el servidor de instancia única"),
        )
        .ok();

    gpui_platform::application().run(move |cx: &mut App| {
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
            KeyBinding::new("ctrl-shift-e", OpenInEditor, Some("GitHelper")),
            KeyBinding::new("ctrl-shift-t", OpenTerminalHere, Some("GitHelper")),
            KeyBinding::new("ctrl-shift-x", RevealInFileManager, Some("GitHelper")),
        ]);
        let persisted_startup = locate_startup_store();
        let bounds = initial_window_bounds(cx, None);
        let window_result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    MainWindow::new(
                        persisted_startup,
                        startup.clone(),
                        instance_receiver.clone(),
                        cx,
                    )
                })
            },
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

/// Localiza el almacén sin leerlo; la lectura se hará en background tras el primer frame.
fn locate_startup_store() -> Option<StartupState> {
    let store = AppStateStore::default_location()
        .inspect_err(|error| error!(error = %error, "No se pudo localizar el estado persistido"))
        .ok()?;
    Some(StartupState { store })
}

/// Ajusta la geometría ya leída a los monitores visibles; no vuelve a tocar disco.
fn initial_window_bounds(cx: &App, persisted: Option<WindowPlacement>) -> Bounds<gpui::Pixels> {
    let displays = display_bounds(cx);
    let placement = persisted.map_or_else(
        || default_window_placement(&displays),
        |placement| validate_window_placement(placement, &displays),
    );
    Bounds::new(
        point(px(placement.x), px(placement.y)),
        size(px(placement.width), px(placement.height)),
    )
}

fn display_bounds(cx: &App) -> Vec<DisplayBounds> {
    cx.displays()
        .iter()
        .map(|display| {
            let bounds = display.visible_bounds();
            DisplayBounds {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            }
        })
        .collect()
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
