//! Wall clock + interruptible sleep, abstracted so scheduling logic can be tested with a fake.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

pub trait Clock {
    /// Current time, always UTC.
    fn now(&self) -> DateTime<Utc>;

    /// Sleeps for `duration`. Returns `false` if the sleep was cut short by a shutdown request.
    fn sleep(&self, duration: Duration) -> bool;

    fn shutdown_requested(&self) -> bool;
}

/// Real clock. Sleeps in short ticks so `SIGTERM` is honored within a fraction of a second.
#[derive(Debug, Clone)]
pub struct SystemClock {
    shutdown: Arc<AtomicBool>,
}

const TICK: Duration = Duration::from_millis(250);

impl SystemClock {
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self { shutdown }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        loop {
            if self.shutdown_requested() {
                return false;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return true;
            }
            std::thread::sleep(remaining.min(TICK));
        }
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}

/// Clock for `simulate`: sleeping advances simulated time instantly instead of waiting.
/// Time stops at `end`; a sleep that would pass it reports an interruption, like a shutdown.
#[derive(Debug)]
pub struct SimulatedClock {
    now: std::cell::Cell<DateTime<Utc>>,
    end: DateTime<Utc>,
    shutdown: Arc<AtomicBool>,
}

impl SimulatedClock {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            now: std::cell::Cell::new(start),
            end,
            shutdown,
        }
    }
}

impl Clock for SimulatedClock {
    fn now(&self) -> DateTime<Utc> {
        self.now.get()
    }

    fn sleep(&self, duration: Duration) -> bool {
        if self.shutdown_requested() {
            return false;
        }
        let target = chrono::TimeDelta::from_std(duration)
            .ok()
            .and_then(|d| self.now.get().checked_add_signed(d))
            .unwrap_or(self.end);
        if target > self.end {
            self.now.set(self.end);
            return false;
        }
        self.now.set(target);
        true
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
pub mod testing {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// Deterministic clock: `sleep` advances time instantly. Optionally simulates a shutdown
    /// request after a given number of sleeps.
    pub struct FakeClock {
        now: Cell<DateTime<Utc>>,
        sleeps: RefCell<Vec<Duration>>,
        interrupt_after: Cell<Option<usize>>,
        shutdown: Cell<bool>,
    }

    impl FakeClock {
        pub fn at(rfc3339: &str) -> Self {
            Self {
                now: Cell::new(DateTime::parse_from_rfc3339(rfc3339).unwrap().to_utc()),
                sleeps: RefCell::default(),
                interrupt_after: Cell::new(None),
                shutdown: Cell::new(false),
            }
        }

        /// The `n + 1`-th sleep call reports a shutdown.
        pub fn interrupt_after(self, n: usize) -> Self {
            self.interrupt_after.set(Some(n));
            self
        }

        pub fn sleeps(&self) -> Vec<Duration> {
            self.sleeps.borrow().clone()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            self.now.get()
        }

        fn sleep(&self, duration: Duration) -> bool {
            if self.shutdown.get() || self.interrupt_after.get() == Some(self.sleeps.borrow().len())
            {
                self.shutdown.set(true);
                return false;
            }
            self.sleeps.borrow_mut().push(duration);
            self.now
                .set(self.now.get() + chrono::TimeDelta::from_std(duration).unwrap());
            true
        }

        fn shutdown_requested(&self) -> bool {
            self.shutdown.get()
        }
    }
}
