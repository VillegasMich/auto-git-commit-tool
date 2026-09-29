//! auto-git-commit-tool: a few commits per day to a private GitHub repository.
//!
//! Behavior is specified in `docs/architecture.md`; configuration in `docs/configuration.md`.

mod app;
mod bootstrap;
mod clock;
mod config;
mod exec;
mod github;
mod preflight;
mod repo;
mod retry;
mod run;
mod scheduler;
mod simulate;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use signal_hook::consts::{SIGINT, SIGTERM};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::clock::SystemClock;
use crate::config::Config;

/// Keep your GitHub contribution graph active with 1–5 daily commits to a private repository.
///
/// All configuration comes from environment variables (GH_TOKEN, REPO_NAME, COMMIT_TIME, ...);
/// see docs/configuration.md.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
    /// Run forever, committing once per day at COMMIT_TIME (UTC). Default.
    Daemon,
    /// Do today's run now (no-op if today's commits already exist), then exit.
    Once,
    /// Show account, repository, last log entry and next scheduled run.
    Status,
    /// Validate configuration, required tools and GitHub authentication.
    Check,
    /// Test run with a simulated clock that reaches COMMIT_TIME right away.
    ///
    /// Uses the real scheduler, daily run and git. By default the remote is a throwaway local
    /// repository: GitHub is never contacted and GH_TOKEN is not needed. With --github the run
    /// talks to GitHub for real (auth, create private repo, clone, push) using a separate
    /// repository. Ends by simulating a restart to show the day is not committed twice.
    Simulate(SimulateArgs),
}

#[derive(Debug, Clone, Args)]
struct SimulateArgs {
    /// Number of days (scheduled runs) to simulate.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=366))]
    days: u32,
    /// Simulated start: HH:MM (today, UTC) or RFC 3339. Default: 1 minute before COMMIT_TIME.
    /// A time after COMMIT_TIME exercises the catch-up after a missed run.
    #[arg(long, value_parser = simulate::parse_start)]
    start: Option<DateTime<Utc>>,
    /// Keep the temporary sandbox directory for inspection.
    #[arg(long)]
    keep: bool,
    /// Use DIR (must be empty or missing) as sandbox instead of a temporary one; it is kept.
    #[arg(long, value_name = "DIR")]
    sandbox: Option<PathBuf>,
    /// Talk to GitHub for real: authenticate, create the private repository if missing, clone,
    /// commit and push. Uses GH_TOKEN or your gh login.
    #[arg(long)]
    github: bool,
    /// Repository used with --github. Must differ from REPO_NAME.
    #[arg(long, value_name = "NAME", default_value = simulate::DEFAULT_GITHUB_REPO, requires = "github")]
    repo: String,
}

fn main() -> ExitCode {
    let command = Cli::parse().command.unwrap_or(Command::Daemon);
    init_logging(&command);
    let shutdown = match install_signal_handlers() {
        Ok(flag) => flag,
        Err(e) => {
            error!("{e:#}");
            return ExitCode::FAILURE;
        }
    };
    match run(command, Arc::clone(&shutdown)) {
        Ok(()) => ExitCode::SUCCESS,
        // Stopped (SIGTERM) while still starting up, e.g. waiting for the network: not a failure.
        Err(e) if shutdown.load(Ordering::Relaxed) => {
            info!(reason = format!("{e:#}"), "stopped during startup");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command, shutdown: Arc<AtomicBool>) -> Result<()> {
    if let Command::Simulate(args) = command {
        let options = simulate::Options {
            days: args.days,
            start: args.start,
            sandbox: args.sandbox,
            keep: args.keep,
            remote: if args.github {
                simulate::Remote::GitHub { repo: args.repo }
            } else {
                simulate::Remote::Local
            },
        };
        return simulate::simulate(Config::from_env_without_token()?, options, shutdown);
    }

    let config = Config::from_env()?;
    let clock = SystemClock::new(shutdown);
    match command {
        Command::Daemon => app::daemon(config, clock),
        Command::Once => app::once(config, clock),
        Command::Status => app::status(config, clock),
        Command::Check => app::check(config, clock),
        Command::Simulate(_) => unreachable!("handled above"),
    }
}

/// First SIGTERM/SIGINT requests a graceful shutdown; a second one exits immediately.
fn install_signal_handlers() -> Result<Arc<AtomicBool>> {
    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register_conditional_shutdown(signal, 1, Arc::clone(&shutdown))
            .context("installing signal handler")?;
        signal_hook::flag::register(signal, Arc::clone(&shutdown))
            .context("installing signal handler")?;
    }
    Ok(shutdown)
}

fn init_logging(command: &Command) {
    // Interactive commands print a report; keep them quiet unless RUST_LOG says otherwise.
    let default_level = match command {
        Command::Daemon | Command::Once | Command::Simulate(_) => "info",
        Command::Status | Command::Check => "warn",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stdout)
        .with_ansi(std::io::stdout().is_terminal());
    if matches!(command, Command::Simulate(_)) {
        // Wall-clock log timestamps would contradict the simulated time in the messages.
        builder.without_time().init();
    } else {
        builder.init();
    }
}
