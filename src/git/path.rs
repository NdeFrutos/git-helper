use std::{
    collections::HashSet,
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use super::GitError;

/// Presupuesto conservador para la línea de comandos de Windows.
///
/// `CreateProcessW` admite 32 767 unidades UTF-16 incluyendo el terminador;
/// el margen restante absorbe el entrecomillado que añade el runtime.
const COMMAND_LINE_BUDGET: usize = 30_000;

/// Cota adicional de rutas por invocación, para no depender solo del tamaño.
const MAX_PATHS_PER_BATCH: usize = 512;

/// Coste en unidades UTF-16 de pasar `value` como argumento entrecomillado.
#[must_use]
pub fn command_line_cost(value: &OsStr) -> usize {
    value
        .to_string_lossy()
        .chars()
        .map(char::len_utf16)
        .sum::<usize>()
        // Dos comillas y el separador que introduce el runtime.
        .saturating_add(3)
}

/// Valida un lote completo antes de ejecutar nada y elimina repeticiones.
///
/// Un único pathspec inseguro invalida el lote entero: así una selección no
/// puede aplicarse a medias por una ruta que nunca debió llegar a Git. Conserva
/// el orden de entrada.
pub fn validate_pathspecs(paths: &[PathBuf]) -> Result<Vec<PathBuf>, GitError> {
    let mut seen = HashSet::new();
    let mut validated = Vec::with_capacity(paths.len());
    for path in paths {
        let path = validate_relative_path(path)?;
        if seen.insert(path.clone()) {
            validated.push(path);
        }
    }
    Ok(validated)
}

/// Reparte pathspecs en lotes que caben en la línea de comandos de Windows.
///
/// `reserved` es el coste del prefijo fijo (`git -C <raíz> add --`). Una ruta
/// que por sí sola agote el presupuesto viaja en su propio lote: Git decidirá
/// si puede procesarla y el error quedará atribuido exactamente a esa ruta.
#[must_use]
pub fn plan_pathspec_batches(paths: &[PathBuf], reserved: usize) -> Vec<Vec<PathBuf>> {
    let budget = COMMAND_LINE_BUDGET.saturating_sub(reserved);
    let mut batches = Vec::new();
    let mut current: Vec<PathBuf> = Vec::new();
    let mut used = 0;
    for path in paths {
        let cost = command_line_cost(path.as_os_str());
        let exceeds_budget = used + cost > budget;
        let exceeds_count = current.len() >= MAX_PATHS_PER_BATCH;
        if !current.is_empty() && (exceeds_budget || exceeds_count) {
            batches.push(std::mem::take(&mut current));
            used = 0;
        }
        used += cost;
        current.push(path.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Rechaza pathspecs vacíos, absolutos o capaces de escapar del repositorio.
pub fn validate_relative_path(path: &Path) -> Result<PathBuf, GitError> {
    let mut has_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(GitError::UnsafePath {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    if !has_normal_component {
        return Err(GitError::UnsafePath {
            path: path.to_path_buf(),
        });
    }
    Ok(path.to_path_buf())
}

/// Comprueba físicamente una ruta existente antes de eliminarla.
pub fn validate_existing_path_inside_repository(
    repository_root: &Path,
    relative_path: &Path,
) -> Result<PathBuf, GitError> {
    let relative_path = validate_relative_path(relative_path)?;
    let canonical_root = repository_root
        .canonicalize()
        .map_err(|source| GitError::Io {
            path: repository_root.to_path_buf(),
            source,
        })?;
    let full_path = repository_root.join(&relative_path);
    let canonical_path = full_path.canonicalize().map_err(|source| GitError::Io {
        path: full_path,
        source,
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(GitError::PathOutsideRepository {
            path: canonical_path,
        });
    }
    Ok(relative_path)
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsStr,
        path::{Path, PathBuf},
    };

    use super::{
        COMMAND_LINE_BUDGET, MAX_PATHS_PER_BATCH, command_line_cost, plan_pathspec_batches,
        validate_relative_path,
    };

    #[test]
    fn rejects_empty_absolute_and_parent_paths() {
        assert!(validate_relative_path(Path::new("")).is_err());
        assert!(validate_relative_path(Path::new("../secreto.txt")).is_err());
        assert!(validate_relative_path(Path::new(r"C:\secreto.txt")).is_err());
    }

    #[test]
    fn accepts_spaces_unicode_and_leading_dashes() {
        assert!(validate_relative_path(Path::new("ruta con ñ/-archivo.txt")).is_ok());
    }

    #[test]
    fn counts_astral_characters_as_two_utf16_units() {
        // Un emoji ocupa dos unidades en la línea de comandos de Windows.
        assert_eq!(command_line_cost(OsStr::new("a")), 4);
        assert_eq!(command_line_cost(OsStr::new("🦀")), 5);
    }

    #[test]
    fn splits_batches_that_exceed_the_windows_command_line() {
        let path = PathBuf::from("directorio/".to_owned() + &"ñ".repeat(200));
        let cost = command_line_cost(path.as_os_str());
        let fit = COMMAND_LINE_BUDGET / cost;
        let paths = vec![path; fit + 1];

        let batches = plan_pathspec_batches(&paths, 0);

        assert!(batches.len() > 1);
        assert_eq!(
            batches.iter().map(Vec::len).sum::<usize>(),
            paths.len(),
            "ninguna ruta puede perderse al repartir el lote"
        );
        for batch in &batches {
            let used: usize = batch
                .iter()
                .map(|path| command_line_cost(path.as_os_str()))
                .sum();
            assert!(used <= COMMAND_LINE_BUDGET);
        }
    }

    #[test]
    fn reserves_room_for_the_fixed_prefix_and_caps_the_batch_size() {
        let paths = vec![PathBuf::from("a.txt"); MAX_PATHS_PER_BATCH + 1];

        assert_eq!(plan_pathspec_batches(&paths, 0).len(), 2);

        let huge = PathBuf::from("x".repeat(400));
        let paths = vec![huge.clone(), huge];
        let reserved = COMMAND_LINE_BUDGET - 500;

        let batches = plan_pathspec_batches(&paths, reserved);

        assert_eq!(batches.len(), 2, "el prefijo reservado reduce el lote");
    }

    #[test]
    fn isolates_a_path_that_does_not_fit_on_its_own() {
        let huge = PathBuf::from("x".repeat(COMMAND_LINE_BUDGET + 10));
        let paths = vec![PathBuf::from("a.txt"), huge.clone(), PathBuf::from("b.txt")];

        let batches = plan_pathspec_batches(&paths, 0);

        assert_eq!(
            batches,
            vec![
                vec![PathBuf::from("a.txt")],
                vec![huge],
                vec![PathBuf::from("b.txt")]
            ]
        );
    }

    #[test]
    fn returns_no_batches_for_an_empty_selection() {
        assert!(plan_pathspec_batches(&[], 0).is_empty());
    }

    #[test]
    fn validating_a_batch_keeps_order_drops_repeats_and_rejects_the_whole_set() {
        let paths = vec![
            PathBuf::from("b.txt"),
            PathBuf::from("a ñ.txt"),
            PathBuf::from("b.txt"),
        ];

        assert_eq!(
            super::validate_pathspecs(&paths).unwrap(),
            vec![PathBuf::from("b.txt"), PathBuf::from("a ñ.txt"),]
        );

        let unsafe_batch = vec![PathBuf::from("ok.txt"), PathBuf::from("../secreto.txt")];
        assert!(super::validate_pathspecs(&unsafe_batch).is_err());
    }
}
