mod branch_parser;
mod client;
mod clone;
mod context;
mod discard;
mod error;
mod log_parser;
mod path;
mod remote_plan;
mod ssh_url;
mod status_parser;

pub use branch_parser::{BRANCH_FORMAT, parse_branch_refs};
pub use client::GitClient;
pub use clone::{CloneDestinationPlan, plan_clone_destination, remote_matches_requested_url};
pub use context::StagedContextData;
pub use discard::{DiscardMode, DiscardPlan, plan_discard};
pub use error::{GitError, classify_remote_failure};
pub use log_parser::{LOG_FORMAT, parse_log};
pub use path::{validate_existing_path_inside_repository, validate_relative_path};
pub use remote_plan::{plan_fetch, plan_pull, plan_push, resolve_upstream};
pub use ssh_url::{
    ParsedSshUrl, default_clone_destination, default_clone_root, normalize_ssh_url, parse_ssh_url,
    ssh_urls_equivalent,
};
pub use status_parser::parse_status;
