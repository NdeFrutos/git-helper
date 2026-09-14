use std::path::PathBuf;

use thiserror::Error;

/// Argumentos parseados del comando `ghelper`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliArgs {
    /// Ruta de repositorio solicitada por el usuario, si la hubo.
    pub repository_path: Option<PathBuf>,
}

/// Argumentos internos que acepta `git-helper.exe` al arrancar desde `ghelper`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InternalStartupArgs {
    /// Raíz canónica del repositorio que debe abrirse al iniciar.
    pub open_repository: Option<PathBuf>,
}

/// Errores de parseo de la línea de comandos.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliParseError {
    #[error("argumento desconocido: {0}")]
    UnknownArgument(String),
    #[error("demasiados argumentos; usa ghelper --help para ver la sintaxis")]
    TooManyArguments,
}

const HELP_TEXT: &str = "\
Git Helper — abre repositorios Git desde la terminal

Uso:
  ghelper [RUTA]
  ghelper --help

Argumentos:
  RUTA    Directorio del repositorio (absoluta o relativa al directorio actual)

Sin argumentos restaura la sesión persistida. Si Git Helper ya está abierto,
la ruta se reenvía a la ventana existente y se activa la pestaña correspondiente.

Ejemplos:
  ghelper .
  ghelper \"C:\\Users\\dev\\proyectos\\mi-repo\"
";

/// Parsea la sintaxis pública de `ghelper`.
pub fn parse_cli_args<I, S>(arguments: I) -> Result<CliArgs, CliParseError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    let arguments = skip_program_name(&arguments);

    match arguments {
        [] => Ok(CliArgs {
            repository_path: None,
        }),
        [path] if path.starts_with('-') => Err(CliParseError::UnknownArgument(path.clone())),
        [path] => Ok(CliArgs {
            repository_path: Some(PathBuf::from(path)),
        }),
        _ => Err(CliParseError::TooManyArguments),
    }
}

/// Indica si los argumentos públicos solicitan la ayuda.
pub fn wants_help<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    matches!(
        skip_program_name(&arguments),
        [flag] if flag == "--help" || flag == "-h"
    )
}

/// Devuelve el texto de ayuda de `ghelper`.
#[must_use]
pub fn help_text() -> &'static str {
    HELP_TEXT
}

/// Parsea los flags internos de `git-helper.exe`.
pub fn parse_internal_startup_args<I, S>(arguments: I) -> InternalStartupArgs
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let arguments = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    let mut arguments = skip_program_name(&arguments).iter();
    let mut open_repository = None;

    while let Some(argument) = arguments.next() {
        if *argument == "--open-repo" {
            let Some(path) = arguments.next() else {
                return InternalStartupArgs::default();
            };
            open_repository = Some(PathBuf::from(path));
        }
    }

    InternalStartupArgs { open_repository }
}

fn skip_program_name(arguments: &[String]) -> &[String] {
    arguments.get(1..).unwrap_or(arguments)
}

#[cfg(test)]
mod tests {
    use super::{
        CliParseError, help_text, parse_cli_args, parse_internal_startup_args, wants_help,
    };

    #[test]
    fn parses_empty_arguments() {
        assert_eq!(
            parse_cli_args(["ghelper"]).unwrap(),
            super::CliArgs {
                repository_path: None
            }
        );
    }

    #[test]
    fn parses_repository_path() {
        let parsed = parse_cli_args(["ghelper", r"C:\repos\demo"]).unwrap();
        assert_eq!(
            parsed.repository_path,
            Some(std::path::PathBuf::from(r"C:\repos\demo"))
        );
    }

    #[test]
    fn rejects_unknown_flags() {
        assert_eq!(
            parse_cli_args(["ghelper", "--verbose"]).unwrap_err(),
            CliParseError::UnknownArgument("--verbose".to_owned())
        );
    }

    #[test]
    fn rejects_extra_arguments() {
        assert_eq!(
            parse_cli_args(["ghelper", "uno", "dos"]).unwrap_err(),
            CliParseError::TooManyArguments
        );
    }

    #[test]
    fn detects_help_request() {
        assert!(wants_help(["ghelper", "--help"]));
        assert!(!wants_help(["ghelper", "."]));
        assert!(help_text().contains("ghelper [RUTA]"));
    }

    #[test]
    fn parses_internal_open_repository_flag() {
        let parsed =
            parse_internal_startup_args(["git-helper.exe", "--open-repo", r"C:\repos\demo"]);
        assert_eq!(
            parsed.open_repository,
            Some(std::path::PathBuf::from(r"C:\repos\demo"))
        );
    }
}
