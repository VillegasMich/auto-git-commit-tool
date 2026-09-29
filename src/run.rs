//! One daily run: pick `n`, append `n` lines with `n` commits, push once.
//!
//! The run is idempotent: if the last line of the log file is already dated today (UTC), no new
//! commits are made — only pending (unpushed) commits from an earlier attempt are pushed.

use std::ops::RangeInclusive;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use rand::{Rng, RngExt};
use tracing::{error, info, warn};

use crate::clock::Clock;
use crate::retry::{Backoff, RetryError, retry};

/// Operations on the local clone. Implemented by [`crate::repo::GitRepo`]; faked in tests.
pub trait Workspace {
    /// Local-only recovery: bring the working copy to a clean, consistent state.
    fn prepare(&mut self) -> Result<()>;
    /// Fetch and rebase onto the remote.
    fn sync(&mut self) -> Result<()>;
    fn last_line(&self) -> Result<Option<String>>;
    fn append_line(&mut self, line: &str) -> Result<()>;
    /// Stage the log file and commit; returns the new commit's short SHA.
    fn commit(&mut self, message: &str) -> Result<String>;
    fn push(&mut self) -> Result<()>;
    fn unpushed_commits(&self) -> Result<u32>;
}

#[derive(Debug, Clone)]
pub struct RunSettings {
    pub min_commits: u32,
    pub max_commits: u32,
    /// Pause between commits, in seconds, so commit timestamps are distinct.
    pub jitter_secs: RangeInclusive<u64>,
    pub sync_backoff: Backoff,
    pub push_backoff: Backoff,
}

impl RunSettings {
    pub fn new(min_commits: u32, max_commits: u32) -> Self {
        Self {
            min_commits,
            max_commits,
            jitter_secs: 2..=8,
            // Syncing is best effort: without it we still commit locally and push later.
            sync_backoff: Backoff {
                initial: Duration::from_secs(5),
                max: Duration::from_secs(30),
                attempts: 4,
            },
            // ~50 minutes in total before giving up until the next retry.
            push_backoff: Backoff {
                initial: Duration::from_secs(5),
                max: Duration::from_secs(10 * 60),
                attempts: 12,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    NothingToPush,
    Pushed,
    /// Commits stay local and are pushed by the next run.
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    AlreadyDone {
        push: PushResult,
    },
    Committed {
        made: u32,
        planned: u32,
        push: PushResult,
    },
}

impl RunOutcome {
    pub fn push(&self) -> PushResult {
        match self {
            Self::AlreadyDone { push } | Self::Committed { push, .. } => *push,
        }
    }

    /// Commits are waiting to be pushed; the scheduler should try again soon.
    pub fn needs_retry(&self) -> bool {
        self.push() == PushResult::Failed
    }
}

/// One line of the log file: `<date> | <RFC 3339 UTC, µs> | commit <i>/<n>`.
pub fn format_line(date: NaiveDate, at: DateTime<Utc>, i: u32, n: u32) -> String {
    format!(
        "{date} | {} | commit {i}/{n}",
        at.to_rfc3339_opts(SecondsFormat::Micros, true)
    )
}

pub fn commit_message(date: NaiveDate, i: u32, n: u32) -> String {
    format!("chore: daily log {date} ({i}/{n})")
}

/// Date field of a log line, if it parses.
pub fn line_date(line: &str) -> Option<NaiveDate> {
    line.split('|').next()?.trim().parse().ok()
}

/// "Already ran today" detection from the last line of the log file.
pub fn already_ran_on(last_line: Option<&str>, date: NaiveDate) -> bool {
    last_line.and_then(line_date) == Some(date)
}

pub fn daily_run(
    ws: &mut impl Workspace,
    settings: &RunSettings,
    clock: &dyn Clock,
    rng: &mut impl Rng,
) -> Result<RunOutcome> {
    ws.prepare()?;
    if let Err(e) = retry(clock, &settings.sync_backoff, "git pull", || ws.sync()) {
        warn!(error = %e, "could not sync with remote; continuing with local state");
    }

    let today = clock.now().date_naive();
    if already_ran_on(ws.last_line()?.as_deref(), today) {
        info!(date = %today, "already committed today, skipping");
        let push = push_pending(ws, settings, clock)?;
        return Ok(RunOutcome::AlreadyDone { push });
    }

    let planned = rng.random_range(settings.min_commits..=settings.max_commits);
    info!(date = %today, planned, "starting daily run");

    let mut made = 0;
    for i in 1..=planned {
        if i > 1 {
            let pause = Duration::from_secs(rng.random_range(settings.jitter_secs.clone()));
            if !clock.sleep(pause) {
                break;
            }
        }
        let line = format_line(today, clock.now(), i, planned);
        ws.append_line(&line)?;
        let sha = ws.commit(&commit_message(today, i, planned))?;
        info!(%sha, %line, "committed");
        made += 1;
    }

    let push = if clock.shutdown_requested() {
        warn!(
            made,
            planned, "shutdown requested; pushing what was committed"
        );
        push_once(ws)
    } else {
        push_pending(ws, settings, clock)?
    };
    Ok(RunOutcome::Committed {
        made,
        planned,
        push,
    })
}

fn push_pending(
    ws: &mut impl Workspace,
    settings: &RunSettings,
    clock: &dyn Clock,
) -> Result<PushResult> {
    let pending = ws.unpushed_commits()?;
    if pending == 0 {
        return Ok(PushResult::NothingToPush);
    }
    info!(pending, "pushing commits");
    let result = retry(clock, &settings.push_backoff, "git push", || {
        let pushed = ws.push();
        if pushed.is_err() {
            // Most likely a non-fast-forward: rebase onto the remote before the next attempt.
            let _ = ws.sync();
        }
        pushed
    });
    Ok(match result {
        Ok(()) => {
            info!(pending, "push succeeded");
            PushResult::Pushed
        }
        Err(e @ RetryError::Exhausted { .. }) => {
            error!(error = %e, "push failed; commits stay local and will be pushed later");
            PushResult::Failed
        }
        Err(RetryError::Interrupted { .. }) => PushResult::Interrupted,
    })
}

fn push_once(ws: &mut impl Workspace) -> PushResult {
    match ws.push() {
        Ok(()) => PushResult::Pushed,
        Err(e) => {
            warn!(
                error = format!("{e:#}"),
                "push before shutdown failed; will retry on next start"
            );
            PushResult::Interrupted
        }
    }
}

#[cfg(test)]
pub mod testing {
    use anyhow::{anyhow, bail};

    use super::*;

    /// In-memory workspace. `lines` is the log file; `unpushed` counts local-only commits.
    #[derive(Debug, Default)]
    pub struct FakeWorkspace {
        pub lines: Vec<String>,
        pub messages: Vec<String>,
        pub unpushed: u32,
        pub push_failures: u32,
        pub sync_fails: bool,
        pub pushes: u32,
        pub prepared: bool,
    }

    impl Workspace for FakeWorkspace {
        fn prepare(&mut self) -> Result<()> {
            self.prepared = true;
            Ok(())
        }

        fn sync(&mut self) -> Result<()> {
            if self.sync_fails {
                bail!("network down");
            }
            Ok(())
        }

        fn last_line(&self) -> Result<Option<String>> {
            Ok(self.lines.last().cloned())
        }

        fn append_line(&mut self, line: &str) -> Result<()> {
            self.lines.push(line.to_owned());
            Ok(())
        }

        fn commit(&mut self, message: &str) -> Result<String> {
            self.messages.push(message.to_owned());
            self.unpushed += 1;
            Ok(format!("{:07x}", self.messages.len()))
        }

        fn push(&mut self) -> Result<()> {
            if self.push_failures > 0 {
                self.push_failures -= 1;
                return Err(anyhow!("push rejected"));
            }
            self.pushes += 1;
            self.unpushed = 0;
            Ok(())
        }

        fn unpushed_commits(&self) -> Result<u32> {
            Ok(self.unpushed)
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::testing::FakeWorkspace;
    use super::*;
    use crate::clock::testing::FakeClock;

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn rng() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    #[test]
    fn line_format_matches_spec() {
        let at = DateTime::parse_from_rfc3339("2026-09-28T12:00:02.905114Z")
            .unwrap()
            .to_utc();
        assert_eq!(
            format_line(date("2026-09-28"), at, 2, 3),
            "2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3"
        );
        assert_eq!(
            commit_message(date("2026-09-28"), 2, 3),
            "chore: daily log 2026-09-28 (2/3)"
        );
    }

    #[test]
    fn detects_previous_run() {
        let today = date("2026-09-28");
        let line = "2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3";
        assert!(already_ran_on(Some(line), today));
        assert!(!already_ran_on(Some(line), date("2026-09-29")));
        assert!(!already_ran_on(None, today));
        assert!(!already_ran_on(Some("# daily-log"), today));
        assert!(!already_ran_on(Some(""), today));
    }

    #[test]
    fn commits_between_min_and_max_and_pushes_once() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let mut ws = FakeWorkspace::default();
        let outcome = daily_run(&mut ws, &RunSettings::new(1, 5), &clock, &mut rng()).unwrap();

        let RunOutcome::Committed {
            made,
            planned,
            push,
        } = outcome
        else {
            panic!("expected commits, got {outcome:?}");
        };
        assert!((1..=5).contains(&planned));
        assert_eq!(made, planned);
        assert_eq!(push, PushResult::Pushed);
        assert_eq!(ws.pushes, 1);
        assert!(ws.prepared);
        assert_eq!(ws.lines.len(), planned as usize);
        for (idx, line) in ws.lines.iter().enumerate() {
            assert!(line.starts_with("2026-09-28 | 2026-09-28T12:00:"), "{line}");
            assert!(
                line.ends_with(&format!("commit {}/{planned}", idx + 1)),
                "{line}"
            );
        }
        // Distinct timestamps: one jitter pause between consecutive commits.
        assert_eq!(clock.sleeps().len(), planned as usize - 1);
    }

    #[test]
    fn fixed_count_when_min_equals_max() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let mut ws = FakeWorkspace::default();
        daily_run(&mut ws, &RunSettings::new(3, 3), &clock, &mut rng()).unwrap();
        assert_eq!(ws.messages.len(), 3);
    }

    #[test]
    fn second_run_same_day_is_a_no_op() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let mut ws = FakeWorkspace::default();
        let settings = RunSettings::new(2, 2);
        daily_run(&mut ws, &settings, &clock, &mut rng()).unwrap();
        let outcome = daily_run(&mut ws, &settings, &clock, &mut rng()).unwrap();
        assert_eq!(
            outcome,
            RunOutcome::AlreadyDone {
                push: PushResult::NothingToPush
            }
        );
        assert_eq!(ws.messages.len(), 2);
    }

    #[test]
    fn next_day_runs_again() {
        let mut ws = FakeWorkspace {
            lines: vec!["2026-09-27 | 2026-09-27T12:00:00.000000Z | commit 1/1".into()],
            ..Default::default()
        };
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let outcome = daily_run(&mut ws, &RunSettings::new(1, 1), &clock, &mut rng()).unwrap();
        assert!(matches!(outcome, RunOutcome::Committed { made: 1, .. }));
    }

    #[test]
    fn recovery_pushes_commits_left_by_a_crash() {
        // Machine went down after committing but before pushing.
        let mut ws = FakeWorkspace {
            lines: vec!["2026-09-28 | 2026-09-28T12:00:00.000000Z | commit 1/2".into()],
            unpushed: 1,
            ..Default::default()
        };
        let clock = FakeClock::at("2026-09-28T15:00:00Z");
        let outcome = daily_run(&mut ws, &RunSettings::new(1, 5), &clock, &mut rng()).unwrap();
        assert_eq!(
            outcome,
            RunOutcome::AlreadyDone {
                push: PushResult::Pushed
            }
        );
        assert_eq!(ws.unpushed, 0);
        assert!(ws.messages.is_empty());
    }

    #[test]
    fn commits_locally_when_offline_and_reports_failed_push() {
        let mut ws = FakeWorkspace {
            sync_fails: true,
            push_failures: u32::MAX,
            ..Default::default()
        };
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let outcome = daily_run(&mut ws, &RunSettings::new(2, 2), &clock, &mut rng()).unwrap();
        assert_eq!(
            outcome,
            RunOutcome::Committed {
                made: 2,
                planned: 2,
                push: PushResult::Failed
            }
        );
        assert!(outcome.needs_retry());
        assert_eq!(ws.unpushed, 2);
    }

    #[test]
    fn push_retries_until_accepted() {
        let mut ws = FakeWorkspace {
            push_failures: 2,
            ..Default::default()
        };
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let outcome = daily_run(&mut ws, &RunSettings::new(1, 1), &clock, &mut rng()).unwrap();
        assert_eq!(outcome.push(), PushResult::Pushed);
        assert!(!outcome.needs_retry());
    }

    #[test]
    fn shutdown_mid_batch_stops_and_pushes_what_exists() {
        let mut ws = FakeWorkspace::default();
        // First jitter pause is interrupted.
        let clock = FakeClock::at("2026-09-28T12:00:00Z").interrupt_after(0);
        let outcome = daily_run(&mut ws, &RunSettings::new(4, 4), &clock, &mut rng()).unwrap();
        assert_eq!(
            outcome,
            RunOutcome::Committed {
                made: 1,
                planned: 4,
                push: PushResult::Pushed
            }
        );
    }
}
