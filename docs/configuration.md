# Configuration

All settings come from environment variables. Invalid values abort startup with a clear error.

## Required

### `GH_TOKEN`

GitHub token. `gh` reads it natively, and `gh auth setup-git` makes `git push` use it too.
See [deployment.md](deployment.md#github-token) for required scopes.

## Optional

### `REPO_NAME` (default: `daily-log`)

Name of the repository under the authenticated account. Created as **private** if missing.
If a repository with this name already exists it is reused as-is (even if public — a warning is logged).

### `COMMIT_TIME` (default: `12:00`)

Time of the daily run, `HH:MM` 24-hour format, **UTC**. All commits of the day are made in a
single batch starting at this time.

Tip: choose a time that falls on the same calendar day in your local timezone, since GitHub's
contribution graph buckets commits by the commit timestamp's date.

### `MIN_COMMITS` / `MAX_COMMITS` (defaults: `1` / `5`)

Inclusive bounds for the random number of commits per day. Constraints: `1 <= MIN_COMMITS <= MAX_COMMITS`.
Set both to the same value for a fixed number of commits.

### `LOG_FILE` (default: `activity.log`)

Path, relative to the repository root, of the text file that receives one line per commit.

Line format:

```text
<YYYY-MM-DD> | <RFC 3339 UTC timestamp, microsecond precision> | commit <i>/<n>
```

### `DATA_DIR` (default: `/data`)

Directory where the repository is cloned (`$DATA_DIR/$REPO_NAME`). Mount a volume here so the clone
survives container restarts. If the volume is lost, the repo is simply re-cloned from GitHub.

The native systemd unit sets it to `/var/lib/auto-git-commit-tool`; don't set it in the env file
for that mode.

### `GIT_AUTHOR_NAME` (default: GitHub login)

Name used for `user.name` in the clone.

### `GIT_AUTHOR_EMAIL` (default: `<id>+<login>@users.noreply.github.com`)

Email used for `user.email`. **Must be associated with your GitHub account**, otherwise commits
will not count on the contribution graph. The default noreply address always works.

### `RUN_ON_START` (default: `false`)

When `true`, perform a run right after startup if no run has happened yet today (UTC), even if
today's `COMMIT_TIME` hasn't been reached yet. Useful for testing.

### `CATCH_UP` (default: `true`)

When `true` and the service starts **after** today's `COMMIT_TIME` without having committed today
(the machine was off, rebooting or the service was stopped at that time), it runs immediately
instead of waiting until tomorrow. Runs are idempotent, so this never doubles a day's commits.
Set to `false` to only ever commit at `COMMIT_TIME`.

### `RUST_LOG` (default: `info`)

Log filter for `tracing-subscriber` (`error`, `warn`, `info`, `debug`, `trace`).

## Example `.env`

Start from [`.env.example`](../.env.example) (`cp .env.example .env`), which lists every setting
with its default. Minimal version:

```dotenv
GH_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxx
REPO_NAME=daily-log
COMMIT_TIME=15:30
MIN_COMMITS=1
MAX_COMMITS=5
RUN_ON_START=true
CATCH_UP=true
```

Never commit a real `.env` file — it is listed in `.gitignore`.
