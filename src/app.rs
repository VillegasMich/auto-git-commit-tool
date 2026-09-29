//! Wires config, preflight, bootstrap, the daily run and the scheduler into CLI commands.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::SecondsFormat;
use tracing::{error, info, warn};

use crate::bootstrap::{Target, ensure_clone, ensure_remote};
use crate::clock::{Clock, SystemClock};
use crate::config::Config;
use crate::github::{GhCli, GitHub, User};
use crate::preflight;
use crate::repo::GitRepo;
use crate::retry::{Backoff, RetryError, retry};
use crate::run::{RunOutcome, RunSettings, Workspace, already_ran_on, daily_run};
use crate::scheduler::{JobStatus, Schedule, next_run, run_forever, should_run_at_startup};

/// Startup retries cover e.g. a machine that boots before its network is up (~25 min total).
const STARTUP_BACKOFF: Backoff = Backoff {
    initial: Duration::from_secs(5),
    max: Duration::from_secs(120),
    attempts: 15,
};

/// When a run fails or leaves commits unpushed, try again this soon instead of tomorrow.
pub const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

struct Service {
    config: Config,
    gh: GhCli,
    clock: SystemClock,
    user: User,
    target: Target,
    settings: RunSettings,
}

impl Service {
    /// Preflight and bootstrap. Errors here are fatal and end the process with a non-zero code.
    fn start(config: Config, clock: SystemClock) -> Result<Self> {
        let gh = GhCli;
        preflight::check_tools(&gh)?;
        let user = preflight::authenticate(&gh, &clock, &STARTUP_BACKOFF)?;
        gh.setup_git()
            .context("configuring git credentials with `gh auth setup-git`")?;

        let service = Self {
            target: Target {
                owner: user.login.clone(),
                name: config.repo_name.clone(),
                clone_dir: config.clone_dir(),
            },
            settings: RunSettings::new(config.min_commits, config.max_commits),
            config,
            gh,
            clock,
            user,
        };
        match retry(
            &service.clock,
            &STARTUP_BACKOFF,
            "repository bootstrap",
            || service.bootstrap(),
        ) {
            Ok(()) => Ok(service),
            Err(RetryError::Exhausted { last, .. }) => Err(last.context("repository bootstrap")),
            Err(RetryError::Interrupted { last }) => bail!("shutdown during bootstrap: {last:#}"),
        }
    }

    fn bootstrap(&self) -> Result<()> {
        let remote = ensure_remote(&self.gh, &self.target)?;
        ensure_clone(&self.gh, &self.target, remote, self.clock.now())
    }

    fn open_repo(&self) -> Result<GitRepo> {
        let repo = GitRepo::new(self.target.clone_dir.clone(), self.config.log_file.clone());
        let name = self
            .config
            .author_name
            .as_deref()
            .unwrap_or(&self.user.login);
        let email = self
            .config
            .author_email
            .clone()
            .unwrap_or_else(|| self.user.noreply_email());
        repo.set_identity(name, &email)?;
        Ok(repo)
    }

    /// One run: re-check the remote (it may have been deleted since startup), then commit.
    fn run_once(&self) -> Result<RunOutcome> {
        if let Err(e) = self.bootstrap() {
            if !GitRepo::is_clone(&self.target.clone_dir) {
                return Err(e);
            }
            warn!(
                error = format!("{e:#}"),
                "could not verify remote repository; using local clone"
            );
        }
        let mut repo = self.open_repo()?;
        daily_run(&mut repo, &self.settings, &self.clock, &mut rand::rng())
    }

    fn job(&self) -> JobStatus {
        job_status(self.run_once())
    }
}

/// Logs a run's result and tells the scheduler whether to retry soon.
pub fn job_status(result: Result<RunOutcome>) -> JobStatus {
    match result {
        Ok(outcome) => {
            info!(?outcome, "daily run finished");
            if outcome.needs_retry() {
                JobStatus::Retry
            } else {
                JobStatus::Done
            }
        }
        Err(e) => {
            error!(error = format!("{e:#}"), "daily run failed");
            JobStatus::Retry
        }
    }
}

/// Long-running service: catch up if needed, then run once per day at `COMMIT_TIME`.
pub fn daemon(config: Config, clock: SystemClock) -> Result<()> {
    info!(
        repo = %config.repo_name,
        commit_time = %config.commit_time.format("%H:%M"),
        min = config.min_commits,
        max = config.max_commits,
        "starting daemon"
    );
    let service = Service::start(config, clock)?;
    let schedule = Schedule {
        at: service.config.commit_time,
        run_immediately: should_run_at_startup(
            service.clock.now(),
            service.config.commit_time,
            service.config.run_on_start,
            service.config.catch_up,
        ),
        retry_after: RETRY_AFTER,
    };
    run_forever(&service.clock, &schedule, || service.job());
    Ok(())
}

/// Single run now (idempotent), then exit.
pub fn once(config: Config, clock: SystemClock) -> Result<()> {
    let service = Service::start(config, clock)?;
    let outcome = service.run_once()?;
    info!(?outcome, "run finished");
    if outcome.needs_retry() {
        bail!("commits were made but could not be pushed; they will be pushed by the next run");
    }
    Ok(())
}

/// Read-only report. Never creates or modifies anything.
pub fn status(config: Config, clock: SystemClock) -> Result<()> {
    let gh = GhCli;
    preflight::check_tools(&gh)?;
    let single_attempt = Backoff {
        attempts: 1,
        ..STARTUP_BACKOFF
    };
    let user = preflight::authenticate(&gh, &clock, &single_attempt)?;
    let full_name = format!("{}/{}", user.login, config.repo_name);
    let remote = match gh.repo_visibility(&full_name) {
        Ok(Some(visibility)) => format!("{visibility:?}").to_lowercase(),
        Ok(None) => "not found (will be created on first run)".to_owned(),
        Err(e) => format!("unknown ({e:#})"),
    };
    let now = clock.now();
    let dir = config.clone_dir();

    println!("account:      {} (id {})", user.login, user.id);
    println!("repository:   {full_name} [{remote}]");
    println!(
        "schedule:     daily at {} UTC",
        config.commit_time.format("%H:%M")
    );
    if GitRepo::is_clone(&dir) {
        let repo = GitRepo::new(dir.clone(), config.log_file.clone());
        let last = repo.last_line()?;
        let ran_today = already_ran_on(last.as_deref(), now.date_naive());
        println!("clone:        {}", dir.display());
        println!("last entry:   {}", last.as_deref().unwrap_or("(none)"));
        println!(
            "today (UTC):  {} — {}",
            now.date_naive(),
            if ran_today { "done" } else { "pending" }
        );
        println!("unpushed:     {}", repo.unpushed_commits()?);
    } else {
        println!(
            "clone:        {} (missing; cloned on first run)",
            dir.display()
        );
    }
    println!(
        "next run:     {}",
        next_run(now, config.commit_time).to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    Ok(())
}

/// Validate configuration, tools and authentication.
pub fn check(config: Config, clock: SystemClock) -> Result<()> {
    let gh = GhCli;
    preflight::check_tools(&gh)?;
    let single_attempt = Backoff {
        attempts: 1,
        ..STARTUP_BACKOFF
    };
    let user = preflight::authenticate(&gh, &clock, &single_attempt)?;
    println!("ok: git and gh found, authenticated as {}", user.login);
    println!("config: {config:#?}");
    Ok(())
}
