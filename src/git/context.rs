/// Datos staged obtenidos mediante comandos Git separados y seguros.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedContextData {
    pub name_status: String,
    pub numstat: String,
    pub textual_diff: String,
    pub recent_subjects: Vec<String>,
}
