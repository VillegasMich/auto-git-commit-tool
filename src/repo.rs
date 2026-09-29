//! Thin wrappers around `git` for the local clone.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::exec::Cmd;
use crate::run::Workspace;

/// Abort HTTP transfers that stall below 1 KiB/s for a minute instead of hanging.
const GIT_NETWORK_OPTS: [&str; 4] = [
    "-c",
    "http.lowSpeedLimit=1024",
    "-c",
    "http.lowSpeedTime=60",
];

#[derive(Debug, Clone)]
pub struct GitRepo {
    dir: PathBuf,
    log_file: PathBuf,
}

impl GitRepo {
    pub fn new(dir: PathBuf, log_file: PathBuf) -> Self {
        Self { dir, log_file }
    }

    pub fn is_clone(dir: &Path) -> bool {
        dir.join(".git").is_dir()
    }

    fn git(&self) -> Cmd {
        Cmd::new("git").arg("-C").arg(&self.dir)
    }

    fn git_net(&self) -> Cmd {
        self.git().args(GIT_NETWORK_OPTS)
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join(&self.log_file)
    }

    fn git_dir(&self) -> PathBuf {
        self.dir.join(".git")
    }

    pub fn set_identity(&self, name: &str, email: &str) -> Result<()> {
        self.git().args(["config", "user.name", name]).run()?;
        self.git().args(["config", "user.email", email]).run()?;
        Ok(())
    }

    fn has_head(&self) -> bool {
        self.git()
            .args(["rev-parse", "--verify", "--quiet", "HEAD"])
            .run()
            .is_ok()
    }

    fn has_upstream(&self) -> bool {
        self.git()
            .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .run()
            .is_ok()
    }

    fn rebase_in_progress(&self) -> bool {
        let git_dir = self.git_dir();
        git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists()
    }
}

impl Workspace for GitRepo {
    /// Undo whatever an interrupted previous process (power loss, `SIGKILL`) may have left behind.
    fn prepare(&mut self) -> Result<()> {
        let lock = self.git_dir().join("index.lock");
        if lock.exists() {
            warn!(path = %lock.display(), "removing stale git index lock");
            fs::remove_file(&lock).with_context(|| format!("removing {}", lock.display()))?;
        }
        if self.rebase_in_progress() {
            warn!("aborting interrupted rebase");
            self.git().args(["rebase", "--abort"]).run()?;
        }
        if self.git_dir().join("MERGE_HEAD").exists() {
            warn!("aborting interrupted merge");
            self.git().args(["merge", "--abort"]).run()?;
        }
        if self.has_head() {
            let dirty = self
                .git()
                .args(["status", "--porcelain", "--untracked-files=no"])
                .run()?;
            if !dirty.is_empty() {
                // Only uncommitted edits are discarded; local commits are kept and pushed later.
                warn!(changes = %dirty, "discarding uncommitted changes from an interrupted run");
                self.git()
                    .args(["reset", "--hard", "--quiet", "HEAD"])
                    .run()?;
            }
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        if !self.has_upstream() {
            // Empty remote or first push not done yet: nothing to pull.
            return Ok(());
        }
        let pulled = self.git_net().args(["pull", "--rebase", "--quiet"]).run();
        if let Err(e) = pulled {
            if self.rebase_in_progress() {
                let _ = self.git().args(["rebase", "--abort"]).run();
            }
            return Err(e).context("git pull --rebase");
        }
        Ok(())
    }

    fn last_line(&self) -> Result<Option<String>> {
        let path = self.log_path();
        match fs::read_to_string(&path) {
            Ok(content) => Ok(content
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(str::to_owned)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn append_line(&mut self, line: &str) -> Result<()> {
        let path = self.log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let needs_newline = ends_without_newline(&mut file)?;
        let text = if needs_newline {
            format!("\n{line}\n")
        } else {
            format!("{line}\n")
        };
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    fn commit(&mut self, message: &str) -> Result<String> {
        self.git().arg("add").arg("--").arg(&self.log_file).run()?;
        self.git()
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                message,
            ])
            .run()?;
        Ok(self.git().args(["rev-parse", "--short", "HEAD"]).run()?)
    }

    fn push(&mut self) -> Result<()> {
        self.git_net()
            .args(["push", "--quiet", "--set-upstream", "origin", "HEAD"])
            .run()
            .context("git push")?;
        info!("pushed to origin");
        Ok(())
    }

    fn unpushed_commits(&self) -> Result<u32> {
        if !self.has_head() {
            return Ok(0);
        }
        let range = if self.has_upstream() {
            "@{u}..HEAD"
        } else {
            "HEAD"
        };
        let count = self.git().args(["rev-list", "--count", range]).run()?;
        count
            .parse()
            .with_context(|| format!("unexpected `git rev-list --count` output {count:?}"))
    }
}

fn ends_without_newline(file: &mut fs::File) -> Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] != b'\n')
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// A local bare "remote" plus a clone of it, with one initial commit (like `--add-readme`).
    fn setup() -> (TempDir, GitRepo) {
        let tmp = TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        let seed = tmp.path().join("seed");
        let clone = tmp.path().join("clone");
        let git = |args: &[&str]| Cmd::new("git").args(args).run().unwrap();

        git(&[
            "init",
            "--quiet",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ]);
        git(&["init", "--quiet", "-b", "main", seed.to_str().unwrap()]);
        fs::write(seed.join("README.md"), "# daily-log\n").unwrap();
        let s = seed.to_str().unwrap();
        git(&["-C", s, "add", "README.md"]);
        git(&[
            "-C",
            s,
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "init",
        ]);
        git(&["-C", s, "push", "-q", remote.to_str().unwrap(), "main"]);
        git(&[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ]);

        let repo = GitRepo::new(clone, PathBuf::from("activity.log"));
        repo.set_identity("Test", "test@example.com").unwrap();
        (tmp, repo)
    }

    #[test]
    fn commit_and_push_roundtrip() {
        let (_tmp, mut repo) = setup();
        assert_eq!(repo.last_line().unwrap(), None);

        repo.append_line("2026-09-28 | x | commit 1/2").unwrap();
        repo.commit("chore: daily log 2026-09-28 (1/2)").unwrap();
        repo.append_line("2026-09-28 | y | commit 2/2").unwrap();
        let sha = repo.commit("chore: daily log 2026-09-28 (2/2)").unwrap();

        assert!(!sha.is_empty());
        assert_eq!(
            repo.last_line().unwrap().as_deref(),
            Some("2026-09-28 | y | commit 2/2")
        );
        assert_eq!(repo.unpushed_commits().unwrap(), 2);
        repo.push().unwrap();
        assert_eq!(repo.unpushed_commits().unwrap(), 0);
        repo.sync().unwrap();
    }

    #[test]
    fn append_repairs_missing_trailing_newline() {
        let (_tmp, mut repo) = setup();
        fs::write(repo.log_path(), "first").unwrap();
        repo.append_line("second").unwrap();
        assert_eq!(
            fs::read_to_string(repo.log_path()).unwrap(),
            "first\nsecond\n"
        );
    }

    #[test]
    fn prepare_recovers_from_interrupted_run() {
        let (_tmp, mut repo) = setup();
        repo.append_line("committed").unwrap();
        repo.commit("c1").unwrap();
        // Crash after writing a line but before committing, leaving a stale lock behind.
        repo.append_line("half-written").unwrap();
        fs::write(repo.git_dir().join("index.lock"), "").unwrap();

        repo.prepare().unwrap();

        assert!(!repo.git_dir().join("index.lock").exists());
        assert_eq!(repo.last_line().unwrap().as_deref(), Some("committed"));
        assert_eq!(
            repo.unpushed_commits().unwrap(),
            1,
            "local commits must be kept"
        );
    }
}
