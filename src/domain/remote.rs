/// Remote Git configurado en el repositorio.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Remote {
    pub name: String,
}

/// Relación de la rama local con su upstream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpstreamState {
    pub full_name: String,
    pub remote_name: String,
    pub branch_name: String,
    pub ahead: u32,
    pub behind: u32,
}

/// Operación remota resuelta y validada antes de ejecutarse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteOperationPlan {
    Fetch {
        remote_name: String,
    },
    PullFastForward,
    Push,
    SetUpstreamAndPush {
        remote_name: String,
        branch_name: String,
    },
}
