use crate::domain::{HeadState, Remote, RemoteOperationPlan, StatusSnapshot, UpstreamState};

use super::GitError;

/// Resuelve el remote mediante coincidencia de prefijo más largo.
#[must_use]
pub fn resolve_upstream(status: &StatusSnapshot, remotes: &[Remote]) -> Option<UpstreamState> {
    let upstream_name = status.upstream_name.as_ref()?;
    let remote = remotes
        .iter()
        .filter(|remote| {
            upstream_name
                .strip_prefix(&remote.name)
                .is_some_and(|suffix| suffix.starts_with('/'))
        })
        .max_by_key(|remote| remote.name.len())?;
    let branch_name = upstream_name
        .strip_prefix(&remote.name)?
        .strip_prefix('/')?
        .to_owned();

    Some(UpstreamState {
        full_name: upstream_name.clone(),
        remote_name: remote.name.clone(),
        branch_name,
        ahead: status.ahead,
        behind: status.behind,
    })
}

/// Decide qué remote usar para fetch sin adivinar entre varias opciones.
pub fn plan_fetch(
    upstream: Option<&UpstreamState>,
    remotes: &[Remote],
    selected_remote: Option<&str>,
) -> Result<RemoteOperationPlan, GitError> {
    let remote_name = if let Some(upstream) = upstream {
        upstream.remote_name.clone()
    } else if let Some(selected_remote) = selected_remote {
        require_existing_remote(remotes, selected_remote)?
            .name
            .clone()
    } else {
        match remotes {
            [] => return Err(GitError::MissingRemote),
            [remote] => remote.name.clone(),
            _ => {
                return Err(GitError::RemoteSelectionRequired {
                    remotes: remotes.iter().map(|remote| remote.name.clone()).collect(),
                });
            }
        }
    };
    validate_reference_argument(&remote_name)?;
    Ok(RemoteOperationPlan::Fetch { remote_name })
}

/// Valida que pull disponga de un upstream.
pub fn plan_pull(upstream: Option<&UpstreamState>) -> Result<RemoteOperationPlan, GitError> {
    upstream
        .map(|_| RemoteOperationPlan::PullFastForward)
        .ok_or(GitError::MissingUpstream)
}

/// Decide si push puede reutilizar upstream o necesita publicarlo.
pub fn plan_push(
    head: &HeadState,
    upstream: Option<&UpstreamState>,
    remotes: &[Remote],
    selected_remote: Option<&str>,
) -> Result<RemoteOperationPlan, GitError> {
    let branch_name = match head {
        HeadState::Branch { name, oid: Some(_) } => name,
        HeadState::Branch { oid: None, .. } | HeadState::Unborn => {
            return Err(GitError::UnbornHead);
        }
        HeadState::Detached { .. } => return Err(GitError::DetachedHead),
    };
    if upstream.is_some() {
        return Ok(RemoteOperationPlan::Push);
    }

    let remote_name = if let Some(selected_remote) = selected_remote {
        require_existing_remote(remotes, selected_remote)?
            .name
            .clone()
    } else {
        match remotes {
            [] => return Err(GitError::MissingRemote),
            [remote] => remote.name.clone(),
            _ => {
                return Err(GitError::RemoteSelectionRequired {
                    remotes: remotes.iter().map(|remote| remote.name.clone()).collect(),
                });
            }
        }
    };
    validate_reference_argument(&remote_name)?;
    validate_reference_argument(branch_name)?;
    Ok(RemoteOperationPlan::SetUpstreamAndPush {
        remote_name,
        branch_name: branch_name.clone(),
    })
}

pub(super) fn validate_reference_argument(value: &str) -> Result<(), GitError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(|character| character == '\0')
    {
        Err(GitError::InvalidReferenceName {
            value: value.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn require_existing_remote<'a>(
    remotes: &'a [Remote],
    selected_remote: &str,
) -> Result<&'a Remote, GitError> {
    remotes
        .iter()
        .find(|remote| remote.name == selected_remote)
        .ok_or_else(|| GitError::InvalidReferenceName {
            value: selected_remote.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use crate::domain::{HeadState, Remote, RemoteOperationPlan, StatusSnapshot};

    use super::{GitError, plan_fetch, plan_push, resolve_upstream};

    #[test]
    fn resolves_remote_names_that_contain_slashes() {
        let status = StatusSnapshot {
            upstream_name: Some("team/origin/main".to_owned()),
            ahead: 2,
            behind: 1,
            ..StatusSnapshot::default()
        };
        let remotes = [
            Remote {
                name: "team".to_owned(),
            },
            Remote {
                name: "team/origin".to_owned(),
            },
        ];

        let upstream = resolve_upstream(&status, &remotes).expect("debe resolver upstream");

        assert_eq!(upstream.remote_name, "team/origin");
        assert_eq!(upstream.branch_name, "main");
    }

    #[test]
    fn requires_selection_when_fetch_has_multiple_remotes() {
        let remotes = [
            Remote {
                name: "origin".to_owned(),
            },
            Remote {
                name: "upstream".to_owned(),
            },
        ];

        assert!(plan_fetch(None, &remotes, None).is_err());
    }

    #[test]
    fn plans_first_push_without_force() {
        let remotes = [Remote {
            name: "origin".to_owned(),
        }];
        let head = HeadState::Branch {
            name: "feature/demo".to_owned(),
            oid: Some("abc".to_owned()),
        };

        let plan = plan_push(&head, None, &remotes, None).expect("debe crear el plan");

        assert_eq!(
            plan,
            RemoteOperationPlan::SetUpstreamAndPush {
                remote_name: "origin".to_owned(),
                branch_name: "feature/demo".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_push_from_an_unborn_branch() {
        let remotes = [Remote {
            name: "origin".to_owned(),
        }];
        let head = HeadState::Branch {
            name: "main".to_owned(),
            oid: None,
        };

        assert!(matches!(
            plan_push(&head, None, &remotes, None),
            Err(GitError::UnbornHead)
        ));
    }
}
