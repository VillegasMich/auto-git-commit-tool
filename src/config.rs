//! Configuration loaded from environment variables. See `docs/configuration.md`.

use std::path::{Component, Path, PathBuf};

use chrono::NaiveTime;

pub const DEFAULT_REPO_NAME: &str = "daily-log";
pub const DEFAULT_COMMIT_TIME: &str = "12:00";
pub const DEFAULT_MIN_COMMITS: u32 = 1;
pub const DEFAULT_MAX_COMMITS: u32 = 5;
pub const DEFAULT_LOG_FILE: &str = "activity.log";
pub const DEFAULT_DATA_DIR: &str = "/data";

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

        Ok(Self {
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
        })
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
