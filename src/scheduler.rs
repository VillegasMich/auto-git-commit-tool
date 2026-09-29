//! Daily scheduling in UTC: next-run computation and the sleep loop.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, SecondsFormat, TimeDelta, Utc};
use tracing::info;

use crate::clock::Clock;

/// Longest single sleep. The wall clock is re-checked after each chunk so host suspend or
/// clock adjustments can't make the service miss (or double) a run.
pub const MAX_SLEEP_CHUNK: Duration = Duration::from_secs(60);

/// The scheduled instant for `date`.
pub fn slot_on(date: NaiveDate, at: NaiveTime) -> DateTime<Utc> {
    date.and_time(at).and_utc()
}

/// Next occurrence of `at` strictly after `now`.
pub fn next_run(now: DateTime<Utc>, at: NaiveTime) -> DateTime<Utc> {
    let today = now.date_naive();
    let slot = slot_on(today, at);
    if now < slot {
        slot
    } else {
        slot_on(today.succ_opt().expect("date overflow"), at)
    }
}

/// Whether to run right after startup instead of waiting for the next slot.
///
/// With `catch_up`, a service that (re)starts after today's slot runs immediately; the daily run
/// itself is idempotent, so this is a no-op if today's commits already exist.
pub fn should_run_at_startup(
    now: DateTime<Utc>,
    at: NaiveTime,
    run_on_start: bool,
    catch_up: bool,
) -> bool {
    run_on_start || (catch_up && now >= slot_on(now.date_naive(), at))
}

/// Sleeps until `target`. Returns `false` if interrupted by shutdown.
pub fn sleep_until(clock: &dyn Clock, target: DateTime<Utc>) -> bool {
    loop {
        let now = clock.now();
        if now >= target {
            return true;
        }
        let remaining = (target - now).to_std().unwrap_or(Duration::ZERO);
        if !clock.sleep(remaining.min(MAX_SLEEP_CHUNK)) {
            return false;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Done,
    /// The job failed or left work pending; try again after `Schedule::retry_after`.
    Retry,
}

#[derive(Debug, Clone)]
pub struct Schedule {
    pub at: NaiveTime,
    pub run_immediately: bool,
    pub retry_after: Duration,
}

/// Runs `job` once per day at `schedule.at` (UTC) until shutdown.
pub fn run_forever(clock: &dyn Clock, schedule: &Schedule, mut job: impl FnMut() -> JobStatus) {
    let mut status = if schedule.run_immediately {
        info!("running immediately after startup");
        job()
    } else {
        JobStatus::Done
    };

    while !clock.shutdown_requested() {
        let now = clock.now();
        let mut wake = next_run(now, schedule.at);
        if status == JobStatus::Retry {
            let retry_at =
                now + TimeDelta::from_std(schedule.retry_after).unwrap_or(TimeDelta::MAX);
            wake = wake.min(retry_at);
        }
        info!(next_run = %wake.to_rfc3339_opts(SecondsFormat::Secs, true), "waiting for next run");
        if !sleep_until(clock, wake) {
            break;
        }
        status = job();
    }
    info!("scheduler stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::testing::FakeClock;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    fn noon() -> NaiveTime {
        NaiveTime::from_hms_opt(12, 0, 0).unwrap()
    }

    #[test]
    fn next_run_later_today() {
        assert_eq!(
            next_run(t("2026-09-28T08:00:00Z"), noon()),
            t("2026-09-28T12:00:00Z")
        );
    }

    #[test]
    fn next_run_tomorrow_when_passed_or_exact() {
        assert_eq!(
            next_run(t("2026-09-28T12:00:00Z"), noon()),
            t("2026-09-29T12:00:00Z")
        );
        assert_eq!(
            next_run(t("2026-09-28T18:30:00Z"), noon()),
            t("2026-09-29T12:00:00Z")
        );
    }

    #[test]
    fn next_run_across_month_and_year() {
        assert_eq!(
            next_run(t("2026-12-31T13:00:00Z"), noon()),
            t("2027-01-01T12:00:00Z")
        );
    }

    #[test]
    fn startup_catch_up_only_after_slot() {
        let before = t("2026-09-28T11:59:00Z");
        let after = t("2026-09-28T12:00:00Z");
        assert!(!should_run_at_startup(before, noon(), false, true));
        assert!(should_run_at_startup(after, noon(), false, true));
        assert!(!should_run_at_startup(after, noon(), false, false));
        assert!(should_run_at_startup(before, noon(), true, false));
    }

    #[test]
    fn sleep_until_uses_bounded_chunks() {
        let clock = FakeClock::at("2026-09-28T11:57:30Z");
        assert!(sleep_until(&clock, t("2026-09-28T12:00:00Z")));
        assert_eq!(clock.now(), t("2026-09-28T12:00:00Z"));
        let secs: Vec<u64> = clock.sleeps().iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [60, 60, 30]);
    }

    #[test]
    fn sleep_until_past_target_returns_immediately() {
        let clock = FakeClock::at("2026-09-28T13:00:00Z");
        assert!(sleep_until(&clock, t("2026-09-28T12:00:00Z")));
        assert!(clock.sleeps().is_empty());
    }

    fn schedule(run_immediately: bool) -> Schedule {
        Schedule {
            at: noon(),
            run_immediately,
            retry_after: Duration::from_secs(30 * 60),
        }
    }

    #[test]
    fn runs_once_per_day_at_slot() {
        // Two days of 60 s chunks, then shutdown.
        let clock = FakeClock::at("2026-09-28T11:00:00Z").interrupt_after(60 + 24 * 60 + 5);
        let mut runs = Vec::new();
        run_forever(&clock, &schedule(false), || {
            runs.push(clock.now());
            JobStatus::Done
        });
        assert_eq!(runs, [t("2026-09-28T12:00:00Z"), t("2026-09-29T12:00:00Z")]);
    }

    #[test]
    fn retries_same_day_after_failure() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z").interrupt_after(31);
        let mut runs = Vec::new();
        run_forever(&clock, &schedule(true), || {
            runs.push(clock.now());
            JobStatus::Retry
        });
        assert_eq!(runs, [t("2026-09-28T12:00:00Z"), t("2026-09-28T12:30:00Z")]);
    }
}
