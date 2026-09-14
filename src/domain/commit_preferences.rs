use std::{collections::HashMap, hash::BuildHasher};

use serde::{Deserialize, Serialize};

/// Longitud orientativa del asunto cuando no hay preferencia guardada.
pub const DEFAULT_SUBJECT_MAX_LENGTH: u32 = 72;
/// Cota inferior aceptada al normalizar un valor persistido o editado a mano.
pub const MIN_SUBJECT_MAX_LENGTH: u32 = 20;
/// Cota superior aceptada al normalizar un valor persistido o editado a mano.
pub const MAX_SUBJECT_MAX_LENGTH: u32 = 120;
/// Valores que recorre la interfaz al ajustar la longitud del asunto.
pub const SUBJECT_MAX_LENGTH_OPTIONS: [u32; 4] = [50, 60, 72, 100];

/// Idioma pedido al agente para la propuesta de mensaje.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommitMessageLanguage {
    /// Sigue el idioma de los asuntos recientes del repositorio.
    #[default]
    FollowHistory,
    Spanish,
    English,
}

impl CommitMessageLanguage {
    /// Siguiente valor del ciclo mostrado en la interfaz.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::FollowHistory => Self::Spanish,
            Self::Spanish => Self::English,
            Self::English => Self::FollowHistory,
        }
    }

    /// Etiqueta corta usada en los controles compactos.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FollowHistory => "según el historial",
            Self::Spanish => "español",
            Self::English => "inglés",
        }
    }
}

/// Convención aplicada al asunto de la propuesta.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommitMessageConvention {
    /// Texto libre: solo se piden asunto claro e imperativo.
    #[default]
    FreeForm,
    ConventionalCommits,
}

impl CommitMessageConvention {
    /// Siguiente valor del ciclo mostrado en la interfaz.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::FreeForm => Self::ConventionalCommits,
            Self::ConventionalCommits => Self::FreeForm,
        }
    }

    /// Etiqueta corta usada en los controles compactos.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FreeForm => "texto libre",
            Self::ConventionalCommits => "conventional",
        }
    }
}

/// Uso del alcance (`scope`) cuando se pide Conventional Commits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommitScopeUsage {
    Omit,
    /// El agente decide si el alcance aporta información.
    #[default]
    Optional,
    Required,
}

impl CommitScopeUsage {
    /// Siguiente valor del ciclo mostrado en la interfaz.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Omit => Self::Optional,
            Self::Optional => Self::Required,
            Self::Required => Self::Omit,
        }
    }

    /// Etiqueta corta usada en los controles compactos.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Omit => "sin alcance",
            Self::Optional => "alcance opcional",
            Self::Required => "alcance obligatorio",
        }
    }
}

/// Preferencias completas aplicadas a una generación concreta.
///
/// El mismo valor normalizado se entrega a cualquier proveedor de generación,
/// de modo que Cursor, Codex o Claude reciban instrucciones idénticas.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitMessagePreferences {
    #[serde(default)]
    pub language: CommitMessageLanguage,
    #[serde(default)]
    pub convention: CommitMessageConvention,
    #[serde(default)]
    pub scope: CommitScopeUsage,
    #[serde(default = "default_subject_max_length")]
    pub subject_max_length: u32,
}

const fn default_subject_max_length() -> u32 {
    DEFAULT_SUBJECT_MAX_LENGTH
}

impl Default for CommitMessagePreferences {
    fn default() -> Self {
        Self {
            language: CommitMessageLanguage::default(),
            convention: CommitMessageConvention::default(),
            scope: CommitScopeUsage::default(),
            subject_max_length: DEFAULT_SUBJECT_MAX_LENGTH,
        }
    }
}

impl CommitMessagePreferences {
    /// Devuelve las preferencias dentro de los límites soportados.
    ///
    /// Un valor fuera de rango procede de un estado antiguo o editado a mano;
    /// nunca debe llegar al prompt tal cual.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            subject_max_length: normalize_subject_max_length(self.subject_max_length),
            ..self
        }
    }

    /// Indica si el alcance influye en la propuesta con la convención elegida.
    #[must_use]
    pub const fn scope_applies(self) -> bool {
        matches!(
            self.convention,
            CommitMessageConvention::ConventionalCommits
        )
    }
}

/// Ajusta una longitud de asunto a las cotas soportadas.
#[must_use]
pub const fn normalize_subject_max_length(value: u32) -> u32 {
    if value == 0 {
        return DEFAULT_SUBJECT_MAX_LENGTH;
    }
    if value < MIN_SUBJECT_MAX_LENGTH {
        return MIN_SUBJECT_MAX_LENGTH;
    }
    if value > MAX_SUBJECT_MAX_LENGTH {
        return MAX_SUBJECT_MAX_LENGTH;
    }
    value
}

/// Siguiente longitud orientativa ofrecida por la interfaz.
#[must_use]
pub fn next_subject_max_length(current: u32) -> u32 {
    let current = normalize_subject_max_length(current);
    SUBJECT_MAX_LENGTH_OPTIONS
        .iter()
        .find(|option| **option > current)
        .copied()
        .unwrap_or(SUBJECT_MAX_LENGTH_OPTIONS[0])
}

/// Sobrescrituras de un repositorio; los campos ausentes heredan del global.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitMessagePreferenceOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<CommitMessageLanguage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convention: Option<CommitMessageConvention>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<CommitScopeUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_max_length: Option<u32>,
}

impl CommitMessagePreferenceOverrides {
    /// Indica que el repositorio hereda todos los valores globales.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.language.is_none()
            && self.convention.is_none()
            && self.scope.is_none()
            && self.subject_max_length.is_none()
    }

    /// Aplica las mismas cotas que el valor global a la sobrescritura.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            subject_max_length: match self.subject_max_length {
                Some(value) => Some(normalize_subject_max_length(value)),
                None => None,
            },
            ..self
        }
    }
}

/// Capa de la que procede el valor mostrado en la interfaz.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferenceSource {
    Global,
    Repository,
}

impl PreferenceSource {
    /// Sufijo que distingue un valor heredado de uno propio del repositorio.
    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Global => "",
            Self::Repository => " ·repo",
        }
    }
}

/// Valor efectivo junto a la capa que lo aporta.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedPreference<T> {
    pub value: T,
    pub source: PreferenceSource,
}

impl<T> ResolvedPreference<T> {
    fn new(override_value: Option<T>, global_value: T) -> Self {
        match override_value {
            Some(value) => Self {
                value,
                source: PreferenceSource::Repository,
            },
            None => Self {
                value: global_value,
                source: PreferenceSource::Global,
            },
        }
    }
}

/// Preferencias efectivas de un repositorio con el origen de cada campo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectiveCommitPreferences {
    pub language: ResolvedPreference<CommitMessageLanguage>,
    pub convention: ResolvedPreference<CommitMessageConvention>,
    pub scope: ResolvedPreference<CommitScopeUsage>,
    pub subject_max_length: ResolvedPreference<u32>,
}

impl EffectiveCommitPreferences {
    /// Preferencias planas que se envían al proveedor de generación.
    #[must_use]
    pub const fn values(self) -> CommitMessagePreferences {
        CommitMessagePreferences {
            language: self.language.value,
            convention: self.convention.value,
            scope: self.scope.value,
            subject_max_length: self.subject_max_length.value,
        }
    }

    /// Indica si algún campo procede del propio repositorio.
    #[must_use]
    pub const fn has_repository_overrides(self) -> bool {
        matches!(self.language.source, PreferenceSource::Repository)
            || matches!(self.convention.source, PreferenceSource::Repository)
            || matches!(self.scope.source, PreferenceSource::Repository)
            || matches!(self.subject_max_length.source, PreferenceSource::Repository)
    }
}

/// Combina preferencias globales y sobrescrituras en valores efectivos normalizados.
#[must_use]
pub fn resolve_commit_preferences(
    global: CommitMessagePreferences,
    overrides: Option<CommitMessagePreferenceOverrides>,
) -> EffectiveCommitPreferences {
    let global = global.normalized();
    let overrides = overrides.unwrap_or_default().normalized();
    EffectiveCommitPreferences {
        language: ResolvedPreference::new(overrides.language, global.language),
        convention: ResolvedPreference::new(overrides.convention, global.convention),
        scope: ResolvedPreference::new(overrides.scope, global.scope),
        subject_max_length: ResolvedPreference::new(
            overrides.subject_max_length,
            global.subject_max_length,
        ),
    }
}

/// Devuelve las preferencias efectivas de una raíz de repositorio.
#[must_use]
pub fn effective_commit_preferences(
    global: CommitMessagePreferences,
    overrides_by_repository: &HashMap<String, CommitMessagePreferenceOverrides, impl BuildHasher>,
    repository_key: &str,
) -> EffectiveCommitPreferences {
    resolve_commit_preferences(global, overrides_by_repository.get(repository_key).copied())
}

/// Campo editable desde los controles compactos del formulario de commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitPreferenceField {
    Language,
    Convention,
    Scope,
    SubjectMaxLength,
}

/// Capa sobre la que actúa la edición solicitada por el usuario.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PreferenceScope {
    #[default]
    Global,
    Repository,
}

impl PreferenceScope {
    /// Alterna entre editar el valor global y el del repositorio activo.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Global => Self::Repository,
            Self::Repository => Self::Global,
        }
    }

    /// Etiqueta del selector de capa.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Repository => "este repo",
        }
    }
}

/// Avanza un campo al siguiente valor de la capa indicada.
///
/// Al editar el repositorio se parte del valor efectivo, de modo que el primer
/// clic sobre un valor heredado crea una sobrescritura con el siguiente valor
/// visible y no con el siguiente al global.
pub fn cycle_commit_preference(
    field: CommitPreferenceField,
    scope: PreferenceScope,
    global: &mut CommitMessagePreferences,
    overrides: &mut CommitMessagePreferenceOverrides,
) {
    let effective = resolve_commit_preferences(*global, Some(*overrides)).values();
    match (scope, field) {
        (PreferenceScope::Global, CommitPreferenceField::Language) => {
            global.language = global.language.next();
        }
        (PreferenceScope::Global, CommitPreferenceField::Convention) => {
            global.convention = global.convention.next();
        }
        (PreferenceScope::Global, CommitPreferenceField::Scope) => {
            global.scope = global.scope.next();
        }
        (PreferenceScope::Global, CommitPreferenceField::SubjectMaxLength) => {
            global.subject_max_length = next_subject_max_length(global.subject_max_length);
        }
        (PreferenceScope::Repository, CommitPreferenceField::Language) => {
            overrides.language = Some(effective.language.next());
        }
        (PreferenceScope::Repository, CommitPreferenceField::Convention) => {
            overrides.convention = Some(effective.convention.next());
        }
        (PreferenceScope::Repository, CommitPreferenceField::Scope) => {
            overrides.scope = Some(effective.scope.next());
        }
        (PreferenceScope::Repository, CommitPreferenceField::SubjectMaxLength) => {
            overrides.subject_max_length =
                Some(next_subject_max_length(effective.subject_max_length));
        }
    }
    *global = global.normalized();
    *overrides = overrides.normalized();
}

/// Instrucciones normalizadas que se añaden al prompt de cualquier proveedor.
#[must_use]
pub fn commit_preferences_instructions(preferences: CommitMessagePreferences) -> String {
    let preferences = preferences.normalized();
    let language = match preferences.language {
        CommitMessageLanguage::FollowHistory => {
            "Language: write the message in the same language as the recent commit subjects above; do not translate it."
        }
        CommitMessageLanguage::Spanish => "Language: write the whole message in Spanish.",
        CommitMessageLanguage::English => "Language: write the whole message in English.",
    };
    let convention = match preferences.convention {
        CommitMessageConvention::FreeForm => {
            "Format: free-form subject in imperative mood; do not add a machine-readable prefix."
                .to_owned()
        }
        CommitMessageConvention::ConventionalCommits => {
            let scope = match preferences.scope {
                CommitScopeUsage::Omit => "Do not add a scope.",
                CommitScopeUsage::Optional => {
                    "Add a scope only when the staged paths make it obvious."
                }
                CommitScopeUsage::Required => {
                    "Always add a scope derived from the staged paths; use a single short word."
                }
            };
            format!(
                "Format: follow Conventional Commits as `<type>[optional scope]: <description>` with a lowercase type such as feat, fix, docs, refactor, perf, test, build, ci or chore. {scope}"
            )
        }
    };
    format!(
        "## Message preferences\n\
		- {language}\n\
		- {convention}\n\
		- Subject length: aim for {} characters or fewer; it is a guideline, so prefer a clear subject over a truncated one.\n",
        preferences.subject_max_length
    )
}

/// Plantilla editable que la interfaz puede insertar en el borrador.
#[must_use]
pub fn commit_message_template(preferences: CommitMessagePreferences) -> String {
    let preferences = preferences.normalized();
    let in_english = matches!(preferences.language, CommitMessageLanguage::English);
    let limit = preferences.subject_max_length;
    let subject = match (preferences.convention, preferences.scope) {
        (CommitMessageConvention::FreeForm, _) => {
            if in_english {
                format!("Summary in imperative mood (max {limit} characters)")
            } else {
                format!("Resumen en imperativo (máximo {limit} caracteres)")
            }
        }
        (CommitMessageConvention::ConventionalCommits, CommitScopeUsage::Omit) => {
            if in_english {
                format!("type: summary in imperative mood (max {limit} characters)")
            } else {
                format!("tipo: resumen en imperativo (máximo {limit} caracteres)")
            }
        }
        (CommitMessageConvention::ConventionalCommits, CommitScopeUsage::Optional) => {
            if in_english {
                format!("type(optional scope): summary in imperative mood (max {limit} characters)")
            } else {
                format!("tipo(alcance opcional): resumen en imperativo (máximo {limit} caracteres)")
            }
        }
        (CommitMessageConvention::ConventionalCommits, CommitScopeUsage::Required) => {
            if in_english {
                format!("type(scope): summary in imperative mood (max {limit} characters)")
            } else {
                format!("tipo(alcance): resumen en imperativo (máximo {limit} caracteres)")
            }
        }
    };
    let body = if in_english {
        "Optional body: explain why the change is needed, wrapped at 72 columns."
    } else {
        "Cuerpo opcional: explica por qué es necesario el cambio, a 72 columnas."
    };
    format!("{subject}\n\n{body}")
}

/// Resultado de pedir la plantilla manual con el borrador actual.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateApplication {
    /// El borrador está vacío: la plantilla puede escribirse directamente.
    Apply(String),
    /// Hay texto escrito: sustituirlo exige una confirmación explícita.
    ConfirmReplace(String),
}

/// Decide si la plantilla puede escribirse sin perder trabajo del usuario.
#[must_use]
pub fn plan_commit_template(
    draft: &str,
    preferences: CommitMessagePreferences,
) -> TemplateApplication {
    let template = commit_message_template(preferences);
    if draft.trim().is_empty() {
        TemplateApplication::Apply(template)
    } else {
        TemplateApplication::ConfirmReplace(template)
    }
}

/// Aviso orientativo sobre el borrador actual.
///
/// Las convenciones guían la propuesta, así que este texto nunca bloquea el
/// commit: solo describe en qué se aleja el asunto de lo configurado.
#[must_use]
pub fn commit_message_guidance(
    message: &str,
    preferences: CommitMessagePreferences,
) -> Option<String> {
    let preferences = preferences.normalized();
    let subject = message.lines().next().unwrap_or_default().trim_end();
    if subject.trim().is_empty() {
        return None;
    }
    let mut notes = Vec::new();
    let subject_length = subject.chars().count();
    if subject_length > preferences.subject_max_length as usize {
        notes.push(format!(
            "asunto de {subject_length} caracteres (orientativo ≤{})",
            preferences.subject_max_length
        ));
    }
    if preferences.scope_applies() && !matches_conventional_subject(subject) {
        notes.push("sin prefijo Conventional Commits".to_owned());
    }
    (!notes.is_empty()).then(|| format!("Aviso orientativo: {}.", notes.join("; ")))
}

/// Comprueba la forma `tipo(alcance opcional)!: descripción` sin fijar una lista de tipos.
fn matches_conventional_subject(subject: &str) -> bool {
    let Some((prefix, description)) = subject.split_once(": ") else {
        return false;
    };
    if description.trim().is_empty() {
        return false;
    }
    let prefix = prefix.strip_suffix('!').unwrap_or(prefix);
    let (kind, scope) = match prefix.split_once('(') {
        Some((kind, scope)) => match scope.strip_suffix(')') {
            Some(scope) => (kind, Some(scope)),
            None => return false,
        },
        None => (prefix, None),
    };
    let kind_is_valid = !kind.is_empty()
        && kind
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '-');
    let scope_is_valid = scope.is_none_or(|scope| !scope.trim().is_empty());
    kind_is_valid && scope_is_valid
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        CommitMessageConvention, CommitMessageLanguage, CommitMessagePreferenceOverrides,
        CommitMessagePreferences, CommitPreferenceField, CommitScopeUsage,
        DEFAULT_SUBJECT_MAX_LENGTH, MAX_SUBJECT_MAX_LENGTH, MIN_SUBJECT_MAX_LENGTH,
        PreferenceScope, PreferenceSource, TemplateApplication, commit_message_guidance,
        commit_message_template, commit_preferences_instructions, cycle_commit_preference,
        effective_commit_preferences, plan_commit_template, resolve_commit_preferences,
    };

    #[test]
    fn repository_overrides_win_over_global_values_field_by_field() {
        let global = CommitMessagePreferences {
            language: CommitMessageLanguage::Spanish,
            convention: CommitMessageConvention::FreeForm,
            scope: CommitScopeUsage::Optional,
            subject_max_length: 72,
        };
        let overrides = CommitMessagePreferenceOverrides {
            convention: Some(CommitMessageConvention::ConventionalCommits),
            subject_max_length: Some(50),
            ..CommitMessagePreferenceOverrides::default()
        };

        let effective = resolve_commit_preferences(global, Some(overrides));

        assert_eq!(effective.language.value, CommitMessageLanguage::Spanish);
        assert_eq!(effective.language.source, PreferenceSource::Global);
        assert_eq!(
            effective.convention.value,
            CommitMessageConvention::ConventionalCommits
        );
        assert_eq!(effective.convention.source, PreferenceSource::Repository);
        assert_eq!(effective.subject_max_length.value, 50);
        assert!(effective.has_repository_overrides());
    }

    #[test]
    fn repositories_without_overrides_inherit_every_global_value() {
        let global = CommitMessagePreferences {
            language: CommitMessageLanguage::English,
            ..CommitMessagePreferences::default()
        };
        let mut overrides_by_repository = HashMap::new();
        overrides_by_repository.insert(
            r"c:\repos\otro".to_owned(),
            CommitMessagePreferenceOverrides {
                language: Some(CommitMessageLanguage::Spanish),
                ..CommitMessagePreferenceOverrides::default()
            },
        );

        let effective =
            effective_commit_preferences(global, &overrides_by_repository, r"c:\repos\este");

        assert_eq!(effective.values(), global);
        assert!(!effective.has_repository_overrides());
    }

    #[test]
    fn normalizes_out_of_range_lengths_from_persisted_or_hand_edited_state() {
        let too_small = CommitMessagePreferences {
            subject_max_length: 3,
            ..CommitMessagePreferences::default()
        };
        let too_large = CommitMessagePreferences {
            subject_max_length: 4_000,
            ..CommitMessagePreferences::default()
        };
        let zero = CommitMessagePreferences {
            subject_max_length: 0,
            ..CommitMessagePreferences::default()
        };

        assert_eq!(
            too_small.normalized().subject_max_length,
            MIN_SUBJECT_MAX_LENGTH
        );
        assert_eq!(
            too_large.normalized().subject_max_length,
            MAX_SUBJECT_MAX_LENGTH
        );
        assert_eq!(
            zero.normalized().subject_max_length,
            DEFAULT_SUBJECT_MAX_LENGTH
        );
        assert_eq!(
            resolve_commit_preferences(
                CommitMessagePreferences::default(),
                Some(CommitMessagePreferenceOverrides {
                    subject_max_length: Some(1),
                    ..CommitMessagePreferenceOverrides::default()
                })
            )
            .subject_max_length
            .value,
            MIN_SUBJECT_MAX_LENGTH
        );
    }

    #[test]
    fn cycling_the_repository_layer_starts_from_the_effective_value() {
        let mut global = CommitMessagePreferences {
            language: CommitMessageLanguage::Spanish,
            ..CommitMessagePreferences::default()
        };
        let mut overrides = CommitMessagePreferenceOverrides::default();

        cycle_commit_preference(
            CommitPreferenceField::Language,
            PreferenceScope::Repository,
            &mut global,
            &mut overrides,
        );

        assert_eq!(global.language, CommitMessageLanguage::Spanish);
        assert_eq!(overrides.language, Some(CommitMessageLanguage::English));
    }

    #[test]
    fn cycling_the_global_layer_keeps_the_repository_override() {
        let mut global = CommitMessagePreferences::default();
        let mut overrides = CommitMessagePreferenceOverrides {
            language: Some(CommitMessageLanguage::Spanish),
            ..CommitMessagePreferenceOverrides::default()
        };

        cycle_commit_preference(
            CommitPreferenceField::Language,
            PreferenceScope::Global,
            &mut global,
            &mut overrides,
        );

        assert_eq!(global.language, CommitMessageLanguage::Spanish);
        assert_eq!(overrides.language, Some(CommitMessageLanguage::Spanish));
        assert_eq!(
            resolve_commit_preferences(global, Some(overrides))
                .language
                .value,
            CommitMessageLanguage::Spanish
        );
    }

    #[test]
    fn subject_length_cycles_through_the_offered_options() {
        let mut global = CommitMessagePreferences {
            subject_max_length: 100,
            ..CommitMessagePreferences::default()
        };
        let mut overrides = CommitMessagePreferenceOverrides::default();

        cycle_commit_preference(
            CommitPreferenceField::SubjectMaxLength,
            PreferenceScope::Global,
            &mut global,
            &mut overrides,
        );

        assert_eq!(global.subject_max_length, 50);
    }

    #[test]
    fn instructions_describe_language_convention_scope_and_length() {
        let instructions = commit_preferences_instructions(CommitMessagePreferences {
            language: CommitMessageLanguage::Spanish,
            convention: CommitMessageConvention::ConventionalCommits,
            scope: CommitScopeUsage::Required,
            subject_max_length: 50,
        });

        assert!(instructions.contains("write the whole message in Spanish"));
        assert!(instructions.contains("Conventional Commits"));
        assert!(instructions.contains("Always add a scope"));
        assert!(instructions.contains("50 characters or fewer"));
        assert!(instructions.contains("guideline"));
    }

    #[test]
    fn free_form_instructions_do_not_mention_scope_rules() {
        let instructions = commit_preferences_instructions(CommitMessagePreferences {
            convention: CommitMessageConvention::FreeForm,
            scope: CommitScopeUsage::Required,
            ..CommitMessagePreferences::default()
        });

        assert!(instructions.contains("free-form subject"));
        assert!(!instructions.contains("scope"));
    }

    #[test]
    fn template_follows_convention_scope_and_language() {
        let spanish = commit_message_template(CommitMessagePreferences {
            convention: CommitMessageConvention::ConventionalCommits,
            scope: CommitScopeUsage::Required,
            ..CommitMessagePreferences::default()
        });
        let english = commit_message_template(CommitMessagePreferences {
            language: CommitMessageLanguage::English,
            ..CommitMessagePreferences::default()
        });

        assert!(spanish.starts_with("tipo(alcance): resumen"));
        assert!(english.starts_with("Summary in imperative mood"));
    }

    #[test]
    fn template_never_replaces_a_written_draft_without_confirmation() {
        let preferences = CommitMessagePreferences::default();

        assert!(matches!(
            plan_commit_template("   \n\t", preferences),
            TemplateApplication::Apply(_)
        ));
        assert!(matches!(
            plan_commit_template("fix: algo a medias", preferences),
            TemplateApplication::ConfirmReplace(_)
        ));
    }

    #[test]
    fn guidance_is_advisory_and_reports_length_and_convention_gaps() {
        let preferences = CommitMessagePreferences {
            convention: CommitMessageConvention::ConventionalCommits,
            subject_max_length: 20,
            ..CommitMessagePreferences::default()
        };

        let guidance = commit_message_guidance("Corrige un error muy largo de verdad", preferences)
            .expect("debe avisar");

        assert!(guidance.contains("orientativo ≤20"));
        assert!(guidance.contains("Conventional Commits"));
        assert!(commit_message_guidance("fix: corrige algo", preferences).is_none());
        assert!(commit_message_guidance("", preferences).is_none());
    }

    #[test]
    fn guidance_accepts_conventional_subjects_with_scope_and_breaking_marker() {
        let preferences = CommitMessagePreferences {
            convention: CommitMessageConvention::ConventionalCommits,
            ..CommitMessagePreferences::default()
        };

        assert!(commit_message_guidance("feat(ui)!: cambia el panel", preferences).is_none());
        assert!(
            commit_message_guidance("feat(): sin alcance real", preferences)
                .expect("debe avisar")
                .contains("Conventional Commits")
        );
    }

    #[test]
    fn free_form_guidance_ignores_conventional_prefixes() {
        let preferences = CommitMessagePreferences::default();

        assert!(commit_message_guidance("Añade el panel de ajustes", preferences).is_none());
    }
}
