//! Email notifications: one per daily run, one on a clean shutdown.
//!
//! Kept quiet on purpose: a successful day sends exactly one email, a failing day reports the
//! failure once (not on every 30-minute retry) and once more when the pending commits are pushed.
//! Every subject starts with [`SUBJECT_PREFIX`] and the UTC date, so a mail filter can label them
//! and they are easy to search. Sending is best effort: failures are logged, never fatal.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::{DateTime, NaiveDate, SecondsFormat, TimeDelta, Utc};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use tracing::{info, warn};

use crate::clock::Clock;
use crate::config::{EmailConfig, SmtpTls};
use crate::retry::{Backoff, Fatal, retry};
use crate::run::{Commit, PushResult, RunOutcome};

pub const SUBJECT_PREFIX: &str = "[auto-git-commit]";

const SMTP_TIMEOUT: Duration = Duration::from_secs(20);

const SEND_BACKOFF: Backoff = Backoff {
    initial: Duration::from_secs(10),
    max: Duration::from_secs(60),
    attempts: 3,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub subject: String,
    pub body: String,
}

/// Delivers a message. Implemented by [`SmtpMailer`]; faked in tests.
pub trait Mailer {
    fn send(&self, message: &Message) -> Result<()>;
}

/// Plain-text email over SMTP, encrypted as configured by `SMTP_TLS`.
pub struct SmtpMailer {
    transport: SmtpTransport,
    from: Mailbox,
    to: Mailbox,
}

impl SmtpMailer {
    pub fn new(config: &EmailConfig) -> Result<Self> {
        let builder = match config.tls {
            SmtpTls::Implicit => SmtpTransport::relay(&config.smtp_host),
            SmtpTls::StartTls => SmtpTransport::starttls_relay(&config.smtp_host),
            SmtpTls::None => Ok(SmtpTransport::builder_dangerous(&config.smtp_host)),
        }
        .with_context(|| format!("invalid SMTP_HOST {:?}", config.smtp_host))?
        .port(config.smtp_port)
        .timeout(Some(SMTP_TIMEOUT));
        let builder = match &config.credentials {
            Some(c) => builder.credentials(Credentials::new(
                c.username.clone(),
                c.password.expose().to_owned(),
            )),
            None => builder,
        };
        Ok(Self {
            transport: builder.build(),
            from: config.from.clone(),
            to: config.to.clone(),
        })
    }

    /// Connects (TLS + login) without sending anything.
    pub fn test_connection(&self) -> Result<()> {
        if self.transport.test_connection()? {
            Ok(())
        } else {
            anyhow::bail!("SMTP server did not accept the connection")
        }
    }
}

impl Mailer for SmtpMailer {
    fn send(&self, message: &Message) -> Result<()> {
        let email = lettre::Message::builder()
            .from(self.from.clone())
            .to(self.to.clone())
            .subject(&message.subject)
            .header(ContentType::TEXT_PLAIN)
            .body(message.body.clone())
            .context("building email")?;
        self.transport.send(&email).map_err(|e| {
            // Bad credentials or a rejected address won't fix themselves: don't retry.
            if e.is_permanent() {
                anyhow::Error::new(Fatal(format!("SMTP server rejected the email: {e}")))
            } else {
                anyhow::Error::new(e).context("sending email")
            }
        })?;
        Ok(())
    }
}

/// Which installation a notification comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub host: String,
    /// `owner/name` on GitHub.
    pub repo: String,
    pub heartbeat: bool,
}

impl Installation {
    fn repo_url(&self) -> String {
        format!("https://github.com/{}", self.repo)
    }
}

/// Turns run results and shutdowns into emails, applying the "not noisy" rules.
pub struct Notifier {
    mailer: Option<Box<dyn Mailer>>,
    installation: Installation,
    started_at: DateTime<Utc>,
    /// Day for which a failure was already reported.
    failure_reported_on: Cell<Option<NaiveDate>>,
    /// Subject of the last run notification, repeated in the shutdown email.
    last_run: RefCell<Option<String>>,
    /// Emails that could not be sent.
    failures: Cell<u32>,
}

impl Notifier {
    /// `mailer: None` disables email; every method is then a no-op.
    pub fn new(
        mailer: Option<Box<dyn Mailer>>,
        installation: Installation,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            mailer,
            installation,
            started_at,
            failure_reported_on: Cell::new(None),
            last_run: RefCell::new(None),
            failures: Cell::new(0),
        }
    }

    pub fn run_finished(&self, clock: &dyn Clock, date: NaiveDate, result: &Result<RunOutcome>) {
        let failed = match result {
            Err(_) => true,
            Ok(outcome) => outcome.needs_retry(),
        };
        let reported = self.failure_reported_on.get() == Some(date);
        let Some(message) = run_message(&self.installation, date, result, reported) else {
            return;
        };
        if failed {
            self.failure_reported_on.set(Some(date));
        }
        *self.last_run.borrow_mut() = Some(message.subject.clone());
        self.deliver(clock, &message);
    }

    /// Reports a clean shutdown (SIGTERM/SIGINT). `next_run` is the run that will now be missed
    /// unless the service is back in time.
    pub fn stopped(&self, clock: &dyn Clock, next_run: DateTime<Utc>) {
        let message = stop_message(
            &self.installation,
            self.started_at,
            clock.now(),
            next_run,
            self.last_run.borrow().as_deref(),
        );
        self.deliver(clock, &message);
    }

    fn deliver(&self, clock: &dyn Clock, message: &Message) {
        let Some(mailer) = &self.mailer else {
            return;
        };
        match retry(clock, &SEND_BACKOFF, "send email", || mailer.send(message)) {
            Ok(()) => info!(subject = %message.subject, "notification sent"),
            Err(e) => {
                warn!(error = %e, subject = %message.subject, "could not send notification");
                self.failures.set(self.failures.get() + 1);
            }
        }
    }

    /// Number of emails that could not be sent so far.
    pub fn failures(&self) -> u32 {
        self.failures.get()
    }
}

fn subject(date: NaiveDate, what: &str) -> String {
    format!("{SUBJECT_PREFIX} {date}: {what}")
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The email for a run result, or `None` if it isn't worth one: nothing happened, or a failure
/// already reported today (`failure_reported`).
pub fn run_message(
    ctx: &Installation,
    date: NaiveDate,
    result: &Result<RunOutcome>,
    failure_reported: bool,
) -> Option<Message> {
    let (what, mut body) = match result {
        Err(_)
        | Ok(RunOutcome::AlreadyDone {
            push: PushResult::Failed,
        }) if failure_reported => {
            return None;
        }
        Ok(RunOutcome::Committed {
            push: PushResult::Failed,
            ..
        }) if failure_reported => {
            return None;
        }
        Err(e) => (
            "daily run failed".to_owned(),
            format!(
                "The daily run for {} failed:\n\n    {e:#}\n\n\
                 It is retried every 30 minutes. You get no further email about this failure \
                 today, only one when the day's commits are made or pushed.\n",
                ctx.repo
            ),
        ),
        Ok(RunOutcome::AlreadyDone {
            push: PushResult::Pushed,
        }) => (
            "pending commits pushed".to_owned(),
            format!(
                "Commits that could not be pushed earlier are now on GitHub: {}\n",
                ctx.repo_url()
            ),
        ),
        Ok(RunOutcome::AlreadyDone {
            push: PushResult::Failed,
        }) => (
            "push failed".to_owned(),
            format!(
                "Today's commits exist locally but could not be pushed to {}. \
                 Retrying every 30 minutes; you get one more email once they are pushed.\n",
                ctx.repo
            ),
        ),
        Ok(RunOutcome::AlreadyDone { .. }) => return None,
        Ok(RunOutcome::Committed {
            commits,
            planned,
            push,
        }) => {
            let n = commits.len();
            let made = if n == *planned as usize {
                plural(n, "commit")
            } else {
                format!("{n} of {planned} commits")
            };
            let (what, status) = match push {
                PushResult::Pushed | PushResult::NothingToPush => (
                    format!("{made} pushed"),
                    format!("{made} pushed to {}.", ctx.repo),
                ),
                PushResult::Failed => (
                    format!("{made} made, push failed"),
                    format!(
                        "{made} made but not pushed to {}. Retrying every 30 minutes; \
                         you get one more email once they are pushed.",
                        ctx.repo
                    ),
                ),
                PushResult::Interrupted => (
                    format!("{made} made, push interrupted"),
                    format!(
                        "{made} made; the service was stopped before they reached {}. \
                         They are pushed when it starts again.",
                        ctx.repo
                    ),
                ),
            };
            (what, format!("{status}\n\n{}", commit_list(ctx, commits)))
        }
    };
    let _ = write!(body, "\nHost: {}\n", ctx.host);
    Some(Message {
        subject: subject(date, &what),
        body,
    })
}

fn commit_list(ctx: &Installation, commits: &[Commit]) -> String {
    let mut list = String::new();
    for commit in commits {
        let _ = writeln!(
            list,
            "  {}  {}\n           {}/commit/{}",
            commit.sha,
            commit.line,
            ctx.repo_url(),
            commit.sha
        );
    }
    list
}

pub fn stop_message(
    ctx: &Installation,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
    next_run: DateTime<Utc>,
    last_run: Option<&str>,
) -> Message {
    let ts = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut body = format!(
        "The service on {host} was asked to stop (systemctl stop, docker stop, reboot or \
         shutdown) and exited cleanly.\n\n\
         Stopped at:    {stopped}\n\
         Running since: {started} ({uptime})\n\
         Last run:      {last}\n\
         Next run:      {next} (catches up on start if missed)\n\
         Repository:    {url}\n",
        host = ctx.host,
        stopped = ts(now),
        started = ts(started_at),
        uptime = format_duration(now - started_at),
        last = last_run.unwrap_or("none since start"),
        next = ts(next_run),
        url = ctx.repo_url(),
    );
    if ctx.heartbeat {
        body.push_str(
            "\nIf it does not come back, the healthcheck will report it as down once its grace \
             period runs out.\n",
        );
    }
    Message {
        subject: subject(
            now.date_naive(),
            &format!("service stopped on {}", ctx.host),
        ),
        body,
    }
}

/// `2d 3h 4m`, `5h 0m`, `12m`.
fn format_duration(d: TimeDelta) -> String {
    let minutes = d.num_minutes().max(0);
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, hours) {
        (0, 0) => format!("{mins}m"),
        (0, _) => format!("{hours}h {mins}m"),
        _ => format!("{days}d {hours}h {mins}m"),
    }
}

/// The machine's host name, for telling several installations apart.
pub fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown host".to_owned())
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use anyhow::anyhow;

    use super::*;
    use crate::clock::testing::FakeClock;

    fn ctx() -> Installation {
        Installation {
            host: "pi".to_owned(),
            repo: "me/daily-log".to_owned(),
            heartbeat: true,
        }
    }

    fn date() -> NaiveDate {
        "2026-10-03".parse().unwrap()
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    fn committed(n: u32, planned: u32, push: PushResult) -> Result<RunOutcome> {
        Ok(RunOutcome::Committed {
            commits: (1..=n)
                .map(|i| Commit {
                    sha: format!("abc000{i}"),
                    line: format!(
                        "2026-10-03 | 2026-10-03T12:00:0{i}.000000Z | commit {i}/{planned}"
                    ),
                })
                .collect(),
            planned,
            push,
        })
    }

    fn subject_of(result: &Result<RunOutcome>, reported: bool) -> Option<String> {
        run_message(&ctx(), date(), result, reported).map(|m| m.subject)
    }

    #[test]
    fn success_lists_commits_with_links() {
        let message =
            run_message(&ctx(), date(), &committed(3, 3, PushResult::Pushed), false).unwrap();
        assert_eq!(
            message.subject,
            "[auto-git-commit] 2026-10-03: 3 commits pushed"
        );
        assert!(
            message
                .body
                .starts_with("3 commits pushed to me/daily-log.")
        );
        assert!(message.body.contains(
            "  abc0002  2026-10-03 | 2026-10-03T12:00:02.000000Z | commit 2/3\n           \
             https://github.com/me/daily-log/commit/abc0002"
        ));
        assert!(message.body.ends_with("Host: pi\n"));
    }

    #[test]
    fn subjects_per_outcome() {
        let cases = [
            (committed(1, 1, PushResult::Pushed), "1 commit pushed"),
            (
                committed(2, 4, PushResult::Interrupted),
                "2 of 4 commits made, push interrupted",
            ),
            (
                committed(2, 2, PushResult::Failed),
                "2 commits made, push failed",
            ),
            (
                Ok(RunOutcome::AlreadyDone {
                    push: PushResult::Pushed,
                }),
                "pending commits pushed",
            ),
            (
                Ok(RunOutcome::AlreadyDone {
                    push: PushResult::Failed,
                }),
                "push failed",
            ),
            (Err(anyhow!("clone failed")), "daily run failed"),
        ];
        for (result, what) in cases {
            assert_eq!(
                subject_of(&result, false),
                Some(format!("[auto-git-commit] 2026-10-03: {what}")),
            );
        }
    }

    #[test]
    fn quiet_when_nothing_happened_or_failure_already_reported() {
        let nothing = [PushResult::NothingToPush, PushResult::Interrupted];
        for push in nothing {
            assert_eq!(
                subject_of(&Ok(RunOutcome::AlreadyDone { push }), false),
                None
            );
        }
        assert_eq!(subject_of(&Err(anyhow!("x")), true), None);
        assert_eq!(subject_of(&committed(2, 2, PushResult::Failed), true), None);
        let still_failing = Ok(RunOutcome::AlreadyDone {
            push: PushResult::Failed,
        });
        assert_eq!(subject_of(&still_failing, true), None);
        // Good news is always reported.
        let pushed = Ok(RunOutcome::AlreadyDone {
            push: PushResult::Pushed,
        });
        assert!(subject_of(&pushed, true).is_some());
    }

    #[test]
    fn stop_message_summarizes_state() {
        let message = stop_message(
            &ctx(),
            t("2026-10-01T09:30:00Z"),
            t("2026-10-03T12:45:10Z"),
            t("2026-10-04T12:00:00Z"),
            Some("[auto-git-commit] 2026-10-03: 3 commits pushed"),
        );
        assert_eq!(
            message.subject,
            "[auto-git-commit] 2026-10-03: service stopped on pi"
        );
        assert!(
            message
                .body
                .contains("Running since: 2026-10-01T09:30:00Z (2d 3h 15m)")
        );
        assert!(
            message
                .body
                .contains("Last run:      [auto-git-commit] 2026-10-03: 3 commits")
        );
        assert!(message.body.contains("Next run:      2026-10-04T12:00:00Z"));
        assert!(message.body.contains("healthcheck"));
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(TimeDelta::seconds(59)), "0m");
        assert_eq!(format_duration(TimeDelta::minutes(61)), "1h 1m");
        assert_eq!(
            format_duration(TimeDelta::minutes(1440 * 3 + 5)),
            "3d 0h 5m"
        );
    }

    /// Records sent subjects; fails the first `failures` sends.
    #[derive(Default)]
    struct FakeMailer {
        sent: Rc<RefCell<Vec<String>>>,
        failures: Cell<u32>,
    }

    impl Mailer for FakeMailer {
        fn send(&self, message: &Message) -> Result<()> {
            if self.failures.get() > 0 {
                self.failures.set(self.failures.get() - 1);
                anyhow::bail!("smtp down");
            }
            self.sent.borrow_mut().push(message.subject.clone());
            Ok(())
        }
    }

    fn notifier(failures: u32) -> (Notifier, Rc<RefCell<Vec<String>>>) {
        let mailer = FakeMailer {
            failures: Cell::new(failures),
            ..Default::default()
        };
        let sent = Rc::clone(&mailer.sent);
        let notifier = Notifier::new(Some(Box::new(mailer)), ctx(), t("2026-10-03T00:00:00Z"));
        (notifier, sent)
    }

    #[test]
    fn failing_day_sends_two_emails_not_one_per_retry() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z");
        let (notifier, sent) = notifier(0);
        notifier.run_finished(&clock, date(), &committed(2, 2, PushResult::Failed));
        for _ in 0..5 {
            let retry = Ok(RunOutcome::AlreadyDone {
                push: PushResult::Failed,
            });
            notifier.run_finished(&clock, date(), &retry);
            notifier.run_finished(&clock, date(), &Err(anyhow!("network down")));
        }
        let pushed = Ok(RunOutcome::AlreadyDone {
            push: PushResult::Pushed,
        });
        notifier.run_finished(&clock, date(), &pushed);
        // A failure on a new day is reported again.
        let tomorrow = date().succ_opt().unwrap();
        notifier.run_finished(&clock, tomorrow, &Err(anyhow!("network down")));
        assert_eq!(
            *sent.borrow(),
            [
                "[auto-git-commit] 2026-10-03: 2 commits made, push failed",
                "[auto-git-commit] 2026-10-03: pending commits pushed",
                "[auto-git-commit] 2026-10-04: daily run failed",
            ]
        );
    }

    #[test]
    fn failed_send_is_retried() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z");
        let (notifier, sent) = notifier(1);
        notifier.run_finished(&clock, date(), &committed(1, 1, PushResult::Pushed));
        notifier.stopped(&clock, t("2026-10-04T12:00:00Z"));
        assert_eq!(sent.borrow().len(), 2);
        assert_eq!(notifier.failures(), 0);
        assert_eq!(clock.sleeps(), [SEND_BACKOFF.initial]);
    }

    #[test]
    fn undeliverable_email_is_counted() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z");
        let (notifier, sent) = notifier(u32::MAX);
        notifier.run_finished(&clock, date(), &committed(1, 1, PushResult::Pushed));
        assert!(sent.borrow().is_empty());
        assert_eq!(notifier.failures(), 1);
        assert_eq!(clock.sleeps().len(), SEND_BACKOFF.attempts as usize - 1);
    }

    #[test]
    fn disabled_notifier_sends_nothing() {
        let clock = FakeClock::at("2026-10-03T12:00:00Z");
        let notifier = Notifier::new(None, ctx(), clock.now());
        notifier.run_finished(&clock, date(), &Err(anyhow!("x")));
        notifier.stopped(&clock, clock.now());
        assert!(clock.sleeps().is_empty());
    }
}
