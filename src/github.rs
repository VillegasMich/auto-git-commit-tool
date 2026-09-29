//! Thin wrappers around the GitHub CLI (`gh`).
//!
//! `gh` reads `GH_TOKEN` from the environment; the token is never passed on a command line.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::exec::{Cmd, ExecError};

const REPO_DESCRIPTION: &str = "Daily activity log maintained by auto-git-commit-tool";
const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
    pub id: u64,
}

impl User {
    /// GitHub's per-account noreply address, which always counts towards contributions.
    pub fn noreply_email(&self) -> String {
        format!("{}+{}@users.noreply.github.com", self.id, self.login)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Internal,
    Public,
}

/// Repository-level GitHub operations used by bootstrap; faked in tests.
pub trait GitHub {
    /// `None` if the repository does not exist.
    fn repo_visibility(&self, full_name: &str) -> Result<Option<Visibility>>;
    /// Creates `name` under the authenticated account. Always private.
    fn create_private_repo(&self, name: &str) -> Result<()>;
    fn clone_repo(&self, full_name: &str, dest: &Path) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct GhCli;

impl GhCli {
    fn gh() -> Cmd {
        Cmd::new("gh")
    }

    pub fn version(&self) -> Result<String> {
        let out = Self::gh().arg("--version").run()?;
        Ok(out.lines().next().unwrap_or_default().to_owned())
    }

    /// Fails if `gh` has no valid credentials for github.com.
    pub fn auth_status(&self) -> Result<(), ExecError> {
        Self::gh()
            .args(["auth", "status", "--hostname", "github.com"])
            .run()
            .map(drop)
    }

    pub fn current_user(&self) -> Result<User> {
        let out = Self::gh()
            .args(["api", "user", "--jq", ".login,.id"])
            .run()?;
        parse_user(&out)
    }

    /// Configures git to use `gh` as credential helper, so `git push` over HTTPS is authenticated.
    pub fn setup_git(&self) -> Result<()> {
        Self::gh()
            .args(["auth", "setup-git", "--hostname", "github.com"])
            .run()?;
        Ok(())
    }
}

impl GitHub for GhCli {
    fn repo_visibility(&self, full_name: &str) -> Result<Option<Visibility>> {
        let result = Self::gh()
            .args([
                "repo",
                "view",
                full_name,
                "--json",
                "visibility",
                "--jq",
                ".visibility",
            ])
            .run();
        match result {
            Ok(out) => parse_visibility(&out).map(Some),
            Err(e) if e.stderr().is_some_and(is_not_found) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn create_private_repo(&self, name: &str) -> Result<()> {
        Self::gh()
            .args([
                "repo",
                "create",
                name,
                "--private",
                "--add-readme",
                "--description",
            ])
            .arg(REPO_DESCRIPTION)
            .run()
            .with_context(|| format!("creating private repository {name}"))?;
        Ok(())
    }

    /// Clones over HTTPS with plain `git` (authenticated through `gh auth setup-git`), so the
    /// result doesn't depend on the user's `gh` `git_protocol` setting.
    fn clone_repo(&self, full_name: &str, dest: &Path) -> Result<()> {
        Cmd::new("git")
            .args(["clone", "--quiet"])
            .arg(format!("https://github.com/{full_name}.git"))
            .arg(dest)
            .timeout(CLONE_TIMEOUT)
            .run()
            .with_context(|| format!("cloning {full_name} into {}", dest.display()))?;
        Ok(())
    }
}

fn is_not_found(stderr: &str) -> bool {
    stderr.contains("Could not resolve to a Repository")
}

fn parse_user(out: &str) -> Result<User> {
    let mut lines = out.lines().map(str::trim);
    let (Some(login), Some(id)) = (lines.next(), lines.next()) else {
        bail!("unexpected output from `gh api user`: {out:?}");
    };
    let id = id
        .parse()
        .with_context(|| format!("invalid user id from `gh api user`: {id:?}"))?;
    if login.is_empty() {
        bail!("empty login from `gh api user`");
    }
    Ok(User {
        login: login.to_owned(),
        id,
    })
}

fn parse_visibility(out: &str) -> Result<Visibility> {
    match out.trim() {
        "PRIVATE" => Ok(Visibility::Private),
        "INTERNAL" => Ok(Visibility::Internal),
        "PUBLIC" => Ok(Visibility::Public),
        other => bail!("unexpected repository visibility {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user() {
        let user = parse_user("octocat\n583231\n").unwrap();
        assert_eq!(
            user,
            User {
                login: "octocat".into(),
                id: 583231
            }
        );
        assert_eq!(
            user.noreply_email(),
            "583231+octocat@users.noreply.github.com"
        );
    }

    #[test]
    fn rejects_bad_user_output() {
        assert!(parse_user("").is_err());
        assert!(parse_user("octocat\nnot-a-number").is_err());
    }

    #[test]
    fn parses_visibility() {
        assert_eq!(parse_visibility("PRIVATE\n").unwrap(), Visibility::Private);
        assert_eq!(parse_visibility("PUBLIC").unwrap(), Visibility::Public);
        assert!(parse_visibility("weird").is_err());
    }

    #[test]
    fn recognizes_not_found() {
        assert!(is_not_found(
            "GraphQL: Could not resolve to a Repository with the name 'me/daily-log'. (repository)"
        ));
        assert!(!is_not_found("error connecting to api.github.com"));
    }
}
