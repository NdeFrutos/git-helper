//! Apertura del editor, la terminal y el explorador de archivos desde el panel.
//!
//! Cada acción explícita del usuario abre como mucho una aplicación. Las rutas
//! viajan como argumentos literales del sistema —nunca como texto de una línea
//! de comandos— y no se ejecuta ningún script del repositorio ni shell alguna.

use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use std::ffi::OsString;

use thiserror::Error;

use crate::{
    domain::ExternalCommand,
    process::{ConsoleVisibility, LaunchRequest, ProcessError, launch_detached},
};

/// Herramienta externa que el usuario puede abrir.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalTool {
    Editor,
    Terminal,
    FileManager,
}

impl ExternalTool {
    /// Nombre usado en los mensajes de estado y de error.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Editor => "el editor",
            Self::Terminal => "la terminal",
            Self::FileManager => "el explorador de archivos",
        }
    }
}

/// Motivo por el que no se pudo abrir una herramienta externa.
#[derive(Debug, Error)]
pub enum ExternalToolError {
    #[error(
        "no se encontró ningún editor instalado. Usa «Elegir editor…» para indicar su ejecutable"
    )]
    EditorNotFound,
    #[error(
        "el editor configurado ya no está en {program}. Usa «Elegir editor…» para indicar su ubicación actual"
    )]
    EditorMissing { program: String },
    #[error("no se encontró una terminal del sistema para abrir {directory}")]
    TerminalNotFound { directory: String },
    #[error("no se encontró el explorador de archivos del sistema")]
    FileManagerNotFound,
    #[error("la ruta ya no existe: {path}. Actualiza el estado del repositorio")]
    PathMissing { path: String },
    #[error("no se pudo abrir {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: ProcessError,
    },
}

impl ExternalToolError {
    /// Indica si el usuario puede resolverlo eligiendo otro ejecutable de editor.
    #[must_use]
    pub const fn suggests_editor_setup(&self) -> bool {
        matches!(
            self,
            Self::EditorNotFound | Self::EditorMissing { .. } | Self::Spawn { .. }
        )
    }
}

/// Abre el repositorio —o uno de sus archivos— en el editor configurado.
///
/// El directorio de trabajo siempre es la raíz del repositorio, de modo que dos
/// repositorios con el mismo nombre abren cada uno su propia copia.
pub fn open_in_editor(
    configured: Option<&ExternalCommand>,
    root: &Path,
    file: Option<&Path>,
) -> Result<String, ExternalToolError> {
    let command = resolve_editor(configured)?;
    ensure_directory(root)?;
    let (target, message) = match file {
        Some(file) => {
            if !file.exists() {
                return Err(path_missing(file));
            }
            (
                file.to_path_buf(),
                format!("{} abierto en el editor", display_name(file)),
            )
        }
        None => (
            root.to_path_buf(),
            format!("{} abierto en el editor", display_name(root)),
        ),
    };
    launch(&editor_plan(&command, root, &target))?;
    Ok(message)
}

/// Abre una terminal visible cuyo directorio inicial es la raíz del repositorio.
pub fn open_terminal(root: &Path) -> Result<String, ExternalToolError> {
    ensure_directory(root)?;
    let plan = terminal_candidates(root)
        .into_iter()
        .find(|candidate| locate_program(&candidate.program).is_some())
        .ok_or_else(|| ExternalToolError::TerminalNotFound {
            directory: root.display().to_string(),
        })?;
    launch(&plan)?;
    Ok(format!("Terminal abierta en {}", display_name(root)))
}

/// Muestra el repositorio o un archivo en el explorador de archivos.
///
/// Un archivo que ya no existe —por ejemplo un cambio eliminado— no es un
/// error: se abre la carpeta existente más cercana dentro del repositorio.
pub fn reveal_in_file_manager(
    root: &Path,
    path: Option<&Path>,
) -> Result<String, ExternalToolError> {
    let program = locate_file_manager().ok_or(ExternalToolError::FileManagerNotFound)?;
    let Some(path) = path else {
        ensure_directory(root)?;
        launch(&reveal_plan(program, root, false))?;
        return Ok(format!("{} abierto en el explorador", display_name(root)));
    };
    if path.exists() {
        let is_file = path.is_file();
        launch(&reveal_plan(program, path, is_file))?;
        return Ok(format!("{} mostrado en el explorador", display_name(path)));
    }
    let fallback = nearest_existing_directory(path, root).ok_or_else(|| path_missing(root))?;
    launch(&reveal_plan(program, &fallback, false))?;
    Ok(format!(
        "{} ya no existe; se abrió {}",
        display_name(path),
        display_name(&fallback)
    ))
}

/// Construye el plan de apertura del editor sin ejecutarlo.
fn editor_plan(command: &ExternalCommand, root: &Path, target: &Path) -> LaunchRequest {
    LaunchRequest {
        label: "open-editor",
        program: command.program.clone(),
        arguments: command.arguments_for(target),
        current_directory: Some(root.to_path_buf()),
        console: ConsoleVisibility::Hidden,
    }
}

/// Construye el plan para revelar una ruta en el explorador de archivos.
fn reveal_plan(program: PathBuf, target: &Path, target_is_file: bool) -> LaunchRequest {
    // `/select,` es la forma documentada de que el Explorador abra la carpeta con
    // el archivo ya seleccionado. Se compone como un único argumento del sistema:
    // la ruta sigue siendo literal y no pasa por ningún intérprete.
    #[cfg(target_os = "windows")]
    let arguments = if target_is_file {
        let mut argument = OsString::from("/select,");
        argument.push(target.as_os_str());
        vec![argument]
    } else {
        vec![target.as_os_str().to_owned()]
    };
    // Fuera de Windows no hay una acción «revelar» equivalente y pedir al gestor
    // de archivos que abra el archivo delegaría en la aplicación asociada: se
    // abre su carpeta, que es lo que la acción promete.
    #[cfg(not(target_os = "windows"))]
    let arguments = {
        let directory = if target_is_file {
            target.parent().unwrap_or(target)
        } else {
            target
        };
        vec![directory.as_os_str().to_owned()]
    };

    LaunchRequest {
        label: "reveal-in-file-manager",
        program,
        arguments,
        current_directory: None,
        console: ConsoleVisibility::Hidden,
    }
}

/// Resuelve el editor configurado o el primero instalado que se reconozca.
fn resolve_editor(
    configured: Option<&ExternalCommand>,
) -> Result<ExternalCommand, ExternalToolError> {
    if let Some(command) = configured {
        return locate_program(&command.program)
            .map(|program| ExternalCommand::new(program, command.arguments.clone()))
            .ok_or_else(|| ExternalToolError::EditorMissing {
                program: command.program.display().to_string(),
            });
    }
    default_editor_candidates()
        .iter()
        .find_map(|candidate| locate_program(candidate))
        .map(ExternalCommand::with_target)
        .ok_or(ExternalToolError::EditorNotFound)
}

/// Editores detectados automáticamente cuando no hay configuración.
///
/// Solo se proponen ejecutables nativos: los lanzadores `.cmd` de VS Code o
/// Cursor necesitarían `cmd.exe`, y Git Helper no usa ninguna shell.
#[cfg(target_os = "windows")]
fn default_editor_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let programs = PathBuf::from(local_app_data).join("Programs");
        candidates.push(programs.join("cursor").join("Cursor.exe"));
        candidates.push(programs.join("Microsoft VS Code").join("Code.exe"));
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(variable) {
            let program_files = PathBuf::from(program_files);
            candidates.push(program_files.join("Microsoft VS Code").join("Code.exe"));
            candidates.push(program_files.join("cursor").join("Cursor.exe"));
        }
    }
    candidates.extend([PathBuf::from("Cursor.exe"), PathBuf::from("Code.exe")]);
    candidates
}

/// Fuera de Windows la detección es de conveniencia para el desarrollo.
#[cfg(not(target_os = "windows"))]
fn default_editor_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("cursor"),
        PathBuf::from("code"),
        PathBuf::from("codium"),
    ]
}

/// Terminales candidatas, en orden de preferencia, ya situadas en `root`.
#[cfg(target_os = "windows")]
fn terminal_candidates(root: &Path) -> Vec<LaunchRequest> {
    let windows_terminal = std::env::var_os("LOCALAPPDATA").map(|local_app_data| {
        PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WindowsApps")
            .join("wt.exe")
    });
    let system_root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut candidates = Vec::new();
    if let Some(windows_terminal) = windows_terminal {
        // Windows Terminal crea su propia ventana: `-d` fija la pestaña inicial.
        candidates.push(LaunchRequest {
            label: "open-terminal",
            program: windows_terminal,
            arguments: vec![OsString::from("-d"), root.as_os_str().to_owned()],
            current_directory: Some(root.to_path_buf()),
            console: ConsoleVisibility::Hidden,
        });
    }
    candidates.push(LaunchRequest {
        label: "open-terminal",
        program: system_root
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe"),
        arguments: vec![OsString::from("-NoLogo")],
        current_directory: Some(root.to_path_buf()),
        console: ConsoleVisibility::Visible,
    });
    candidates.push(LaunchRequest {
        label: "open-terminal",
        program: system_root.join("System32").join("cmd.exe"),
        arguments: Vec::new(),
        current_directory: Some(root.to_path_buf()),
        console: ConsoleVisibility::Visible,
    });
    candidates
}

/// Fuera de Windows las candidatas son de conveniencia para el desarrollo.
#[cfg(not(target_os = "windows"))]
fn terminal_candidates(root: &Path) -> Vec<LaunchRequest> {
    ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"]
        .into_iter()
        .map(|program| LaunchRequest {
            label: "open-terminal",
            program: PathBuf::from(program),
            arguments: Vec::new(),
            current_directory: Some(root.to_path_buf()),
            console: ConsoleVisibility::Visible,
        })
        .collect()
}

/// Explorador de archivos del sistema.
#[cfg(target_os = "windows")]
fn locate_file_manager() -> Option<PathBuf> {
    let explorer = std::env::var_os("SystemRoot").map_or_else(
        || PathBuf::from("explorer.exe"),
        |system_root| PathBuf::from(system_root).join("explorer.exe"),
    );
    locate_program(&explorer).or_else(|| locate_program(Path::new("explorer.exe")))
}

/// Fuera de Windows se usa el gestor de archivos predeterminado del escritorio.
#[cfg(not(target_os = "windows"))]
fn locate_file_manager() -> Option<PathBuf> {
    locate_program(Path::new("xdg-open"))
}

/// Comprueba que un programa existe, ya sea por ruta o a través del `PATH`.
fn locate_program(program: &Path) -> Option<PathBuf> {
    let has_directory = program
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    if has_directory {
        return program.is_file().then(|| program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        executable_extensions()
            .iter()
            .map(|extension| {
                let mut with_extension = candidate.clone().into_os_string();
                with_extension.push(".");
                with_extension.push(extension);
                PathBuf::from(with_extension)
            })
            .find(|candidate| candidate.is_file())
    })
}

/// Extensiones ejecutables que se prueban al buscar un nombre suelto en el `PATH`.
const fn executable_extensions() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["exe", "com"]
    } else {
        &[]
    }
}

/// Carpeta existente más cercana a `path` sin salir del repositorio.
fn nearest_existing_directory(path: &Path, root: &Path) -> Option<PathBuf> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if !directory.starts_with(root) {
            break;
        }
        if directory.is_dir() {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    root.is_dir().then(|| root.to_path_buf())
}

fn ensure_directory(root: &Path) -> Result<(), ExternalToolError> {
    if root.is_dir() {
        Ok(())
    } else {
        Err(path_missing(root))
    }
}

fn path_missing(path: &Path) -> ExternalToolError {
    ExternalToolError::PathMissing {
        path: path.display().to_string(),
    }
}

/// Nombre corto para los mensajes; la ruta completa ya está en la barra de estado.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn launch(request: &LaunchRequest) -> Result<(), ExternalToolError> {
    let program = request.program.display().to_string();
    launch_detached(request).map_err(|source| ExternalToolError::Spawn { program, source })
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
    };

    use crate::{
        domain::{ExternalCommand, ToolArgument},
        process::ConsoleVisibility,
    };

    use super::{
        ExternalToolError, editor_plan, locate_program, nearest_existing_directory, open_in_editor,
        resolve_editor, reveal_plan, terminal_candidates,
    };

    #[test]
    fn the_editor_receives_awkward_paths_as_a_single_literal_argument() {
        let command = ExternalCommand::new(
            "editor",
            vec![
                ToolArgument::Literal("--new-window".to_owned()),
                ToolArgument::Target,
            ],
        );
        let root = Path::new("/repos/mi proyecto & ñandú");
        let file = root.join("carpeta con espacios").join("archivo | raro.txt");

        let plan = editor_plan(&command, root, &file);

        assert_eq!(
            plan.arguments,
            vec![OsString::from("--new-window"), file.as_os_str().to_owned()]
        );
        assert_eq!(plan.current_directory.as_deref(), Some(root));
        assert_eq!(plan.console, ConsoleVisibility::Hidden);
    }

    #[test]
    fn repositories_with_the_same_name_keep_their_own_working_directory() {
        let command = ExternalCommand::with_target("editor");
        let first = Path::new("/trabajo/cliente-a/panel");
        let second = Path::new("/trabajo/cliente-b/panel");

        let first_plan = editor_plan(&command, first, first);
        let second_plan = editor_plan(&command, second, second);

        assert_eq!(first_plan.current_directory.as_deref(), Some(first));
        assert_eq!(second_plan.current_directory.as_deref(), Some(second));
        assert_ne!(first_plan.arguments, second_plan.arguments);
    }

    #[test]
    fn a_configured_editor_that_disappeared_can_be_reconfigured() {
        let missing = std::env::temp_dir().join("git-helper-editor-inexistente.exe");
        let configured = ExternalCommand::with_target(missing);

        let error = resolve_editor(Some(&configured)).expect_err("el editor no existe");

        assert!(matches!(error, ExternalToolError::EditorMissing { .. }));
        assert!(error.suggests_editor_setup());
        assert!(error.to_string().contains("Elegir editor"));
    }

    #[test]
    fn a_configured_editor_keeps_its_template_when_it_exists() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        let program = directory.path().join("editor-de-prueba");
        fs::write(&program, b"").expect("debe crear el ejecutable simulado");
        let configured = ExternalCommand::new(
            program.clone(),
            vec![
                ToolArgument::Literal("--wait".to_owned()),
                ToolArgument::Target,
            ],
        );

        let resolved = resolve_editor(Some(&configured)).expect("debe resolverse");

        assert_eq!(resolved.program, program);
        assert_eq!(resolved.arguments, configured.arguments);
    }

    #[test]
    fn opening_a_deleted_file_in_the_editor_explains_the_missing_path() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        let program = directory.path().join("editor-de-prueba");
        fs::write(&program, b"").expect("debe crear el ejecutable simulado");
        let configured = ExternalCommand::with_target(program);
        let deleted = directory.path().join("borrado.txt");

        let error = open_in_editor(Some(&configured), directory.path(), Some(&deleted))
            .expect_err("un archivo eliminado no puede abrirse");

        assert!(matches!(error, ExternalToolError::PathMissing { .. }));
    }

    #[test]
    fn a_repository_folder_that_disappeared_is_reported_before_launching() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        // El editor existe: lo que falta es la carpeta, y ese debe ser el error.
        let program = directory.path().join("editor-de-prueba");
        fs::write(&program, b"").expect("debe crear el ejecutable simulado");
        let configured = ExternalCommand::with_target(program);
        let root = directory.path().join("repositorio-que-ya-no-esta");

        let error = open_in_editor(Some(&configured), &root, None).expect_err("la raíz no existe");

        assert!(matches!(error, ExternalToolError::PathMissing { .. }));
    }

    #[test]
    fn a_deleted_file_reveals_the_closest_existing_folder() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        let root = directory.path();
        let nested = root.join("src").join("ui");
        fs::create_dir_all(&nested).expect("debe crear la jerarquía");

        let existing_parent = nearest_existing_directory(&nested.join("borrado.rs"), root);
        let removed_branch =
            nearest_existing_directory(&root.join("modulo").join("sub").join("borrado.rs"), root);

        assert_eq!(existing_parent.as_deref(), Some(nested.as_path()));
        assert_eq!(removed_branch.as_deref(), Some(root));
    }

    #[test]
    fn revealing_never_leaves_the_repository_root() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        let root = directory.path().join("repositorio");
        fs::create_dir_all(&root).expect("debe crear la raíz");
        let outside = directory.path().join("otro").join("archivo.txt");

        let resolved = nearest_existing_directory(&outside, &root);

        assert_eq!(resolved.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn revealing_a_file_points_the_file_manager_at_its_folder() {
        let root = Path::new("/repos/panel");
        let file = root.join("src").join("main rs.txt");

        let plan = reveal_plan(PathBuf::from("explorer"), &file, true);

        assert_eq!(
            plan.arguments.len(),
            1,
            "una acción abre una sola aplicación"
        );
        let argument = plan.arguments[0].to_string_lossy().into_owned();
        if cfg!(target_os = "windows") {
            assert_eq!(argument, format!("/select,{}", file.display()));
        } else {
            assert_eq!(
                argument,
                file.parent().expect("tiene carpeta").to_string_lossy()
            );
        }
    }

    #[test]
    fn every_terminal_candidate_starts_in_the_repository_root() {
        let root = Path::new("/repos/mi proyecto");

        let candidates = terminal_candidates(root);

        assert!(!candidates.is_empty());
        for candidate in candidates {
            assert_eq!(candidate.current_directory.as_deref(), Some(root));
            for argument in &candidate.arguments {
                assert!(
                    !argument.to_string_lossy().contains("&&"),
                    "los argumentos no componen una línea de comandos"
                );
            }
        }
    }

    #[test]
    fn a_program_name_without_directory_is_searched_in_the_path() {
        let directory = tempfile::tempdir().expect("debe crear el directorio temporal");
        let name = if cfg!(target_os = "windows") {
            "git-helper-fixture.exe"
        } else {
            "git-helper-fixture"
        };
        fs::write(directory.path().join(name), b"").expect("debe crear el ejecutable simulado");

        let located = locate_program(&PathBuf::from(name));
        let found_in_directory =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .any(|entry| entry == directory.path());

        assert_eq!(located.is_some(), found_in_directory);
        assert!(locate_program(Path::new("git-helper-programa-inexistente")).is_none());
    }
}
