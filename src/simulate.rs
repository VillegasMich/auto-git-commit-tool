//! `simulate`: test the service end to end without waiting for `COMMIT_TIME`.
//!
//! Runs the real scheduler and the real daily run with real `git`, driven by a simulated clock
//! that starts just before `COMMIT_TIME` and skips ahead instantly instead of sleeping.
//! Afterwards a service restart is simulated to show that a day is never committed twice.
//!
//! The remote is either a throwaway local bare repository ([`Remote::Local`], GitHub is never
//! contacted) or a real private GitHub repository ([`Remote::GitHub`]: real preflight, repo
//! creation, clone and pushes). The GitHub repository must differ from `REPO_NAME`, so simulated
//! (possibly future) dates never make the real service believe a day is already done.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveTime, SecondsFormat, TimeDelta, Utc};
use tracing::{info, warn};

use crate::app::{RETRY_AFTER, job_status};
use crate::bootstrap::{Target, ensure_clone, ensure_remote};
use crate::clock::{Clock, SimulatedClock, SystemClock};
use crate::config::{Config, validate_repo_name};
use crate::exec::Cmd;
use crate::github::GhCli;
use crate::preflight;
use crate::repo::GitRepo;
use crate::retry::{Backoff, RetryError, retry};
use crate::run::{RunSettings, daily_run};
use crate::scheduler::{Schedule, next_run, run_forever, should_run_at_startup, slot_on};

pub const DEFAULT_GITHUB_REPO: &str = "auto-git-commit-simulation";

/// Real network calls (auth, repo creation, clone) are retried on the real clock.
const NETWORK_BACKOFF: Backoff = Backoff {
    initial: Duration::from_secs(3),
    max: Duration::from_secs(30),
    attempts: 4,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    /// Local bare repository inside the sandbox.
    Local,
    /// Private repository `repo` on the authenticated GitHub account (created if missing).
    GitHub { repo: String },
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Number of scheduled runs (days) to simulate.
    pub days: u32,
    /// Simulated start; defaults to one minute before today's `COMMIT_TIME`.
    pub start: Option<DateTime<Utc>>,
    /// Sandbox directory; defaults to a fresh directory in the system temp dir.
    pub sandbox: Option<PathBuf>,
    /// Keep the default sandbox directory for inspection (a given `sandbox` is always kept).
    pub keep: bool,
    pub remote: Remote,
}

/// Parses `--start`: RFC 3339 (`2026-09-28T16:59:00Z`) or `HH:MM` (today, UTC).
pub fn parse_start(value: &str) -> Result<DateTime<Utc>, String> {
    if let Ok(at) = DateTime::parse_from_rfc3339(value) {
        return Ok(at.to_utc());
    }
    NaiveTime::parse_from_str(value, "%H:%M")
        .map(|time| slot_on(Utc::now().date_naive(), time))
        .map_err(|_| "expected HH:MM (UTC, today) or an RFC 3339 timestamp".to_owned())
}

pub fn simulate(config: Config, options: Options, shutdown: Arc<AtomicBool>) -> Result<()> {
    Cmd::new("git")
        .arg("--version")
        .run()
        .context("`git` is required but could not be run")?;

    let start = options.start.unwrap_or_else(|| {
        slot_on(Utc::now().date_naive(), config.commit_time) - TimeDelta::minutes(1)
    });
    let (root, temporary) = match &options.sandbox {
        Some(dir) => (dir.clone(), false),
        None => (
            std::env::temp_dir().join(format!("auto-git-commit-simulate-{}", std::process::id())),
            true,
        ),
    };

    let result = run_simulation(
        &config,
        &options.remote,
        start,
        options.days,
        &root,
        shutdown,
    );

    if let Ok(lines) = &result {
        println!();
        println!("{} ({} lines):", config.log_file.display(), lines.len());
        for line in lines {
            println!("  {line}");
        }
    }
    if temporary && !options.keep {
        let _ = fs::remove_dir_all(&root);
    } else {
        println!("sandbox kept at {}", root.display());
    }
    result.map(drop)
}

/// A prepared clone plus, for [`Remote::GitHub`], what's needed to re-check the remote per run.
struct Setup {
    repo: GitRepo,
    github: Option<(GhCli, Target)>,
}

/// Runs the simulation in `root` (must not exist or be empty) and returns the final log file lines.
pub fn run_simulation(
    config: &Config,
    remote: &Remote,
    start: DateTime<Utc>,
    days: u32,
    root: &Path,
    shutdown: Arc<AtomicBool>,
) -> Result<Vec<String>> {
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        bail!("sandbox {} already exists and is not empty", root.display());
    }
    let Setup { mut repo, github } = match remote {
        Remote::Local => setup_local(config, root)?,
        Remote::GitHub { repo } => setup_github(config, repo, root, &shutdown)?,
    };
    let settings = RunSettings::new(config.min_commits, config.max_commits);
    let at = config.commit_time;

    let run_immediately = should_run_at_startup(start, at, config.run_on_start, config.catch_up);
    let first_run = if run_immediately {
        start
    } else {
        next_run(start, at)
    };
    // Stop one hour after the last scheduled run.
    let end = first_run + TimeDelta::days(i64::from(days.max(1)) - 1) + TimeDelta::hours(1);

    info!(
        start = %ts(start),
        end = %ts(end),
        commit_time = %at.format("%H:%M"),
        sandbox = %root.display(),
        remote = %match &github {
            Some((_, target)) => format!("github.com/{}", target.full_name()),
            None => "local bare repository (GitHub is never contacted)".to_owned(),
        },
        "simulation started with a simulated UTC clock"
    );
    let clock = SimulatedClock::new(start, end, Arc::clone(&shutdown));
    let schedule = Schedule {
        at,
        run_immediately,
        retry_after: RETRY_AFTER,
    };
    let mut job = |clock: &SimulatedClock| {
        if let Some((gh, target)) = &github {
            // Like the daemon: re-check (and recreate) the remote before each run.
            if let Err(e) = ensure_remote(gh, target) {
                warn!(
                    error = format!("{e:#}"),
                    "could not verify remote repository"
                );
            }
        }
        job_status(daily_run(&mut repo, &settings, clock, &mut rand::rng()))
    };
    run_forever(&clock, &schedule, || {
        info!(now = %ts(clock.now()), "scheduled time reached");
        job(&clock)
    });

    if !clock.shutdown_requested() {
        let restart_at = clock.now();
        info!(now = %ts(restart_at), "simulating a service restart");
        let clock = SimulatedClock::new(restart_at, restart_at + TimeDelta::hours(1), shutdown);
        if should_run_at_startup(restart_at, at, config.run_on_start, config.catch_up) {
            job(&clock);
        } else {
            info!(next_run = %ts(next_run(restart_at, at)), "would wait for the next run");
        }
    }

    let content = fs::read_to_string(repo.log_path()).unwrap_or_default();
    Ok(content.lines().map(str::to_owned).collect())
}

fn setup_local(config: &Config, root: &Path) -> Result<Setup> {
    let clone_dir = create_local_sandbox(root, &config.repo_name)?;
    let repo = GitRepo::new(clone_dir, config.log_file.clone());
    repo.set_identity(
        "auto-git-commit-tool simulation",
        "simulation@example.invalid",
    )?;
    Ok(Setup { repo, github: None })
}

/// Real preflight and bootstrap against GitHub, cloning into the sandbox.
fn setup_github(
    config: &Config,
    repo_name: &str,
    root: &Path,
    shutdown: &Arc<AtomicBool>,
) -> Result<Setup> {
    validate_repo_name(repo_name)?;
    if repo_name == config.repo_name {
        bail!(
            "the simulation repository must differ from REPO_NAME ({repo_name}): simulated dates \
             would make the real service skip real days"
        );
    }
    let gh = GhCli;
    let real_clock = SystemClock::new(Arc::clone(shutdown));
    preflight::check_tools(&gh)?;
    let user = preflight::authenticate(&gh, &real_clock, &NETWORK_BACKOFF)?;
    gh.setup_git()
        .context("configuring git credentials with `gh auth setup-git`")?;

    let target = Target {
        owner: user.login.clone(),
        name: repo_name.to_owned(),
        clone_dir: root.join(repo_name),
    };
    let bootstrap = || {
        let remote = ensure_remote(&gh, &target)?;
        ensure_clone(&gh, &target, remote, Utc::now())
    };
    match retry(
        &real_clock,
        &NETWORK_BACKOFF,
        "repository bootstrap",
        bootstrap,
    ) {
        Ok(()) => {}
        Err(RetryError::Exhausted { last, .. } | RetryError::Interrupted { last }) => {
            return Err(last.context("repository bootstrap"));
        }
    }

    let repo = GitRepo::new(target.clone_dir.clone(), config.log_file.clone());
    let name = config.author_name.as_deref().unwrap_or(&user.login);
    let email = config
        .author_email
        .clone()
        .unwrap_or_else(|| user.noreply_email());
    repo.set_identity(name, &email)?;
    Ok(Setup {
        repo,
        github: Some((gh, target)),
    })
}

/// A bare repository standing in for GitHub, plus a clone with an initial README commit
/// (like `gh repo create --add-readme`).
fn create_local_sandbox(root: &Path, repo_name: &str) -> Result<PathBuf> {
    let remote = root.join("remote.git");
    let clone = root.join(repo_name);
    fs::create_dir_all(&clone).with_context(|| format!("creating {}", clone.display()))?;
    fs::write(
        clone.join("README.md"),
        format!("# {repo_name}\n\nSimulation sandbox.\n"),
    )?;

    Cmd::new("git")
        .args(["init", "--quiet", "--bare", "-b", "main"])
        .arg(&remote)
        .run()?;
    Cmd::new("git")
        .args(["init", "--quiet", "-b", "main"])
        .arg(&clone)
        .run()?;
    let in_clone = |args: &[&str]| Cmd::new("git").arg("-C").arg(&clone).args(args).run();
    in_clone(&["add", "README.md"])?;
    in_clone(&[
        "-c",
        "user.name=simulation",
        "-c",
        "user.email=simulation@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "Initial commit",
    ])?;
    Cmd::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["remote", "add", "origin"])
        .arg(&remote)
        .run()?;
    in_clone(&["push", "--quiet", "--set-upstream", "origin", "main"])?;
    Ok(clone)
}

fn ts(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::run::line_date;

    fn config(vars: &[(&str, &str)]) -> Config {
        let mut all = vec![("GH_TOKEN", "unused")];
        all.extend_from_slice(vars);
        Config::from_lookup(|k| {
            all.iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        })
        .unwrap()
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    fn dates(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| line_date(l))
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn commits_once_per_simulated_day_and_restart_does_not_double() {
        let tmp = TempDir::new().unwrap();
        let cfg = config(&[
            ("COMMIT_TIME", "17:00"),
            ("MIN_COMMITS", "2"),
            ("MAX_COMMITS", "2"),
        ]);
        let shutdown = Arc::new(AtomicBool::new(false));
        let lines = run_simulation(
            &cfg,
            &Remote::Local,
            t("2026-09-28T16:59:00Z"),
            2,
            tmp.path(),
            shutdown,
        )
        .unwrap();

        assert_eq!(
            dates(&lines),
            ["2026-09-28", "2026-09-28", "2026-09-29", "2026-09-29"]
        );
        assert!(lines[0].contains("| 2026-09-28T17:00:00"), "{}", lines[0]);
        assert!(lines[3].ends_with("commit 2/2"));
    }

    #[test]
    fn catch_up_when_started_after_commit_time() {
        let tmp = TempDir::new().unwrap();
        let cfg = config(&[
            ("COMMIT_TIME", "12:00"),
            ("MIN_COMMITS", "1"),
            ("MAX_COMMITS", "1"),
        ]);
        let shutdown = Arc::new(AtomicBool::new(false));
        let lines = run_simulation(
            &cfg,
            &Remote::Local,
            t("2026-09-28T18:30:00Z"),
            1,
            tmp.path(),
            shutdown,
        )
        .unwrap();
        assert_eq!(dates(&lines), ["2026-09-28"]);
        assert!(lines[0].contains("| 2026-09-28T18:30:00"), "{}", lines[0]);
    }

    #[test]
    fn github_mode_refuses_the_real_repository() {
        let tmp = TempDir::new().unwrap();
        let cfg = config(&[("REPO_NAME", "daily-log")]);
        let remote = Remote::GitHub {
            repo: "daily-log".into(),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let err = run_simulation(&cfg, &remote, Utc::now(), 1, tmp.path(), shutdown).unwrap_err();
        assert!(
            err.to_string().contains("must differ from REPO_NAME"),
            "{err:#}"
        );
    }

    #[test]
    fn parses_start() {
        assert_eq!(
            parse_start("2026-09-28T16:59:00Z").unwrap(),
            t("2026-09-28T16:59:00Z")
        );
        let today = parse_start("09:15").unwrap();
        assert_eq!(today.date_naive(), Utc::now().date_naive());
        assert!(parse_start("tomorrow").is_err());
    }
}
