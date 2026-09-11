/// Identificador completo de un commit.
pub type CommitId = String;

/// Página de historial obtenida desde una referencia concreta y su OID resuelto.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    pub reference: String,
    pub oid: String,
    pub commits: Vec<CommitDetails>,
}

/// Referencia decorativa asociada a un commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reference {
    Head(String),
    LocalBranch(String),
    RemoteBranch(String),
    Tag(String),
    Other(String),
}

/// Información resumida para una fila del historial.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitSummary {
    pub id: CommitId,
    pub short_id: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: i64,
    pub references: Vec<Reference>,
}

/// Información completa del commit seleccionado.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitDetails {
    pub summary: CommitSummary,
    pub body: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_at: i64,
    pub parent_ids: Vec<CommitId>,
}
