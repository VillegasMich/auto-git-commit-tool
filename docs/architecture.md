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
├── main.rs        # entry point: load config, preflight, bootstrap, start scheduler
├── config.rs      # env var parsing + validation
├── preflight.rs   # check git/gh binaries, `gh auth status`, resolve user login/id
├── github.rs      # gh wrappers: repo exists?, create private repo, setup-git
├── repo.rs        # git wrappers: clone/pull, append line, add, commit, push
├── scheduler.rs   # compute next run instant, sleep loop
└── run.rs         # one daily run: pick N, write N lines, N commits, push
```

(Planned layout — adjust as the code evolves, and keep this file in sync.)

## Startup flow

1. **Load config** from environment variables, validate (`MIN_COMMITS >= 1`,
   `MIN_COMMITS <= MAX_COMMITS`, `COMMIT_TIME` parses as `HH:MM`, `GH_TOKEN` set).
2. **Preflight**
   - `git --version` and `gh --version` succeed.
   - `gh auth status` succeeds.
   - `gh api user --jq '.login,.id'` → resolve `login` and numeric `id`
     (used for the default noreply email `<id>+<login>@users.noreply.github.com`).
3. **Git credentials**: `gh auth setup-git` so `git push` over HTTPS uses the `gh` token.
4. **Repository bootstrap**
   - `gh repo view <login>/<REPO_NAME>` → exists?
   - If not: `gh repo create <REPO_NAME> --private --add-readme --description "..."`.
   - If `DATA_DIR/<REPO_NAME>/.git` does not exist: `gh repo clone <login>/<REPO_NAME> DATA_DIR/<REPO_NAME>`.
   - Otherwise: `git pull --rebase` on the default branch.
   - Configure local `user.name` / `user.email` in the clone.
5. **Optional immediate run** if `RUN_ON_START=true` and no run exists for today.
6. **Enter scheduler loop.**

## Scheduler

- Compute the next occurrence of `COMMIT_TIME` in UTC. If today's time has already passed,
  schedule for tomorrow.
- Sleep until then. Sleep in bounded chunks (e.g. ≤ 60 s) and re-check the wall clock, so
  host suspend / clock jumps don't cause a missed or doubled run.
- On wake: execute a daily run, then compute the next occurrence.

No system `cron` is used inside the container; the binary is its own scheduler, which keeps the
image simple and the process as PID 1 (handle `SIGTERM` for clean shutdown).

## Daily run

1. `git pull --rebase` to stay in sync with the remote.
2. **Idempotency check**: read the last line of `LOG_FILE`. If its date field equals today's UTC
   date, skip the run (already done today).
3. Pick `n` uniformly at random in `[MIN_COMMITS, MAX_COMMITS]`.
4. For `i` in `1..=n`:
   - Append one line to `LOG_FILE`:
     ```
     <YYYY-MM-DD> | <RFC 3339 UTC timestamp with sub-second precision> | commit <i>/<n>
     ```
     Example: `2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3`
   - `git add LOG_FILE`
   - `git commit -m "chore: daily log <YYYY-MM-DD> (<i>/<n>)"`
   - Wait a few seconds (small random jitter) so commit timestamps are distinct.
5. `git push` once for the whole batch.

The timestamp is taken at the moment the line is written, so it reflects the real execution time.

## Failure handling

| Failure                         | Behavior                                                                  |
| ------------------------------- | ------------------------------------------------------------------------- |
| `gh`/`git` missing              | Exit non-zero at startup (container restart policy surfaces it).          |
| Not authenticated / bad token   | Exit non-zero at startup.                                                 |
| Network error during pull/push  | Log error, retry with exponential backoff (e.g. up to ~1 h), then wait for next day. Unpushed local commits are pushed on the next successful run. |
| Push rejected (non-fast-forward)| `git pull --rebase` then retry push.                                      |
| Repo deleted on GitHub mid-life | Detected on next run → re-run bootstrap (recreate + re-clone).            |

## Logging

Structured logs to stdout (`tracing` + `tracing-subscriber`), level via `RUST_LOG`.
Every run logs: scheduled time, chosen `n`, each commit SHA, and push result.

## Suggested crates

- `chrono` – UTC time, formatting, next-run computation
- `rand` – number of commits and jitter
- `anyhow` / `thiserror` – error handling
- `tracing`, `tracing-subscriber` – logging
- `signal-hook` or `ctrlc` – SIGTERM handling

Keep dependencies minimal; the service is intentionally synchronous (no async runtime needed).
