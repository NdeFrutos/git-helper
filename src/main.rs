#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use git_helper::{
    app::AppStartup,
    cli::{InstanceRequest, parse_internal_startup_args, try_forward_or_continue},
};

fn main() {
    let startup_args = parse_internal_startup_args(std::env::args());
    let request = startup_args
        .open_repository
        .clone()
        .map_or(InstanceRequest::Activate, InstanceRequest::OpenRepository);
    if try_forward_or_continue(&request) {
        return;
    }

    git_helper::app::run(AppStartup {
        open_repository: startup_args.open_repository,
    });
}
