//! Startup checks: required binaries present, `gh` authenticated, account resolved.

use anyhow::{Context, Result, anyhow, bail};
use tracing::info;

use crate::clock::Clock;
use crate::exec::{Cmd, ExecError};
use crate::github::{GhCli, User};
use crate::retry::{Backoff, Fatal, RetryError, retry};

/// `git` and `gh` must both be installed. Missing tools are a fatal configuration error.
pub fn check_tools(gh: &GhCli) -> Result<()> {
    let git = Cmd::new("git")
        .arg("--version")
        .run()
        .context("`git` is required but could not be run")?;
    let gh_version = gh
        .version()
        .context("`gh` (GitHub CLI) is required but could not be run")?;
    info!(%git, gh = %gh_version, "found required tools");
    Ok(())
}

/// Resolves the authenticated account.
///
/// Rejected credentials fail immediately; anything else (e.g. no network yet right after boot)
/// is retried with backoff before giving up.
pub fn authenticate(gh: &GhCli, clock: &dyn Clock, backoff: &Backoff) -> Result<User> {
    let user = retry(clock, backoff, "gh api user", || {
        gh.current_user().map_err(|e| {
            if is_auth_rejection(&e) {
                Fatal("GitHub rejected GH_TOKEN (bad credentials)".into()).into()
            } else {
                e
            }
        })
    });
    let user = match user {
        Ok(user) => user,
        Err(RetryError::Exhausted { last, .. }) => {
            return Err(last.context("resolving GitHub user"));
        }
        Err(RetryError::Interrupted { last }) => bail!("shutdown during preflight: {last:#}"),
    };

    // Its output is not included in the error: keep anything token-related out of the logs.
    gh.auth_status()
        .map_err(|_| anyhow!("`gh auth status` failed; check that GH_TOKEN is valid"))?;
    info!(login = %user.login, id = user.id, "authenticated with GitHub");
    Ok(user)
}

fn is_auth_rejection(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ExecError>()
        .and_then(ExecError::stderr)
        .is_some_and(|s| s.contains("HTTP 401") || s.contains("Bad credentials"))
}
