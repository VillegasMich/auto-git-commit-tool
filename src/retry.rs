//! Retry with exponential backoff for transient failures (network, push rejected, ...).

use std::time::Duration;

use tracing::warn;

use crate::clock::Clock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
    /// Total number of attempts, including the first one.
    pub attempts: u32,
}

impl Backoff {
    /// Delay before retry number `retry` (0-based): `initial * 2^retry`, capped at `max`.
    pub fn delay(&self, retry: u32) -> Duration {
        let factor = 2u32.saturating_pow(retry);
        self.initial.saturating_mul(factor).min(self.max)
    }
}

/// Return this (via `anyhow`) from a retried operation to stop retrying immediately.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Fatal(pub String);

#[derive(Debug, thiserror::Error)]
pub enum RetryError {
    #[error("gave up after {attempts} attempts: {last:#}")]
    Exhausted { attempts: u32, last: anyhow::Error },
    #[error("interrupted by shutdown: {last:#}")]
    Interrupted { last: anyhow::Error },
}

/// Runs `op` until it succeeds, backoff is exhausted, or a shutdown is requested.
pub fn retry<T>(
    clock: &dyn Clock,
    backoff: &Backoff,
    what: &str,
    mut op: impl FnMut() -> anyhow::Result<T>,
) -> Result<T, RetryError> {
    let attempts = backoff.attempts.max(1);
    let mut attempt = 1;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(last) if attempt >= attempts || last.is::<Fatal>() => {
                return Err(RetryError::Exhausted {
                    attempts: attempt,
                    last,
                });
            }
            Err(last) => {
                let delay = backoff.delay(attempt - 1);
                warn!(
                    what,
                    attempt,
                    retry_in_secs = delay.as_secs(),
                    error = format!("{last:#}"),
                    "operation failed, will retry"
                );
                if !clock.sleep(delay) {
                    return Err(RetryError::Interrupted { last });
                }
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use anyhow::anyhow;

    use super::*;
    use crate::clock::testing::FakeClock;

    fn backoff(attempts: u32) -> Backoff {
        Backoff {
            initial: Duration::from_secs(5),
            max: Duration::from_secs(30),
            attempts,
        }
    }

    #[test]
    fn delay_doubles_and_caps() {
        let b = backoff(10);
        let delays: Vec<u64> = (0..5).map(|r| b.delay(r).as_secs()).collect();
        assert_eq!(delays, [5, 10, 20, 30, 30]);
        assert_eq!(b.delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn succeeds_after_transient_failures() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let calls = Cell::new(0);
        let result = retry(&clock, &backoff(5), "op", || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(anyhow!("flaky"))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(
            clock.sleeps(),
            [Duration::from_secs(5), Duration::from_secs(10)]
        );
    }

    #[test]
    fn gives_up_after_all_attempts() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let result: Result<(), _> = retry(&clock, &backoff(3), "op", || Err(anyhow!("down")));
        assert!(matches!(
            result,
            Err(RetryError::Exhausted { attempts: 3, .. })
        ));
        assert_eq!(clock.sleeps().len(), 2);
    }

    #[test]
    fn fatal_errors_are_not_retried() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z");
        let result: Result<(), _> = retry(&clock, &backoff(10), "op", || {
            Err(Fatal("bad token".into()).into())
        });
        assert!(matches!(
            result,
            Err(RetryError::Exhausted { attempts: 1, .. })
        ));
        assert!(clock.sleeps().is_empty());
    }

    #[test]
    fn stops_on_shutdown() {
        let clock = FakeClock::at("2026-09-28T12:00:00Z").interrupt_after(1);
        let result: Result<(), _> = retry(&clock, &backoff(10), "op", || Err(anyhow!("down")));
        assert!(matches!(result, Err(RetryError::Interrupted { .. })));
    }
}
