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

## Notifications (optional)

Setup guide: [tutorials/notifications.md](tutorials/notifications.md). Verify with the
`notify-test` command; `check` logs in to the SMTP server without sending.

### `NOTIFY_ENABLED` (default: `true`)

Master switch. `false` turns off emails and healthcheck pings while keeping the other settings;
none of them are validated then.

### `SMTP_HOST` (default: unset)

SMTP server for notification emails, e.g. `smtp.gmail.com`. Unset: no emails. Setting any other
email variable (`SMTP_*`, `NOTIFY_EMAIL_*`) without it is an error.

### `SMTP_PORT` (default: `465`)

SMTP port. With the default `SMTP_TLS`, `465` connects with implicit TLS and any other port
(e.g. `587`) requires STARTTLS.

### `SMTP_TLS` (default: by port)

`implicit`, `starttls` or `none`. `none` (no encryption) is only accepted when `SMTP_HOST` is
this machine (`localhost`, `127.0.0.1`, `::1`); the E2E tests use it for their fake SMTP server.

### `SMTP_USERNAME` / `SMTP_PASSWORD` (default: unset)

SMTP login; set both or neither. For Gmail: your address and an
[app password](https://myaccount.google.com/apppasswords). The password is never logged.

### `NOTIFY_EMAIL_FROM` (default: `SMTP_USERNAME`)

Sender, `addr@example.com` or `Name <addr@example.com>`. Without a name, the display name is
`auto-git-commit-tool`. Required if `SMTP_USERNAME` is not an email address.

### `NOTIFY_EMAIL_TO` (default: the sender address)

Recipient.

### `HEALTHCHECK_URL` (default: unset)

Ping URL of a dead-man's switch such as [healthchecks.io](https://healthchecks.io)
(`https://hc-ping.com/<uuid>`) or a self-hosted equivalent. The service sends an HTTP `GET` every
`HEALTHCHECK_INTERVAL_MINUTES` once it has started; the monitor alerts you when the pings stop.
Treat it as a secret: it is never logged.

### `HEALTHCHECK_INTERVAL_MINUTES` (default: `5`)

Minutes between pings, `1`–`1440`. Configure the check's period to the same value.

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
