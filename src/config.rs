//! Configuration loaded from environment variables. See `docs/configuration.md`.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use chrono::NaiveTime;
use lettre::message::Mailbox;

pub const DEFAULT_REPO_NAME: &str = "daily-log";
pub const DEFAULT_COMMIT_TIME: &str = "12:00";
pub const DEFAULT_MIN_COMMITS: u32 = 1;
pub const DEFAULT_MAX_COMMITS: u32 = 5;
pub const DEFAULT_LOG_FILE: &str = "activity.log";
pub const DEFAULT_DATA_DIR: &str = "/data";
pub const DEFAULT_SMTP_PORT: u16 = 465;
pub const DEFAULT_HEALTHCHECK_INTERVAL_MINUTES: u64 = 5;

/// Display name on notification emails whose sender address has none.
const EMAIL_SENDER_NAME: &str = "auto-git-commit-tool";

/// Upper bound on commits per day, to keep a typo from spamming the repository.
const MAX_COMMITS_LIMIT: u32 = 50;

/// Validated service configuration.
///
/// `GH_TOKEN` is required but deliberately not stored here: `gh` reads it from the environment,
/// and keeping it out of this struct guarantees it can never end up in a `Debug` log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub repo_name: String,
    pub commit_time: NaiveTime,
    pub min_commits: u32,
    pub max_commits: u32,
    pub log_file: PathBuf,
    pub data_dir: PathBuf,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub run_on_start: bool,
    pub catch_up: bool,
    /// Email notifications; `None` when `SMTP_HOST` is unset or `NOTIFY_ENABLED=false`.
    pub email: Option<EmailConfig>,
    /// Dead-man's switch pings; `None` when `HEALTHCHECK_URL` is unset or `NOTIFY_ENABLED=false`.
    pub healthcheck: Option<HealthcheckConfig>,
}

/// A value that must never be logged: `Debug` prints a placeholder instead.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub tls: SmtpTls,
    pub credentials: Option<SmtpCredentials>,
    pub from: Mailbox,
    pub to: Mailbox,
}

/// How the SMTP connection is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// TLS from the first byte (port 465).
    Implicit,
    /// Plain connection upgraded with STARTTLS, which the server must support (port 587).
    StartTls,
    /// No encryption. Only allowed for a server on this machine (e.g. a test server).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpCredentials {
    pub username: String,
    pub password: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthcheckConfig {
    /// Ping URL. Anyone holding it can report the service as alive, so it is kept secret.
    pub url: Secret,
    pub interval: Duration,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("GH_TOKEN is not set; export a GitHub token (see docs/deployment.md)")]
    MissingToken,
    #[error("{var}={value:?} is invalid: {reason}")]
    Invalid {
        var: &'static str,
        value: String,
        reason: String,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Settings only, without requiring `GH_TOKEN`: for `simulate`, which never contacts GitHub.
    pub fn from_env_without_token() -> Result<Self, ConfigError> {
        Self::parse_settings(|key| std::env::var(key).ok())
    }

    /// Builds the config from an arbitrary key lookup, so validation is testable without
    /// touching the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        if non_empty(&lookup, "GH_TOKEN").is_none() {
            return Err(ConfigError::MissingToken);
        }
        Self::parse_settings(lookup)
    }

    fn parse_settings(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |key: &str| non_empty(&lookup, key);

        let repo_name = get("REPO_NAME").unwrap_or_else(|| DEFAULT_REPO_NAME.to_owned());
        validate_repo_name(&repo_name)?;

        let commit_time = parse_time(
            "COMMIT_TIME",
            &get("COMMIT_TIME").unwrap_or_else(|| DEFAULT_COMMIT_TIME.to_owned()),
        )?;

        let min_commits = parse_count("MIN_COMMITS", get("MIN_COMMITS"), DEFAULT_MIN_COMMITS)?;
        let max_commits = parse_count("MAX_COMMITS", get("MAX_COMMITS"), DEFAULT_MAX_COMMITS)?;
        if min_commits > max_commits {
            return Err(ConfigError::Invalid {
                var: "MIN_COMMITS",
                value: min_commits.to_string(),
                reason: format!("must be <= MAX_COMMITS ({max_commits})"),
            });
        }

        let log_file =
            PathBuf::from(get("LOG_FILE").unwrap_or_else(|| DEFAULT_LOG_FILE.to_owned()));
        validate_log_file(&log_file)?;

        let data_dir =
            PathBuf::from(get("DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.to_owned()));

        Self {
            repo_name,
            commit_time,
            min_commits,
            max_commits,
            log_file,
            data_dir,
            author_name: get("GIT_AUTHOR_NAME"),
            author_email: get("GIT_AUTHOR_EMAIL"),
            run_on_start: parse_bool("RUN_ON_START", get("RUN_ON_START"), false)?,
            catch_up: parse_bool("CATCH_UP", get("CATCH_UP"), true)?,
            email: None,
            healthcheck: None,
        }
        .with_notifications(&get)
    }

    fn with_notifications(
        mut self,
        get: &impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        if parse_bool("NOTIFY_ENABLED", get("NOTIFY_ENABLED"), true)? {
            self.email = parse_email(get)?;
            self.healthcheck = parse_healthcheck(get)?;
        }
        Ok(self)
    }

    /// Where the target repository is cloned: `$DATA_DIR/$REPO_NAME`.
    pub fn clone_dir(&self) -> PathBuf {
        self.data_dir.join(&self.repo_name)
    }
}

fn non_empty(lookup: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    lookup(key)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn invalid(var: &'static str, value: &str, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        var,
        value: value.to_owned(),
        reason: reason.into(),
    }
}

pub fn validate_repo_name(name: &str) -> Result<(), ConfigError> {
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !valid_chars || name.len() > 100 || name == "." || name == ".." {
        return Err(invalid(
            "REPO_NAME",
            name,
            "use at most 100 letters, digits, '-', '_' or '.'",
        ));
    }
    Ok(())
}

fn parse_time(var: &'static str, value: &str) -> Result<NaiveTime, ConfigError> {
    NaiveTime::parse_from_str(value, "%H:%M")
        .map_err(|_| invalid(var, value, "expected HH:MM in 24-hour format (UTC)"))
}

fn parse_count(var: &'static str, value: Option<String>, default: u32) -> Result<u32, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value.parse::<u32>() {
        Ok(n) if (1..=MAX_COMMITS_LIMIT).contains(&n) => Ok(n),
        _ => Err(invalid(
            var,
            &value,
            format!("expected an integer between 1 and {MAX_COMMITS_LIMIT}"),
        )),
    }
}

fn parse_bool(
    var: &'static str,
    value: Option<String>,
    default: bool,
) -> Result<bool, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(invalid(var, &value, "expected true or false")),
    }
}

fn parse_email(get: &impl Fn(&str) -> Option<String>) -> Result<Option<EmailConfig>, ConfigError> {
    let Some(smtp_host) = get("SMTP_HOST") else {
        for var in [
            "NOTIFY_EMAIL_TO",
            "NOTIFY_EMAIL_FROM",
            "SMTP_USERNAME",
            "SMTP_PASSWORD",
        ] {
            if let Some(value) = get(var) {
                let shown = if var == "SMTP_PASSWORD" {
                    "[redacted]"
                } else {
                    &value
                };
                return Err(invalid(var, shown, "has no effect without SMTP_HOST"));
            }
        }
        return Ok(None);
    };

    let smtp_port = match get("SMTP_PORT") {
        None => DEFAULT_SMTP_PORT,
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|&port| port != 0)
            .ok_or_else(|| invalid("SMTP_PORT", &value, "expected a port number"))?,
    };

    let tls = match get("SMTP_TLS").map(|v| v.to_ascii_lowercase()).as_deref() {
        None if smtp_port == 465 => SmtpTls::Implicit,
        None => SmtpTls::StartTls,
        Some("implicit") => SmtpTls::Implicit,
        Some("starttls") => SmtpTls::StartTls,
        Some("none") if is_loopback(&smtp_host) => SmtpTls::None,
        Some("none") => {
            return Err(invalid(
                "SMTP_TLS",
                "none",
                "only allowed when SMTP_HOST is this machine (localhost, 127.0.0.1, ::1)",
            ));
        }
        Some(other) => {
            return Err(invalid(
                "SMTP_TLS",
                other,
                "expected implicit, starttls or none",
            ));
        }
    };

    let credentials = match (get("SMTP_USERNAME"), get("SMTP_PASSWORD")) {
        (Some(username), Some(password)) => Some(SmtpCredentials {
            username,
            password: Secret::new(password),
        }),
        (None, None) => None,
        (Some(username), None) => {
            return Err(invalid(
                "SMTP_USERNAME",
                &username,
                "SMTP_PASSWORD is not set",
            ));
        }
        (None, Some(_)) => {
            return Err(invalid(
                "SMTP_PASSWORD",
                "[redacted]",
                "SMTP_USERNAME is not set",
            ));
        }
    };

    let from = match get("NOTIFY_EMAIL_FROM") {
        Some(value) => parse_mailbox("NOTIFY_EMAIL_FROM", &value)?,
        None => credentials
            .as_ref()
            .and_then(|c| c.username.parse::<Mailbox>().ok())
            .ok_or_else(|| {
                invalid(
                    "NOTIFY_EMAIL_FROM",
                    "",
                    "required when SMTP_USERNAME is not an email address",
                )
            })?,
    };
    let from = match from.name {
        Some(_) => from,
        None => Mailbox::new(Some(EMAIL_SENDER_NAME.to_owned()), from.email),
    };
    let to = match get("NOTIFY_EMAIL_TO") {
        Some(value) => parse_mailbox("NOTIFY_EMAIL_TO", &value)?,
        None => Mailbox::new(None, from.email.clone()),
    };

    Ok(Some(EmailConfig {
        smtp_host,
        smtp_port,
        tls,
        credentials,
        from,
        to,
    }))
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn parse_mailbox(var: &'static str, value: &str) -> Result<Mailbox, ConfigError> {
    value
        .parse()
        .map_err(|_| invalid(var, value, "expected an email address"))
}

fn parse_healthcheck(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<Option<HealthcheckConfig>, ConfigError> {
    let interval_minutes = match get("HEALTHCHECK_INTERVAL_MINUTES") {
        None => DEFAULT_HEALTHCHECK_INTERVAL_MINUTES,
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|m| (1..=24 * 60).contains(m))
            .ok_or_else(|| {
                invalid(
                    "HEALTHCHECK_INTERVAL_MINUTES",
                    &value,
                    "expected an integer between 1 and 1440",
                )
            })?,
    };
    let Some(url) = get("HEALTHCHECK_URL") else {
        return Ok(None);
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        // The URL is secret: don't echo it back.
        return Err(invalid(
            "HEALTHCHECK_URL",
            "[redacted]",
            "expected an http(s) URL, e.g. https://hc-ping.com/<uuid>",
        ));
    }
    Ok(Some(HealthcheckConfig {
        url: Secret::new(url),
        interval: Duration::from_secs(interval_minutes * 60),
    }))
}

fn validate_log_file(path: &Path) -> Result<(), ConfigError> {
    let value = path.to_string_lossy();
    let all_normal = path.components().all(|c| matches!(c, Component::Normal(_)));
    if !all_normal {
        return Err(invalid(
            "LOG_FILE",
            &value,
            "must be a relative path inside the repository (no '..' or leading '/')",
        ));
    }
    if path.components().next() == Some(Component::Normal(".git".as_ref())) {
        return Err(invalid("LOG_FILE", &value, "must not point inside .git"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn load(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    fn with_token<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut vars = vec![("GH_TOKEN", "test-token")];
        vars.extend_from_slice(extra);
        vars
    }

    #[test]
    fn defaults() {
        let cfg = load(&with_token(&[])).unwrap();
        assert_eq!(cfg.repo_name, "daily-log");
        assert_eq!(cfg.commit_time, NaiveTime::from_hms_opt(12, 0, 0).unwrap());
        assert_eq!((cfg.min_commits, cfg.max_commits), (1, 5));
        assert_eq!(cfg.log_file, PathBuf::from("activity.log"));
        assert_eq!(cfg.clone_dir(), PathBuf::from("/data/daily-log"));
        assert!(!cfg.run_on_start);
        assert!(cfg.catch_up);
        assert_eq!(cfg.author_name, None);
    }

    #[test]
    fn token_is_required() {
        assert_eq!(load(&[]), Err(ConfigError::MissingToken));
        assert_eq!(load(&[("GH_TOKEN", "  ")]), Err(ConfigError::MissingToken));
    }

    #[test]
    fn settings_parse_without_token() {
        let map = HashMap::from([("MIN_COMMITS".to_owned(), "0".to_owned())]);
        assert!(Config::parse_settings(|k| map.get(k).cloned()).is_err());
        assert!(Config::parse_settings(|_| None).is_ok());
    }

    #[test]
    fn token_never_appears_in_debug_output() {
        let cfg = load(&[("GH_TOKEN", "ghp_supersecret")]).unwrap();
        assert!(!format!("{cfg:?}").contains("supersecret"));
    }

    #[test]
    fn overrides() {
        let cfg = load(&with_token(&[
            ("REPO_NAME", "my-log"),
            ("COMMIT_TIME", "23:45"),
            ("MIN_COMMITS", "3"),
            ("MAX_COMMITS", "3"),
            ("LOG_FILE", "logs/daily.txt"),
            ("DATA_DIR", "/tmp/x"),
            ("GIT_AUTHOR_NAME", "Me"),
            ("RUN_ON_START", "TRUE"),
            ("CATCH_UP", "no"),
        ]))
        .unwrap();
        assert_eq!(cfg.repo_name, "my-log");
        assert_eq!(cfg.commit_time, NaiveTime::from_hms_opt(23, 45, 0).unwrap());
        assert_eq!((cfg.min_commits, cfg.max_commits), (3, 3));
        assert_eq!(cfg.clone_dir(), PathBuf::from("/tmp/x/my-log"));
        assert_eq!(cfg.author_name.as_deref(), Some("Me"));
        assert!(cfg.run_on_start);
        assert!(!cfg.catch_up);
    }

    #[test]
    fn rejects_bad_values() {
        let cases = [
            ("COMMIT_TIME", "25:00"),
            ("COMMIT_TIME", "noon"),
            ("MIN_COMMITS", "0"),
            ("MAX_COMMITS", "-1"),
            ("MAX_COMMITS", "1000"),
            ("REPO_NAME", "bad/name"),
            ("REPO_NAME", ".."),
            ("LOG_FILE", "/etc/passwd"),
            ("LOG_FILE", "../escape.log"),
            ("LOG_FILE", ".git/config"),
            ("RUN_ON_START", "maybe"),
        ];
        for (var, value) in cases {
            let err = load(&with_token(&[(var, value)])).unwrap_err();
            assert!(
                matches!(err, ConfigError::Invalid { var: v, .. } if v == var),
                "{var}={value} should be rejected, got {err:?}"
            );
        }
    }

    #[test]
    fn notifications_off_by_default() {
        let cfg = load(&with_token(&[])).unwrap();
        assert_eq!((cfg.email, cfg.healthcheck), (None, None));
    }

    #[test]
    fn email_defaults_send_to_self_over_implicit_tls() {
        let cfg = load(&with_token(&[
            ("SMTP_HOST", "smtp.gmail.com"),
            ("SMTP_USERNAME", "me@gmail.com"),
            ("SMTP_PASSWORD", "app-password"),
        ]))
        .unwrap();
        let email = cfg.email.unwrap();
        assert_eq!((email.smtp_port, email.tls), (465, SmtpTls::Implicit));
        assert_eq!(
            email.from.to_string(),
            "auto-git-commit-tool <me@gmail.com>"
        );
        assert_eq!(email.to.to_string(), "me@gmail.com");
        assert_eq!(email.credentials.unwrap().username, "me@gmail.com");
    }

    #[test]
    fn email_overrides() {
        let cfg = load(&with_token(&[
            ("SMTP_HOST", "smtp.example.com"),
            ("SMTP_PORT", "587"),
            ("NOTIFY_EMAIL_FROM", "Bot <bot@example.com>"),
            ("NOTIFY_EMAIL_TO", "me@example.org"),
        ]))
        .unwrap();
        let email = cfg.email.unwrap();
        assert_eq!((email.smtp_port, email.tls), (587, SmtpTls::StartTls));
        assert_eq!(email.credentials, None);
        assert_eq!(email.from.to_string(), "Bot <bot@example.com>");
        assert_eq!(email.to.to_string(), "me@example.org");
    }

    #[test]
    fn plain_smtp_only_on_this_machine() {
        let tls = |host: &str| {
            load(&with_token(&[
                ("SMTP_HOST", host),
                ("SMTP_TLS", "none"),
                ("NOTIFY_EMAIL_FROM", "a@b.co"),
            ]))
            .map(|cfg| cfg.email.unwrap().tls)
        };
        for host in ["localhost", "127.0.0.1", "::1", "[::1]"] {
            assert_eq!(tls(host), Ok(SmtpTls::None), "{host}");
        }
        for host in ["smtp.gmail.com", "10.0.0.1", "localhost.evil.com"] {
            assert!(tls(host).is_err(), "{host}");
        }
    }

    #[test]
    fn healthcheck_settings() {
        let cfg = load(&with_token(&[
            ("HEALTHCHECK_URL", "https://hc-ping.com/abc"),
            ("HEALTHCHECK_INTERVAL_MINUTES", "10"),
        ]))
        .unwrap();
        let hc = cfg.healthcheck.unwrap();
        assert_eq!(hc.url.expose(), "https://hc-ping.com/abc");
        assert_eq!(hc.interval, Duration::from_secs(600));
    }

    #[test]
    fn notify_enabled_false_disables_everything() {
        let cfg = load(&with_token(&[
            ("NOTIFY_ENABLED", "false"),
            ("SMTP_HOST", "smtp.gmail.com"),
            ("NOTIFY_EMAIL_FROM", "me@gmail.com"),
            ("HEALTHCHECK_URL", "https://hc-ping.com/abc"),
        ]))
        .unwrap();
        assert_eq!((cfg.email, cfg.healthcheck), (None, None));
    }

    #[test]
    fn rejects_bad_notification_settings() {
        let smtp = [("SMTP_HOST", "smtp.gmail.com")];
        let cases: [(&[(&str, &str)], &str); 9] = [
            (&[("NOTIFY_EMAIL_TO", "me@gmail.com")], "NOTIFY_EMAIL_TO"),
            (&[("SMTP_PASSWORD", "pw")], "SMTP_PASSWORD"),
            (
                &[smtp[0], ("SMTP_USERNAME", "me@gmail.com")],
                "SMTP_USERNAME",
            ),
            (
                &[smtp[0], ("SMTP_USERNAME", "me"), ("SMTP_PASSWORD", "pw")],
                "NOTIFY_EMAIL_FROM",
            ),
            (
                &[smtp[0], ("NOTIFY_EMAIL_FROM", "not an email")],
                "NOTIFY_EMAIL_FROM",
            ),
            (
                &[smtp[0], ("NOTIFY_EMAIL_FROM", "a@b.c"), ("SMTP_PORT", "0")],
                "SMTP_PORT",
            ),
            (
                &[smtp[0], ("NOTIFY_EMAIL_FROM", "a@b.c"), ("SMTP_TLS", "ssl")],
                "SMTP_TLS",
            ),
            (&[("HEALTHCHECK_URL", "hc-ping.com/abc")], "HEALTHCHECK_URL"),
            (
                &[("HEALTHCHECK_INTERVAL_MINUTES", "0")],
                "HEALTHCHECK_INTERVAL_MINUTES",
            ),
        ];
        for (vars, var) in cases {
            let err = load(&with_token(vars)).unwrap_err();
            assert!(
                matches!(err, ConfigError::Invalid { var: v, .. } if v == var),
                "{vars:?} should be rejected for {var}, got {err:?}"
            );
        }
    }

    #[test]
    fn secrets_never_appear_in_debug_or_errors() {
        let cfg = load(&with_token(&[
            ("SMTP_HOST", "smtp.gmail.com"),
            ("SMTP_USERNAME", "me@gmail.com"),
            ("SMTP_PASSWORD", "pw-supersecret"),
            ("HEALTHCHECK_URL", "https://hc-ping.com/uuid-supersecret"),
        ]))
        .unwrap();
        assert!(!format!("{cfg:?}").contains("supersecret"));
        let err = load(&with_token(&[("SMTP_PASSWORD", "pw-supersecret")])).unwrap_err();
        assert!(!err.to_string().contains("supersecret"));
        let err = load(&with_token(&[(
            "HEALTHCHECK_URL",
            "ftp://uuid-supersecret",
        )]))
        .unwrap_err();
        assert!(!err.to_string().contains("supersecret"));
    }

    #[test]
    fn min_must_not_exceed_max() {
        let err = load(&with_token(&[("MIN_COMMITS", "4"), ("MAX_COMMITS", "2")])).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "MIN_COMMITS",
                ..
            }
        ));
    }
}
