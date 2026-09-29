//! Thin wrapper around [`std::process::Command`] used to invoke `git` and `gh`.
//!
//! Every command runs non-interactively, with stdin closed and a hard timeout so a stalled
//! network operation can never hang the service forever.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::debug;

/// Default upper bound for any single external command.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("could not start `{program}` (is it installed and on PATH?): {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("i/o error while running `{command}`: {source}")]
    Io {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{command}` timed out after {}s", timeout.as_secs())]
    Timeout { command: String, timeout: Duration },
    #[error("`{command}` exited with {status}: {stderr}")]
    Failed {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
}

impl ExecError {
    /// Standard error of a command that ran and exited unsuccessfully.
    pub fn stderr(&self) -> Option<&str> {
        match self {
            Self::Failed { stderr, .. } => Some(stderr),
            _ => None,
        }
    }
}

/// A command to run. Arguments must never contain secrets: they are visible in `ps` and logs.
#[derive(Debug, Clone)]
pub struct Cmd {
    program: String,
    args: Vec<OsString>,
    timeout: Duration,
}

impl Cmd {
    pub fn new(program: &str) -> Self {
        Self {
            program: program.to_owned(),
            args: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Runs the command and returns its stdout (trailing whitespace trimmed).
    pub fn run(&self) -> Result<String, ExecError> {
        let command = self.to_string();
        debug!(%command, "exec");

        let mut process = Command::new(&self.program);
        process
            .args(&self.args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_SPINNER_DISABLED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = process.spawn().map_err(|source| ExecError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());

        let status = match wait_timeout(&mut child, self.timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // Reader threads are detached: grandchildren may still hold the pipes open.
                return Err(ExecError::Timeout {
                    command,
                    timeout: self.timeout,
                });
            }
            Err(source) => return Err(ExecError::Io { command, source }),
        };

        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        if status.success() {
            Ok(stdout.trim_end().to_owned())
        } else {
            Err(ExecError::Failed {
                command,
                status,
                stderr: stderr.trim().to_owned(),
            })
        }
    }
}

impl fmt::Display for Cmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.program)?;
        for arg in &self.args {
            write!(f, " {}", arg.to_string_lossy())?;
        }
        Ok(())
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout() {
        let out = Cmd::new("sh").args(["-c", "echo hello"]).run().unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn failure_carries_stderr() {
        let err = Cmd::new("sh")
            .args(["-c", "echo boom >&2; exit 3"])
            .run()
            .unwrap_err();
        assert_eq!(err.stderr(), Some("boom"));
    }

    #[test]
    fn missing_program_is_spawn_error() {
        let err = Cmd::new("definitely-not-a-real-program-xyz")
            .run()
            .unwrap_err();
        assert!(matches!(err, ExecError::Spawn { .. }));
    }

    #[test]
    fn times_out() {
        let err = Cmd::new("sleep")
            .arg("5")
            .timeout(Duration::from_millis(100))
            .run()
            .unwrap_err();
        assert!(matches!(err, ExecError::Timeout { .. }));
    }

    #[test]
    fn display_joins_args() {
        let cmd = Cmd::new("git").args(["commit", "-m", "msg"]);
        assert_eq!(cmd.to_string(), "git commit -m msg");
    }
}
