//! End-to-end tests: run the built binary's `simulate` command and check the resulting log file
//! (and the remote it was pushed to) against the expected result.
//!
//! - Offline tests always run: the remote is a local bare repository.
//! - The GitHub test only runs with `E2E_GITHUB=1`. It creates/uses a private repository
//!   (`E2E_REPO`, default `auto-git-commit-e2e`) on the authenticated account (GH_TOKEN or the
//!   gh login) and writes a unique log file per run, so runs never interfere with each other.
//!
//! ```bash
//! cargo test --test e2e                       # offline only
//! E2E_GITHUB=1 cargo test --test e2e          # + real GitHub
//! ```

use std::path::Path;
use std::process::{Command, Output};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use tempfile::TempDir;

const SETTINGS: [&str; 10] = [
    "REPO_NAME",
    "COMMIT_TIME",
    "MIN_COMMITS",
    "MAX_COMMITS",
    "LOG_FILE",
    "DATA_DIR",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "RUN_ON_START",
    "CATCH_UP",
];

/// The binary with a clean, explicit configuration (the developer's own env is ignored).
fn simulate(vars: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_auto-git-commit-tool"));
    for key in SETTINGS {
        cmd.env_remove(key);
    }
    cmd.env("RUST_LOG", "info").env("NO_COLOR", "1");
    for (key, value) in vars {
        cmd.env(key, value);
    }
    let output = cmd.arg("simulate").args(args).output().expect("run binary");
    assert!(
        output.status.success(),
        "simulate failed ({})\n--- stdout\n{}\n--- stderr\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn git(args: &[&str]) -> String {
    let out = Command::new("git").args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn t(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().to_utc()
}

#[derive(Debug)]
struct Line {
    date: NaiveDate,
    at: DateTime<Utc>,
    i: u32,
    n: u32,
}

/// `<YYYY-MM-DD> | <RFC 3339 µs UTC> | commit <i>/<n>`
fn parse_line(line: &str) -> Line {
    let parts: Vec<&str> = line.split(" | ").collect();
    assert_eq!(parts.len(), 3, "bad line {line:?}");
    let timestamp = parts[1];
    assert!(
        timestamp.ends_with('Z') && timestamp.split('.').nth(1).is_some_and(|f| f.len() == 7),
        "timestamp must be UTC with microseconds: {line:?}"
    );
    let (i, n) = parts[2]
        .strip_prefix("commit ")
        .and_then(|c| c.split_once('/'))
        .unwrap_or_else(|| panic!("bad commit field in {line:?}"));
    Line {
        date: parts[0].parse().unwrap(),
        at: t(timestamp),
        i: i.parse().unwrap(),
        n: n.parse().unwrap(),
    }
}

/// Expected result: for each `(date, first commit time)`, exactly `per_day` lines numbered
/// `1/n..n/n`, the first at the given time and the next ones 2–8 s apart.
fn assert_log(content: &str, days: &[(&str, &str)], per_day: u32) {
    let lines: Vec<Line> = content.lines().map(parse_line).collect();
    assert_eq!(
        lines.len(),
        days.len() * per_day as usize,
        "unexpected number of lines:\n{content}"
    );
    for ((date, first), day) in days.iter().zip(lines.chunks(per_day as usize)) {
        assert_eq!(
            day[0].at,
            t(first),
            "first commit time of {date}:\n{content}"
        );
        for (idx, line) in day.iter().enumerate() {
            assert_eq!(line.date, date.parse::<NaiveDate>().unwrap(), "{content}");
            assert_eq!((line.i, line.n), (idx as u32 + 1, per_day), "{content}");
            if idx > 0 {
                let gap = line.at - day[idx - 1].at;
                assert!(
                    (TimeDelta::seconds(2)..=TimeDelta::seconds(8)).contains(&gap),
                    "gap {gap} between commits not in 2..=8 s:\n{content}"
                );
            }
        }
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn offline_simulation_matches_expected_log() {
    let sandbox = TempDir::new().unwrap();
    let dir = sandbox.path().join("sim");
    let output = simulate(
        &[
            ("COMMIT_TIME", "12:00"),
            ("MIN_COMMITS", "3"),
            ("MAX_COMMITS", "3"),
        ],
        &[
            "--days",
            "3",
            "--start",
            "2030-01-01T11:59:00Z",
            "--sandbox",
            dir.to_str().unwrap(),
        ],
    );

    let local = read(&dir.join("daily-log/activity.log"));
    assert_log(
        &local,
        &[
            ("2030-01-01", "2030-01-01T12:00:00Z"),
            ("2030-01-02", "2030-01-02T12:00:00Z"),
            ("2030-01-03", "2030-01-03T12:00:00Z"),
        ],
        3,
    );

    // Everything was pushed: the "remote" has the same file and one commit per line.
    let remote = dir.join("remote.git");
    let remote = remote.to_str().unwrap();
    assert_eq!(
        git(&["--git-dir", remote, "show", "main:activity.log"]),
        local
    );
    assert_eq!(
        git(&["--git-dir", remote, "rev-list", "--count", "main"]).trim(),
        "10"
    );
    let subjects = git(&[
        "--git-dir",
        remote,
        "log",
        "--reverse",
        "--format=%s",
        "main",
    ]);
    assert_eq!(
        subjects.lines().nth(1),
        Some("chore: daily log 2030-01-01 (1/3)")
    );

    // The simulated restart after the last run must not commit again.
    assert!(
        stdout(&output).contains("already committed today"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn offline_catch_up_after_missed_commit_time() {
    let sandbox = TempDir::new().unwrap();
    let dir = sandbox.path().join("sim");
    simulate(
        &[
            ("COMMIT_TIME", "12:00"),
            ("MIN_COMMITS", "2"),
            ("MAX_COMMITS", "2"),
        ],
        &[
            "--start",
            "2030-01-01T18:30:00Z",
            "--sandbox",
            dir.to_str().unwrap(),
        ],
    );
    assert_log(
        &read(&dir.join("daily-log/activity.log")),
        &[("2030-01-01", "2030-01-01T18:30:00Z")],
        2,
    );
}

#[test]
fn offline_without_catch_up_waits_for_next_day() {
    let sandbox = TempDir::new().unwrap();
    let dir = sandbox.path().join("sim");
    simulate(
        &[
            ("COMMIT_TIME", "12:00"),
            ("MIN_COMMITS", "1"),
            ("MAX_COMMITS", "1"),
            ("CATCH_UP", "false"),
        ],
        &[
            "--start",
            "2030-01-01T18:30:00Z",
            "--sandbox",
            dir.to_str().unwrap(),
        ],
    );
    assert_log(
        &read(&dir.join("daily-log/activity.log")),
        &[("2030-01-02", "2030-01-02T12:00:00Z")],
        1,
    );
}

#[test]
fn github_simulation_matches_expected_log() {
    if std::env::var("E2E_GITHUB").as_deref() != Ok("1") {
        eprintln!("skipped: set E2E_GITHUB=1 to run the end-to-end test against GitHub");
        return;
    }
    let repo = std::env::var("E2E_REPO").unwrap_or_else(|_| "auto-git-commit-e2e".to_owned());
    // Unique per run, so repeated and concurrent runs (local + CI) start from an empty log.
    let log_file = format!(
        "e2e/{}-{}.log",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        std::process::id()
    );
    let sandbox = TempDir::new().unwrap();
    let dir = sandbox.path().join("sim");

    let output = simulate(
        &[
            ("COMMIT_TIME", "12:00"),
            ("MIN_COMMITS", "2"),
            ("MAX_COMMITS", "2"),
            ("LOG_FILE", &log_file),
        ],
        &[
            "--github",
            "--repo",
            &repo,
            "--days",
            "2",
            "--start",
            "2030-01-01T11:59:00Z",
            "--sandbox",
            dir.to_str().unwrap(),
        ],
    );

    let local = read(&dir.join(&repo).join(&log_file));
    assert_log(
        &local,
        &[
            ("2030-01-01", "2030-01-01T12:00:00Z"),
            ("2030-01-02", "2030-01-02T12:00:00Z"),
        ],
        2,
    );
    assert!(
        stdout(&output).contains("already committed today"),
        "{}",
        stdout(&output)
    );

    // Check GitHub itself: the repository is private and has exactly the pushed file.
    // Retried briefly: the API can lag a moment behind a push.
    let gh = |args: &[&str]| {
        let mut last = String::new();
        for _ in 0..5 {
            let out = Command::new("gh").args(args).output().expect("run gh");
            if out.status.success() {
                return String::from_utf8_lossy(&out.stdout).into_owned();
            }
            last = String::from_utf8_lossy(&out.stderr).into_owned();
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        panic!("gh {args:?}: {last}");
    };
    let owner = gh(&["api", "user", "--jq", ".login"]).trim().to_owned();
    let full_name = format!("{owner}/{repo}");
    let visibility = gh(&[
        "repo",
        "view",
        &full_name,
        "--json",
        "visibility",
        "--jq",
        ".visibility",
    ]);
    assert_eq!(visibility.trim(), "PRIVATE");
    let remote = gh(&[
        "api",
        "-H",
        "Accept: application/vnd.github.raw",
        &format!("repos/{full_name}/contents/{log_file}"),
    ]);
    assert_eq!(remote, local, "file on GitHub differs from the local clone");
}
