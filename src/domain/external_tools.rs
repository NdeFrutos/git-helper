use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Argumento tipado de una herramienta externa.
///
/// La plantilla se guarda como datos, nunca como una línea de comandos: así una
/// ruta con espacios, Unicode o metacaracteres llega literalmente al programa.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgument {
    /// Texto fijo configurado por el usuario, por ejemplo `--new-window`.
    Literal(String),
    /// Se sustituye por la ruta que se abre: la raíz del repositorio o un archivo.
    Target,
}

/// Programa externo y sus argumentos tipados.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalCommand {
    pub program: PathBuf,
    #[serde(default)]
    pub arguments: Vec<ToolArgument>,
}

impl ExternalCommand {
    /// Construye un comando con la plantilla indicada.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, arguments: Vec<ToolArgument>) -> Self {
        Self {
            program: program.into(),
            arguments,
        }
    }

    /// Construye el caso habitual: el programa recibe solo la ruta que se abre.
    #[must_use]
    pub fn with_target(program: impl Into<PathBuf>) -> Self {
        Self::new(program, vec![ToolArgument::Target])
    }

    /// Resuelve la plantilla para `target`.
    ///
    /// Si la configuración no menciona el destino, este se añade al final: basta
    /// con indicar el ejecutable para que la acción siga abriendo el repositorio.
    #[must_use]
    pub fn arguments_for(&self, target: &Path) -> Vec<OsString> {
        let mut resolved = Vec::with_capacity(self.arguments.len() + 1);
        let mut includes_target = false;
        for argument in &self.arguments {
            match argument {
                ToolArgument::Literal(text) => resolved.push(OsString::from(text)),
                ToolArgument::Target => {
                    includes_target = true;
                    resolved.push(target.as_os_str().to_owned());
                }
            }
        }
        if !includes_target {
            resolved.push(target.as_os_str().to_owned());
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, path::Path};

    use super::{ExternalCommand, ToolArgument};

    #[test]
    fn passes_paths_with_spaces_and_metacharacters_literally() {
        let command = ExternalCommand::new(
            "code.exe",
            vec![
                ToolArgument::Literal("--new-window".to_owned()),
                ToolArgument::Target,
            ],
        );
        let target = Path::new(r"C:\Users\dev\mis repos\proyecto & ñandú\a b.txt");

        let arguments = command.arguments_for(target);

        assert_eq!(
            arguments,
            vec![
                OsString::from("--new-window"),
                target.as_os_str().to_owned()
            ]
        );
    }

    #[test]
    fn appends_the_target_when_the_template_does_not_name_it() {
        let command =
            ExternalCommand::new("code.exe", vec![ToolArgument::Literal("-n".to_owned())]);

        let arguments = command.arguments_for(Path::new("repo"));

        assert_eq!(
            arguments,
            vec![OsString::from("-n"), OsString::from("repo")]
        );
    }

    #[test]
    fn keeps_the_target_position_chosen_in_the_template() {
        let command = ExternalCommand::new(
            "editor",
            vec![
                ToolArgument::Target,
                ToolArgument::Literal("--wait".to_owned()),
            ],
        );

        let arguments = command.arguments_for(Path::new("repo"));

        assert_eq!(
            arguments,
            vec![OsString::from("repo"), OsString::from("--wait")]
        );
    }

    #[test]
    fn serializes_arguments_as_readable_data() {
        let command = ExternalCommand::new(
            "code.exe",
            vec![
                ToolArgument::Literal("--new-window".to_owned()),
                ToolArgument::Target,
            ],
        );

        let serialized = serde_json::to_string(&command).expect("debe serializarse");
        let restored =
            serde_json::from_str::<ExternalCommand>(&serialized).expect("debe leerse de vuelta");

        assert!(serialized.contains(r#"{"literal":"--new-window"}"#));
        assert!(serialized.contains(r#""target""#));
        assert_eq!(restored, command);
    }
}
