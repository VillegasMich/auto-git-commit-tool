# Testing

Three layers, from fastest to most realistic:

| Layer            | Command                                   | Needs              | Touches GitHub |
| ---------------- | ----------------------------------------- | ------------------ | -------------- |
| Unit tests       | `cargo test --bins`                       | `git`              | no             |
| Offline E2E      | `cargo test --test e2e`                   | `git`              | no             |
| GitHub E2E       | `E2E_GITHUB=1 cargo test --test e2e`      | `git`, `gh`, token | **yes**        |

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
```

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
| `github_simulation_matches_expected_log`     | 2 days × 2 commits via `--github`; repo is private and the file fetched from the GitHub API equals the local one. |

The GitHub test uses the repository `E2E_REPO` (default `auto-git-commit-e2e`, created on first
run) and a unique `LOG_FILE` per run (`e2e/<timestamp>-<pid>.log`), so repeated and concurrent runs
(local and CI) never interfere. Nothing is deleted afterwards (deleting needs the `delete_repo`
scope); delete the repository whenever you like.

```bash
cargo test --test e2e                                     # offline
E2E_GITHUB=1 cargo test --test e2e github -- --nocapture  # GitHub (uses GH_TOKEN or gh login)
```

## CI (`.github/workflows/ci.yml`)

On pushes to `main`, pull requests and manual dispatch:

1. **test** – `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
   (unit + offline E2E).
2. **docker** – builds the image and runs `simulate --days 2` inside it.
3. **e2e-github** – runs the GitHub E2E test. Needs a repository secret, otherwise it's skipped
   with a notice:
   - `E2E_GH_TOKEN`: a token for the account that owns the E2E repository. Classic PAT with
     `repo`, or fine-grained with Administration R/W (only if the repo doesn't exist yet),
     Contents R/W and Metadata R. The workflow's built-in `GITHUB_TOKEN` can't create repos.
   - Optional repository variable `E2E_REPO` to change the repository name.

   Set it with: `gh secret set E2E_GH_TOKEN` (paste the token when prompted).
