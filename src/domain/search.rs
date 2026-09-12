use std::path::Path;

use super::{CommitSummary, FileChange};

/// Consulta de búsqueda normalizada.
///
/// El texto escrito por el usuario se trata siempre como dato literal: no se
/// interpreta como expresión regular ni se entrega a ninguna shell. La
/// comparación se hace en minúsculas Unicode para que acentos y mayúsculas no
/// obliguen a escribir la consulta exacta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    raw: String,
    folded: String,
    folded_path: String,
}

impl SearchQuery {
    /// Normaliza el texto escrito; devuelve `None` si no queda nada que buscar.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let folded = fold(trimmed);
        Some(Self {
            raw: trimmed.to_owned(),
            folded_path: normalize_separators(&folded),
            folded,
        })
    }

    /// Texto tal y como se muestra al usuario en la consulta activa.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Texto plegado que se usa para comparar texto libre.
    #[must_use]
    pub fn folded(&self) -> &str {
        &self.folded
    }
}

/// Indica si un cambio del working tree coincide con la consulta.
///
/// Se comparan la ruta actual y la original de un renombrado, de modo que la
/// entrada staged y la unstaged del mismo archivo se filtran igual y conservan
/// sus acciones por separado.
#[must_use]
pub fn change_matches(change: &FileChange, query: &SearchQuery) -> bool {
    path_matches(&change.path, query)
        || change
            .original_path
            .as_deref()
            .is_some_and(|path| path_matches(path, query))
}

/// Indica si un commit coincide por mensaje, autor o hash.
#[must_use]
pub fn commit_matches(commit: &CommitSummary, query: &SearchQuery) -> bool {
    contains_folded(&commit.subject, query.folded())
        || contains_folded(&commit.author_name, query.folded())
        || contains_folded(&commit.author_email, query.folded())
        || commit_id_matches(&commit.id, &commit.short_id, query.folded())
}

/// Compara rutas unificando el separador.
///
/// Escribir `src\ui` debe encontrar `src/ui`: Git siempre entrega barras
/// normales, pero en Windows el usuario escribe la barra invertida. Esa
/// equivalencia solo vale para rutas, no para el texto de un commit.
fn path_matches(path: &Path, query: &SearchQuery) -> bool {
    // `to_string_lossy` evita descartar rutas que el sistema no sepa
    // convertir a UTF-8; Git las entrega ya decodificadas en la práctica.
    let path = normalize_separators(&path.to_string_lossy());
    contains_folded(&path, &query.folded_path)
}

fn normalize_separators(value: &str) -> String {
    value.replace('\\', "/")
}

/// Compara el hash solo cuando la consulta es un prefijo hexadecimal.
///
/// Sin esa restricción una palabra como `added` acertaría contra cualquier
/// commit cuyo hash empiece por `adde`, que no es lo que el usuario pide.
fn commit_id_matches(id: &str, short_id: &str, folded: &str) -> bool {
    if !folded
        .chars()
        .all(|character| character.is_ascii_hexdigit())
    {
        return false;
    }
    starts_with_ignore_ascii_case(id, folded) || starts_with_ignore_ascii_case(short_id, folded)
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn fold(value: &str) -> String {
    value.to_lowercase()
}

/// Busca una subcadena ya plegada dentro de un texto sin plegar.
fn contains_folded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()));
    }
    fold(haystack).contains(needle)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{SearchQuery, change_matches, commit_matches};
    use crate::domain::{ChangeKind, CommitSummary, FileChange};

    fn change(path: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            original_path: None,
            index_status: ChangeKind::Modified,
            worktree_status: ChangeKind::Unmodified,
            is_conflicted: false,
        }
    }

    fn commit(subject: &str, author: &str, id: &str) -> CommitSummary {
        CommitSummary {
            id: id.to_owned(),
            short_id: id.chars().take(7).collect(),
            subject: subject.to_owned(),
            author_name: author.to_owned(),
            author_email: format!("{author}@example.test"),
            authored_at: 0,
            references: Vec::new(),
        }
    }

    #[test]
    fn empty_query_is_not_a_search() {
        assert!(SearchQuery::parse("   ").is_none());
        assert!(SearchQuery::parse("").is_none());
    }

    #[test]
    fn paths_match_without_case_or_separator_noise() {
        let query = SearchQuery::parse("SRC\\UI").unwrap();
        assert!(change_matches(&change("src/ui/main_window.rs"), &query));
        assert!(!change_matches(&change("src/git/client.rs"), &query));
    }

    #[test]
    fn unicode_paths_match_ignoring_case() {
        let query = SearchQuery::parse("AÑADIDO").unwrap();
        assert!(change_matches(&change("docs/añadido.md"), &query));
    }

    #[test]
    fn renamed_changes_match_by_original_path() {
        let mut renamed = change("src/nuevo.rs");
        renamed.original_path = Some(PathBuf::from("src/antiguo.rs"));
        let query = SearchQuery::parse("antiguo").unwrap();
        assert!(change_matches(&renamed, &query));
    }

    #[test]
    fn special_characters_are_literal_not_regular_expressions() {
        let query = SearchQuery::parse(".*").unwrap();
        assert!(!change_matches(&change("src/main.rs"), &query));
        assert!(change_matches(&change("src/a.*b.rs"), &query));
    }

    #[test]
    fn commits_match_by_subject_author_and_hash_prefix() {
        let summary = commit("feat: añade búsqueda", "Chloé", "abc123def4567890");
        let subject = SearchQuery::parse("BÚSQUEDA").unwrap();
        let author = SearchQuery::parse("chloé").unwrap();
        let email = SearchQuery::parse("chloé@example").unwrap();
        let hash = SearchQuery::parse("ABC123").unwrap();
        assert!(commit_matches(&summary, &subject));
        assert!(commit_matches(&summary, &author));
        assert!(commit_matches(&summary, &email));
        assert!(commit_matches(&summary, &hash));
    }

    #[test]
    fn non_hexadecimal_queries_never_match_the_hash() {
        let summary = commit("mensaje", "autora", "added0123456789");
        let query = SearchQuery::parse("addes").unwrap();
        assert!(!commit_matches(&summary, &query));
    }

    #[test]
    fn separator_equivalence_does_not_leak_into_commit_text() {
        let summary = commit(r"corrige ruta C:\temp", "autora", "abc1230");
        let literal = SearchQuery::parse(r"C:\temp").unwrap();
        let slashed = SearchQuery::parse("c:/temp").unwrap();
        assert!(commit_matches(&summary, &literal));
        assert!(!commit_matches(&summary, &slashed));
    }

    #[test]
    fn hexadecimal_queries_only_match_as_prefix() {
        let summary = commit("mensaje", "autora", "abc123def4567890");
        let middle = SearchQuery::parse("123de").unwrap();
        assert!(!commit_matches(&summary, &middle));
    }
}
