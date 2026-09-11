/// Tipo de referencia de rama que se puede consultar desde Git Helper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchKind {
    Local,
    RemoteTracking,
}

/// Estado del upstream configurado para una rama local.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchUpstream {
    Configured {
        full_name: String,
        ahead: u32,
        behind: u32,
    },
    NoUpstream,
    Gone {
        full_name: String,
    },
}

/// Referencia de rama consultable sin cambiar HEAD, índice ni working tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchReference {
    pub name: String,
    pub full_name: String,
    pub oid: Option<String>,
    pub kind: BranchKind,
    pub is_active: bool,
    pub upstream: BranchUpstream,
}
