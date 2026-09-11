mod branch;
mod history;
mod remote;
mod remote_freshness;
mod repository;
mod status;

pub use branch::{BranchKind, BranchReference, BranchUpstream};
pub use history::{CommitDetails, CommitId, CommitSummary, HistoryPage, Reference};
pub use remote::{Remote, RemoteOperationPlan, UpstreamState};
pub use remote_freshness::{
    Clock, DEFAULT_PERIODIC_FETCH_INTERVAL_SECS, FetchOrigin, FixedClock, PERIODIC_FETCH_INTERVALS,
    PeriodicFetchCandidate, RemoteFreshnessRecord, RemoteFreshnessTracker, SystemClock,
    format_freshness_label, format_periodic_fetch_interval_label, format_relative_age,
    next_periodic_fetch_interval, normalized_repo_key, periodic_fetch_poll_interval,
    primary_remote_label, remote_ref_fingerprint, select_periodic_fetch,
};
pub use repository::{
    AppSettings, AppState, ChangeCounters, HistorySnapshot, MutationState, OperationKind,
    RefreshCoordinator, RefreshState, RepositoryId, RepositorySession, RepositoryView,
    SshCloneMapping, ThemePreference, WorkingTreeSnapshot,
};
pub use status::{ChangeKind, ChangeSelection, FileChange, HeadState, StatusSnapshot};
