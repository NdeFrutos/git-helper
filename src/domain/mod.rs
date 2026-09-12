mod branch;
mod commit_preferences;
mod history;
mod remote;
mod remote_freshness;
mod repository;
mod status;

pub use branch::{BranchKind, BranchReference, BranchUpstream};
pub use commit_preferences::{
    CommitMessageConvention, CommitMessageLanguage, CommitMessagePreferenceOverrides,
    CommitMessagePreferences, CommitPreferenceField, CommitScopeUsage, DEFAULT_SUBJECT_MAX_LENGTH,
    EffectiveCommitPreferences, MAX_SUBJECT_MAX_LENGTH, MIN_SUBJECT_MAX_LENGTH, PreferenceScope,
    PreferenceSource, ResolvedPreference, SUBJECT_MAX_LENGTH_OPTIONS, TemplateApplication,
    commit_message_guidance, commit_message_template, commit_preferences_instructions,
    cycle_commit_preference, effective_commit_preferences, next_subject_max_length,
    normalize_subject_max_length, plan_commit_template, resolve_commit_preferences,
};
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
