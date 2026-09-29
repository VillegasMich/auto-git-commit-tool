# Architecture

## Goal

Run unattended on any machine (inside a Docker container) and produce between `MIN_COMMITS`
and `MAX_COMMITS` commits per day to a private GitHub repository owned by the authenticated user,
at a fixed time of day.

## Design principles

- **Single static binary + two external tools.** The Rust binary orchestrates; `git` and `gh`
  do the actual work. They are invoked via `std::process::Command`. No libgit2, no GitHub REST client.
- **`gh` is a hard dependency.** The service refuses to start if `gh` is missing or unauthenticated.
- **UTC everywhere.** Scheduling and timestamps use UTC; the container timezone is irrelevant.
- **Idempotent daily runs.** Restarting the container must never produce a second batch of commits
  for the same day.
- **Fail loud, keep running.** A failed run is logged and retried; it does not crash the service.

## Components

```
src/
├── main.rs        # CLI (clap), logging, signal handlers, dispatch to app
├── app.rs         # wires everything into the commands: daemon, once, status, check
├── config.rs      # env var parsing + validation
├── preflight.rs   # check git/gh binaries, resolve user login/id, `gh auth status`
├── github.rs      # gh wrappers (trait `GitHub`): repo visibility, create private repo, clone, setup-git
├── bootstrap.rs   # ensure remote repo exists (create private) and local clone exists
├── repo.rs        # git wrappers (`GitRepo`, implements `Workspace`): recover, pull, append, commit, push
├── run.rs         # one daily run (trait `Workspace`): idempotency check, N commits, push
├── scheduler.rs   # next-run computation, catch-up decision, bounded sleep loop
├── simulate.rs    # `simulate` test mode: simulated clock + local sandbox repo
├── retry.rs       # exponential backoff
├── clock.rs       # `Clock` trait: `SystemClock` (sleep interruptible by SIGTERM), `SimulatedClock`
└── exec.rs        # `std::process::Command` wrapper: non-interactive, timeout, captured stderr
```

`run.rs`, `scheduler.rs` and `bootstrap.rs` only talk to the outside world through the
`Workspace`, `GitHub` and `Clock` traits, so they are unit tested with in-memory fakes.

## Command-line interface

Configuration always comes from environment variables; the CLI only selects what to do.

| Command                       | Purpose                                                                 |
| ----------------------------- | ----------------------------------------------------------------------- |
| `daemon` (default, no args)   | Long-running service: startup flow below, then one run per day.         |
| `once`                        | Startup flow, one (idempotent) run now, exit. Non-zero if push failed.  |
| `status`                      | Read-only report: account, repo, last log line, done today?, next run.  |
| `check`                       | Validate config, tools and authentication, then exit.                   |
| `simulate [--days N] [--start T] [--keep] [--sandbox DIR] [--github [--repo NAME]]` | Test mode, see below. |

### `simulate` (test mode)

Runs the real scheduler (`run_forever`), the real daily run and real `git`, with two substitutions:

- **Clock:** `SimulatedClock` starts at `--start` (default: one minute before today's
  `COMMIT_TIME`) and advances instantly on every sleep, so the scheduled time is "reached"
  immediately. Simulation ends one hour after the `N`-th scheduled run (`--days`, default 1).
  A `--start` after `COMMIT_TIME` exercises the catch-up path.
- **Repository:** a throwaway sandbox (system temp dir, or `--sandbox DIR`): a local bare
  repository stands in for GitHub, and a clone of it with an initial README commit. GitHub is
  never contacted and no token is needed.
- **`--github`:** the remote is real instead: preflight, `gh auth setup-git`, create the private
  repository `--repo` (default `auto-git-commit-simulation`) if missing, clone it into the
  sandbox, and before each run re-check it like the daemon does; pushes go to GitHub. `--repo`
  must differ from `REPO_NAME`, otherwise simulated dates would mark real days as done.

Afterwards a service restart is simulated at the final simulated time, which must report
"already committed today". The resulting log file is printed and the sandbox removed (`--keep`
keeps it). Log lines use simulated time; git commit dates are real wall-clock time.
See [testing.md](testing.md) for how the E2E tests use it.

## Startup flow

1. **Load config** from environment variables, validate (`MIN_COMMITS >= 1`,
   `MIN_COMMITS <= MAX_COMMITS`, `COMMIT_TIME` parses as `HH:MM`, `GH_TOKEN` set).
2. **Preflight**
   - `git --version` and `gh --version` succeed (otherwise exit non-zero immediately).
   - `gh api user --jq '.login,.id'` → resolve `login` and numeric `id`
     (used for the default noreply email `<id>+<login>@users.noreply.github.com`).
     HTTP 401 (bad token) exits immediately; other failures (e.g. the network is not up yet
     right after boot) are retried with backoff for ~25 minutes before exiting non-zero.
   - `gh auth status` succeeds.
3. **Git credentials**: `gh auth setup-git` so `git push` over HTTPS uses the `gh` token.
4. **Repository bootstrap**
   - `gh repo view <login>/<REPO_NAME>` → exists?
   - If not: `gh repo create <REPO_NAME> --private --add-readme --description "..."`.
   - If `DATA_DIR/<REPO_NAME>/.git` does not exist:
     `git clone https://github.com/<login>/<REPO_NAME>.git DATA_DIR/<REPO_NAME>` (HTTPS regardless
     of the user's `gh` `git_protocol` setting). A non-empty directory without `.git` is never
     overwritten (exit non-zero).
   - If the repo had to be created but a clone already exists, the clone belongs to a deleted
     repository: it is renamed to `<REPO_NAME>.orphaned-<timestamp>` and a fresh clone is made.
   - Transient failures are retried with the same startup backoff.
5. **Immediate run** (idempotent) if `RUN_ON_START=true`, or if `CATCH_UP=true` (default) and
   today's `COMMIT_TIME` has already passed — e.g. the machine was off or rebooting at that time.
6. **Enter scheduler loop.**

## Scheduler

- Compute the next occurrence of `COMMIT_TIME` in UTC. If today's time has already passed,
  schedule for tomorrow.
- Sleep until then. Sleep in bounded chunks (e.g. ≤ 60 s) and re-check the wall clock, so
  host suspend / clock jumps don't cause a missed or doubled run.
- On wake: execute a daily run, then compute the next occurrence.
- If a run fails, or leaves commits unpushed, the next wake-up is `min(now + 30 min, next slot)`.
  Because runs are idempotent, the retry only pushes pending commits or finishes a failed day.

No system `cron` is used inside the container; the binary is its own scheduler, which keeps the
image simple. In the image, `tini` is PID 1: it forwards `SIGTERM` to the binary and reaps
orphaned git helper processes.

**Shutdown:** the first `SIGTERM`/`SIGINT` sets a flag checked by every sleep (at most 250 ms
latency). A run in progress stops before its next commit, pushes what it has once, and exits
cleanly. A second signal exits immediately.

## Daily run

0. Before each run the remote is re-checked (bootstrap above), so a repository deleted on GitHub
   is recreated. If GitHub can't be reached but a local clone exists, the run continues locally.
1. **Local recovery** (`Workspace::prepare`), see [Recovery](#recovery-after-restart-or-crash).
2. `git pull --rebase` to stay in sync with the remote (a few quick retries; if it still fails,
   continue with local state — commits are pushed later).
3. **Idempotency check**: read the last non-empty line of `LOG_FILE`. If its date field equals
   today's UTC date, don't commit — only push commits that are still unpushed, if any.
4. Pick `n` uniformly at random in `[MIN_COMMITS, MAX_COMMITS]`.
5. For `i` in `1..=n`:
   - Append one line to `LOG_FILE`:
     ```
     <YYYY-MM-DD> | <RFC 3339 UTC timestamp with sub-second precision> | commit <i>/<n>
     ```
     Example: `2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3`
   - `git add LOG_FILE`
   - `git commit -m "chore: daily log <YYYY-MM-DD> (<i>/<n>)"`
   - Wait 2–8 seconds (random jitter) before the next commit so commit timestamps are distinct.
6. `git push --set-upstream origin HEAD` once for the whole batch, retried with backoff
   (~50 min). Between attempts `git pull --rebase` handles non-fast-forward rejections.

The timestamp is taken at the moment the line is written, so it reflects the real execution time.
The date field is the run's date, so all lines of one batch share it.

## Recovery after restart or crash

The service is meant to be started on boot and killed at any time (power loss, `SIGKILL`,
`docker kill`). The log file in the repository is the only state: there is no separate database.

| Situation at restart                                   | What happens                                                    |
| ------------------------------------------------------ | --------------------------------------------------------------- |
| Today's slot not reached yet                           | Wait for it as usual.                                           |
| Slot passed, nothing committed today                   | Catch-up run immediately (`CATCH_UP=true`).                     |
| Slot passed, today's commits already pushed            | Last line is dated today → nothing to do.                       |
| Commits made but not pushed (crash before push)        | Last line is dated today → no new commits; pending ones pushed. |
| Crash mid-batch (e.g. 2 of 4 commits done)             | Treated as done for today (2 commits); they are pushed.         |
| Line appended but not committed                        | `git reset --hard HEAD` discards it (local commits are kept).   |
| Stale `.git/index.lock`                                | Removed.                                                        |
| Interrupted `git pull --rebase` / merge                | `git rebase --abort` / `git merge --abort`.                     |
| Volume/`DATA_DIR` lost                                 | Re-cloned from GitHub; the pulled log file decides "done today".|

## Failure handling

| Failure                         | Behavior                                                                  |
| ------------------------------- | ------------------------------------------------------------------------- |
| `gh`/`git` missing              | Exit non-zero at startup (container restart policy surfaces it).          |
| Not authenticated / bad token   | Exit non-zero at startup.                                                 |
| Network down at boot            | Preflight/bootstrap retried with backoff (~25 min), then exit non-zero; systemd/Docker restarts the service. |
| Network error during pull       | A few retries, then commit locally anyway.                                |
| Network error during push       | Retry with exponential backoff (~50 min), then retry the whole run every 30 min. Unpushed local commits are pushed by the next successful run. |
| Push rejected (non-fast-forward)| `git pull --rebase` then retry push.                                      |
| Repo deleted on GitHub mid-life | Detected on next run → recreate (private), move old clone aside, re-clone.|
| External command hangs          | Every `git`/`gh` call has a timeout (5 min, clone 15 min); stalled HTTP transfers abort after 60 s below 1 KiB/s. |

## Logging

Structured logs to stdout (`tracing` + `tracing-subscriber`), level via `RUST_LOG`.
Every run logs: scheduled time, chosen `n`, each commit SHA, and push result.

## Suggested crates

- `chrono` – UTC time, formatting, next-run computation
- `rand` – number of commits and jitter
- `anyhow` / `thiserror` – error handling
- `tracing`, `tracing-subscriber` – logging
- `signal-hook` – SIGTERM/SIGINT handling
- `clap` – command-line subcommands

Keep dependencies minimal; the service is intentionally synchronous (no async runtime needed).
