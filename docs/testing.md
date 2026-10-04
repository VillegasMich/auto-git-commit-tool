# Testing

Three layers, from fastest to most realistic:

| Layer            | Command                                   | Needs              | Touches GitHub |
| ---------------- | ----------------------------------------- | ------------------ | -------------- |
| Unit tests       | `cargo test --bins`                       | `git`              | no             |
| Offline E2E      | `cargo test --test e2e`                   | `git`              | no             |
| GitHub E2E       | `E2E_GITHUB=1 cargo test --test e2e`      | `git`, `gh`, token (+ `E2E_SMTP_*`, `E2E_HEALTHCHECK_URL` for notifications) | **yes** |

`cargo test` runs the first two; the GitHub E2E test skips itself unless `E2E_GITHUB=1`.

## Unit tests

Pure logic (config validation, next-run computation, log-line format, "already ran today",
backoff) plus `run.rs`, `scheduler.rs` and `bootstrap.rs` against in-memory fakes of the
`Workspace`, `GitHub` and `Clock` traits. `repo.rs` is tested with real `git` against a local bare
repository in a temp dir. They never contact GitHub.

## The `simulate` command

Manual test mode, also what the E2E tests drive. It runs the real scheduler and daily run with a
simulated clock that starts one minute before `COMMIT_TIME` and jumps ahead instead of sleeping,
then simulates a service restart (which must not commit again).

```bash
auto-git-commit-tool simulate                         # 1 day, local sandbox, no GitHub, no token
auto-git-commit-tool simulate --days 3 --start 18:30  # start after COMMIT_TIME: catch-up path
auto-git-commit-tool simulate --keep                  # keep the temp sandbox to inspect it
auto-git-commit-tool simulate --sandbox ./sim         # sandbox in ./sim (kept)

auto-git-commit-tool simulate --github                # real GitHub, see below
auto-git-commit-tool simulate --github --repo my-sim --days 2
auto-git-commit-tool simulate --notify                # also send the configured notifications
```

### `--notify`

Sends the configured notifications for real: one healthcheck ping at the start, the email for
each simulated run, and the "service stopped" email when the simulated time runs out (the
simulated restart afterwards finds the day done and sends nothing). Subjects carry the simulated
dates. If an email or the ping can't be delivered, `simulate` exits non-zero. Without any
`SMTP_HOST`/`HEALTHCHECK_URL` configured it refuses to start.

### `--github`

Same simulated clock, but everything else is real: `gh` preflight and auth, `gh auth setup-git`,
creating the **private** repository if missing, cloning it (into the sandbox), committing and
pushing to GitHub, and re-checking the remote before each run.

- Uses `--repo` (default `auto-git-commit-simulation`), which **must differ from `REPO_NAME`**:
  simulated dates written to the real log would make the real service skip real days.
- Uses `GH_TOKEN`, or your `gh` login if it isn't set.
- The repository persists between runs, and so does its log file: a second simulation of the same
  day is skipped ("already committed today") — that is the real behavior. Use a different
  `LOG_FILE` (or `--start` date) for a fresh log.
- The commits are real commits made now, so they appear on your contribution graph (if private
  contributions are enabled). Log lines carry the simulated time.

## End-to-end tests (`tests/e2e.rs`)

Black-box tests of the built binary. Each runs `simulate` with a fixed configuration
(`MIN_COMMITS = MAX_COMMITS`, fixed `--start` in 2030) and checks the resulting log file against
the expected result: number of lines, dates, `commit i/n` numbering, first commit exactly at
`COMMIT_TIME` (or at the catch-up time), 2–8 s between commits, and the restart not committing.

| Test                                         | Checks                                                            |
| -------------------------------------------- | ----------------------------------------------------------------- |
| `offline_simulation_matches_expected_log`    | 3 days × 3 commits; remote has the same file and 1 + 9 commits.  |
| `offline_catch_up_after_missed_commit_time`  | Start at 18:30 after a 12:00 slot → runs at 18:30.               |
| `offline_without_catch_up_waits_for_next_day`| `CATCH_UP=false` → first run next day at 12:00.                  |
| `offline_simulation_sends_notifications`     | `--notify` against a fake SMTP server and a fake ping server on localhost (`SMTP_TLS=none`): one ping; subjects `2 commits pushed` × 2 and `service stopped`, nothing for the restart; sender/recipient; each log line and commit link in the email. |
| `offline_undeliverable_notification_fails_simulation` | SMTP server unreachable → `simulate --notify` fails. |
| `github_simulation_matches_expected_log`     | 2 days × 2 commits via `--github`; repo is private and the file fetched from the GitHub API equals the local one. With `E2E_SMTP_*` / `E2E_HEALTHCHECK_URL`: also `--notify` with real Gmail/healthchecks.io (3 emails sent, 1 ping). |

The GitHub test uses the repository `E2E_REPO` (default `auto-git-commit-e2e`, created on first
run) and a unique `LOG_FILE` per run (`e2e/<timestamp>-<pid>.log`), so repeated and concurrent runs
(local and CI) never interfere. Nothing is deleted afterwards (deleting needs the `delete_repo`
scope); delete the repository whenever you like.

The test's own environment never leaks in: every setting, including the notification ones, is
cleared before running the binary. For the GitHub test, real notifications are configured with
`E2E_`-prefixed variables (`E2E_SMTP_HOST`, `E2E_SMTP_PORT`, `E2E_SMTP_USERNAME`,
`E2E_SMTP_PASSWORD`, `E2E_NOTIFY_EMAIL_TO`, `E2E_HEALTHCHECK_URL`), passed on without the prefix.

```bash
cargo test --test e2e                                     # offline
E2E_GITHUB=1 cargo test --test e2e github -- --nocapture  # GitHub (uses GH_TOKEN or gh login)
```

## CI (`.github/workflows/ci.yml`)

On pushes to `main`, pull requests, manual dispatch and published releases (the **Release**
workflow, `release.yml`, also dispatches it on the new tag with `publish`; see
[deployment.md](deployment.md#releasing)):

1. **test** – `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
   (unit + offline E2E).
2. **docker** (after **test**) – builds the image and runs `simulate --days 2` inside it. Then:
   - PRs, pushes, dispatch: dry-run push – computes the tags and lists them in the job summary,
     nothing is uploaded and no Docker Hub credentials are needed.
   - Published release, or manual dispatch on a `v*` tag with `publish` ticked: builds
     `linux/amd64` + `linux/arm64` and pushes to Docker Hub
     ([deployment.md](deployment.md#publishing-to-docker-hub)).
3. **e2e-github** – runs the GitHub E2E test, only for pushes to `main`, releases and manual
   runs (not pull requests: they make real commits and send real emails). Needs a repository
   secret, otherwise it's skipped with a notice — the job is green either way, so check its
   steps to see whether the test actually ran:
   - `E2E_GH_TOKEN`: a token for the account that owns the E2E repository. Classic PAT with
     `repo`, or fine-grained with Administration R/W (only if the repo doesn't exist yet),
     Contents R/W and Metadata R. The workflow's built-in `GITHUB_TOKEN` can't create repos.
   - Optional repository variable `E2E_REPO` to change the repository name.

   Set it with: `gh secret set E2E_GH_TOKEN` (paste the token when prompted).

   Optional, to also send real notifications (3 emails and 1 ping per run):

   | Name                   | Kind     | Value                                                         |
   | ---------------------- | -------- | ------------------------------------------------------------- |
   | `E2E_SMTP_USERNAME`    | secret   | e.g. `you@gmail.com`; enables email (host defaults to `smtp.gmail.com`) |
   | `E2E_SMTP_PASSWORD`    | secret   | the app password                                              |
   | `E2E_NOTIFY_EMAIL_TO`  | variable | e.g. `you+ci@gmail.com`, to filter CI emails away             |
   | `E2E_SMTP_HOST` / `E2E_SMTP_PORT` | variable | other providers                                    |
   | `E2E_HEALTHCHECK_URL`  | secret   | ping URL of a **separate** check: CI pings to the production check would hide a real outage |

   ```bash
   gh secret set E2E_SMTP_USERNAME
   gh secret set E2E_SMTP_PASSWORD
   gh secret set E2E_HEALTHCHECK_URL
   gh variable set E2E_NOTIFY_EMAIL_TO --body 'you+ci@gmail.com'
   ```
