//! Wires config, preflight, bootstrap, the daily run and the scheduler into CLI commands.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::SecondsFormat;
use tracing::{error, info, warn};

use crate::bootstrap::{Target, ensure_clone, ensure_remote};
use crate::clock::{Clock, SystemClock};
use crate::config::Config;
use crate::github::{GhCli, GitHub, User};
use crate::heartbeat::{self, HttpPinger};
use crate::notify::{self, Installation, Mailer, Notifier, SmtpMailer};
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
    notifier: Notifier,
}

impl Service {
    /// Preflight and bootstrap. Errors here are fatal and end the process with a non-zero code.
    fn start(config: Config, clock: SystemClock) -> Result<Self> {
        let gh = GhCli;
        preflight::check_tools(&gh)?;
        let user = preflight::authenticate(&gh, &clock, &STARTUP_BACKOFF)?;
        gh.setup_git()
            .context("configuring git credentials with `gh auth setup-git`")?;

        let target = Target {
            owner: user.login.clone(),
            name: config.repo_name.clone(),
            clone_dir: config.clone_dir(),
        };
        let notifier = notifier(&config, &target, &clock)?;
        let service = Self {
            target,
            notifier,
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

    /// One run, followed by its notification.
    fn run_and_notify(&self) -> Result<RunOutcome> {
        let date = self.clock.now().date_naive();
        let result = self.run_once();
        self.notifier.run_finished(&self.clock, date, &result);
        result
    }

    fn job(&self) -> JobStatus {
        job_status(self.run_and_notify())
    }
}

fn notifier(config: &Config, target: &Target, clock: &SystemClock) -> Result<Notifier> {
    let mailer = match &config.email {
        Some(email) => {
            info!(smtp_host = %email.smtp_host, "email notifications enabled");
            Some(Box::new(SmtpMailer::new(email)?) as Box<dyn Mailer>)
        }
        None => None,
    };
    let installation = Installation {
        host: notify::host_name(),
        repo: target.full_name(),
        heartbeat: config.healthcheck.is_some(),
    };
    Ok(Notifier::new(mailer, installation, clock.now()))
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
    if let Some(healthcheck) = &service.config.healthcheck {
        heartbeat::spawn(healthcheck, service.clock.clone())?;
    }
    run_forever(&service.clock, &schedule, || service.job());
    // `run_forever` only returns on SIGTERM/SIGINT.
    let next = next_run(service.clock.now(), service.config.commit_time);
    service.notifier.stopped(&service.clock, next);
    Ok(())
}

/// Single run now (idempotent), then exit.
pub fn once(config: Config, clock: SystemClock) -> Result<()> {
    let service = Service::start(config, clock)?;
    let outcome = service.run_and_notify()?;
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
    match &config.email {
        Some(email) => {
            SmtpMailer::new(email)?.test_connection().with_context(|| {
                format!(
                    "connecting to SMTP server {}:{}",
                    email.smtp_host, email.smtp_port
                )
            })?;
            println!(
                "ok: logged in to SMTP server {}:{}; notifications go to {}",
                email.smtp_host, email.smtp_port, email.to
            );
        }
        None => println!("email notifications: disabled"),
    }
    match &config.healthcheck {
        Some(h) => println!(
            "healthcheck: ping every {} min (not pinged by check; use notify-test)",
            h.interval.as_secs() / 60
        ),
        None => println!("healthcheck: disabled"),
    }
    println!("config: {config:#?}");
    Ok(())
}

/// Sends a test email and one healthcheck ping. Doesn't need GitHub.
pub fn notify_test(config: Config, clock: SystemClock) -> Result<()> {
    if config.email.is_none() && config.healthcheck.is_none() {
        bail!(
            "no notifications configured: set SMTP_HOST and/or HEALTHCHECK_URL (and NOTIFY_ENABLED is not false)"
        );
    }
    let mut failed = false;
    if let Some(email) = &config.email {
        let now = clock.now();
        let message = notify::Message {
            subject: format!(
                "{} {}: test notification",
                notify::SUBJECT_PREFIX,
                now.date_naive()
            ),
            body: format!(
                "Notifications from auto-git-commit-tool on {} work.\n\nSent at {}.\n",
                notify::host_name(),
                now.to_rfc3339_opts(SecondsFormat::Secs, true)
            ),
        };
        match SmtpMailer::new(email)?.send(&message) {
            Ok(()) => println!("ok: test email sent to {}", email.to),
            Err(e) => {
                println!("error: sending test email: {e:#}");
                failed = true;
            }
        }
    }
    if let Some(healthcheck) = &config.healthcheck {
        match HttpPinger::new(healthcheck).ping() {
            Ok(()) => println!("ok: healthcheck pinged"),
            Err(e) => {
                println!("error: {e:#}");
                failed = true;
            }
        }
    }
    if failed {
        bail!("some notifications could not be sent");
    }
    Ok(())
}
