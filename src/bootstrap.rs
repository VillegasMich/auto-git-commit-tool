//! Repository bootstrap: make sure the private repo exists on GitHub and is cloned locally.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use tracing::{info, warn};

use crate::github::{GitHub, Visibility};
use crate::repo::GitRepo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub owner: String,
    pub name: String,
    pub clone_dir: PathBuf,
}

impl Target {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteState {
    Existing,
    Created,
}

/// Creates the repository (always private) if it doesn't exist.
pub fn ensure_remote(gh: &impl GitHub, target: &Target) -> Result<RemoteState> {
    let repo = target.full_name();
    match gh.repo_visibility(&repo)? {
        Some(Visibility::Private) => Ok(RemoteState::Existing),
        Some(visibility) => {
            warn!(%repo, ?visibility, "repository exists but is not private; using it as-is");
            Ok(RemoteState::Existing)
        }
        None => {
            info!(%repo, "repository not found; creating it as private");
            gh.create_private_repo(&target.name)?;
            Ok(RemoteState::Created)
        }
    }
}

/// Clones the repository if there is no local clone yet.
///
/// If the remote was just (re)created, an existing clone belongs to a deleted repository: it is
/// moved aside (not deleted) and a fresh clone is made.
pub fn ensure_clone(
    gh: &impl GitHub,
    target: &Target,
    remote: RemoteState,
    now: DateTime<Utc>,
) -> Result<()> {
    let dir = &target.clone_dir;
    if GitRepo::is_clone(dir) {
        if remote == RemoteState::Existing {
            return Ok(());
        }
        let orphan = orphan_path(dir, now);
        warn!(
            from = %dir.display(),
            to = %orphan.display(),
            "remote repository was recreated; moving the stale clone aside"
        );
        fs::rename(dir, &orphan)
            .with_context(|| format!("moving {} to {}", dir.display(), orphan.display()))?;
    } else if dir.exists() && fs::read_dir(dir)?.next().is_some() {
        bail!(
            "{} exists but is not a git clone; refusing to overwrite it",
            dir.display()
        );
    }

    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    info!(repo = %target.full_name(), dir = %dir.display(), "cloning repository");
    gh.clone_repo(&target.full_name(), dir)
}

fn orphan_path(dir: &std::path::Path, now: DateTime<Utc>) -> PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.with_file_name(format!("{name}.orphaned-{}", now.format("%Y%m%dT%H%M%SZ")))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

    #[derive(Default)]
    struct FakeGitHub {
        visibility: Option<Visibility>,
        calls: RefCell<Vec<String>>,
    }

    impl GitHub for FakeGitHub {
        fn repo_visibility(&self, full_name: &str) -> Result<Option<Visibility>> {
            self.calls.borrow_mut().push(format!("view {full_name}"));
            Ok(self.visibility)
        }

        fn create_private_repo(&self, name: &str) -> Result<()> {
            self.calls.borrow_mut().push(format!("create {name}"));
            Ok(())
        }

        fn clone_repo(&self, full_name: &str, dest: &Path) -> Result<()> {
            self.calls.borrow_mut().push(format!("clone {full_name}"));
            fs::create_dir_all(dest.join(".git"))?;
            Ok(())
        }
    }

    fn target(tmp: &TempDir) -> Target {
        Target {
            owner: "me".into(),
            name: "daily-log".into(),
            clone_dir: tmp.path().join("data").join("daily-log"),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-28T12:00:00Z")
            .unwrap()
            .to_utc()
    }

    #[test]
    fn creates_missing_repo_as_private() {
        let tmp = TempDir::new().unwrap();
        let gh = FakeGitHub::default();
        let state = ensure_remote(&gh, &target(&tmp)).unwrap();
        assert_eq!(state, RemoteState::Created);
        assert_eq!(
            *gh.calls.borrow(),
            ["view me/daily-log", "create daily-log"]
        );
    }

    #[test]
    fn reuses_existing_repo() {
        let tmp = TempDir::new().unwrap();
        for visibility in [Visibility::Private, Visibility::Public] {
            let gh = FakeGitHub {
                visibility: Some(visibility),
                ..Default::default()
            };
            assert_eq!(
                ensure_remote(&gh, &target(&tmp)).unwrap(),
                RemoteState::Existing
            );
            assert_eq!(gh.calls.borrow().len(), 1, "must not create");
        }
    }

    #[test]
    fn clones_when_missing_and_skips_when_present() {
        let tmp = TempDir::new().unwrap();
        let t = target(&tmp);
        let gh = FakeGitHub::default();
        ensure_clone(&gh, &t, RemoteState::Existing, now()).unwrap();
        ensure_clone(&gh, &t, RemoteState::Existing, now()).unwrap();
        assert_eq!(*gh.calls.borrow(), ["clone me/daily-log"]);
    }

    #[test]
    fn stale_clone_is_moved_aside_when_repo_recreated() {
        let tmp = TempDir::new().unwrap();
        let t = target(&tmp);
        fs::create_dir_all(t.clone_dir.join(".git")).unwrap();
        fs::write(t.clone_dir.join("activity.log"), "old\n").unwrap();

        let gh = FakeGitHub::default();
        ensure_clone(&gh, &t, RemoteState::Created, now()).unwrap();

        let orphan = tmp.path().join("data/daily-log.orphaned-20260928T120000Z");
        assert!(orphan.join("activity.log").exists());
        assert!(!t.clone_dir.join("activity.log").exists());
        assert_eq!(*gh.calls.borrow(), ["clone me/daily-log"]);
    }

    #[test]
    fn refuses_to_clobber_non_git_directory() {
        let tmp = TempDir::new().unwrap();
        let t = target(&tmp);
        fs::create_dir_all(&t.clone_dir).unwrap();
        fs::write(t.clone_dir.join("important.txt"), "keep me").unwrap();
        assert!(ensure_clone(&FakeGitHub::default(), &t, RemoteState::Existing, now()).is_err());
    }
}
