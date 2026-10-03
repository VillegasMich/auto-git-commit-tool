# auto-git-commit-tool

A small, long-running service that keeps your GitHub contribution graph active by making
**1–5 automated commits every day** to a **private repository** on your account.

It is written in Rust, ships as a Docker image (or a native binary), runs as a systemd service,
and uses the [GitHub CLI (`gh`)](https://cli.github.com/) as a hard dependency for authentication
and repository management.

See [`docs/`](docs/) for the detailed design.

## How it works

1. **Startup** – the service verifies that `git` and `gh` are installed and that `gh` is authenticated
   (via the `GH_TOKEN` environment variable).
2. **Repository bootstrap** – it looks for the target repository (default: `daily-log`) on the
   authenticated account. If it does not exist, it is created as **private** with `gh repo create`.
   The repository is then cloned (or pulled, if already cloned) into the data directory.
3. **Scheduling** – like a cron job, the service sleeps until the configured time of day
   (default `12:00` UTC) and wakes up once per day. If it starts after that time and nothing
   was committed today (e.g. the machine was off), it catches up immediately.
4. **Daily run** – it picks a random number of commits between `MIN_COMMITS` and `MAX_COMMITS`
   (default 1–5). For each commit it appends one line to a text file and commits it, then pushes
   all commits in one `git push`.
5. **Recovery** – the log file is the state: if its last line is dated today, the day is done.
   Restarts, reboots and crashes never double a day's commits, and commits that couldn't be pushed
   are pushed later.

Each line in the log file records the date and the exact UTC execution time:

```text
2026-09-28 | 2026-09-28T12:00:00.418273Z | commit 1/3
2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3
2026-09-28 | 2026-09-28T12:00:05.117690Z | commit 3/3
```

## Quick start

### As a systemd service (recommended)

Requires systemd, [`gh`](https://cli.github.com/) logged in (`gh auth login`) or `GH_TOKEN`
exported, and either Docker or a Rust toolchain.

```bash
export COMMIT_TIME=15:30      # optional, see Configuration
scripts/install.sh            # runs the Docker image from systemd
# or, without Docker:
cargo build --release && scripts/install.sh native

journalctl -u auto-git-commit-tool -f
```

The service starts on boot. Details: [`docs/deployment.md`](docs/deployment.md#install-as-a-systemd-service).

### With Docker only

```bash
# Build the image
docker build -t auto-git-commit-tool .

# Run it as a service
docker run -d \
  --name auto-git-commit \
  --restart unless-stopped \
  -e GH_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxx \
  -e COMMIT_TIME=12:00 \
  -v auto-git-commit-data:/data \
  auto-git-commit-tool
```

Or with Docker Compose:

```yaml
services:
  auto-git-commit:
    build: .
    restart: unless-stopped
    environment:
      GH_TOKEN: ${GH_TOKEN}
      REPO_NAME: daily-log
      COMMIT_TIME: "12:00"
      MIN_COMMITS: 1
      MAX_COMMITS: 5
    volumes:
      - data:/data
volumes:
  data:
```

## Command line

```text
auto-git-commit-tool [daemon]   # run forever, once per day at COMMIT_TIME (default)
auto-git-commit-tool once       # today's run now (no-op if already done today), then exit
auto-git-commit-tool status     # account, repo, last entry, done today?, next run
auto-git-commit-tool check      # validate config, git/gh, authentication and SMTP login
auto-git-commit-tool notify-test # send a test email and one healthcheck ping
auto-git-commit-tool simulate   # test: simulated clock reaches COMMIT_TIME now, local sandbox repo
```

`simulate` runs the real scheduler and commit logic against a throwaway local repository, with a
clock that jumps straight to `COMMIT_TIME` — nothing is pushed to GitHub and no token is needed.
Options: `--days N` (simulate N daily runs), `--start HH:MM|RFC3339` (e.g. after `COMMIT_TIME`
to test the catch-up after a missed run), `--keep` (keep the sandbox to inspect it), `--notify`
(also send the configured notifications for real). In Docker:
`docker run --rm --env-file .env auto-git-commit-tool simulate --days 3`.

`simulate --github` does the same against real GitHub: authenticates, creates the private
repository if needed (`--repo`, default `auto-git-commit-simulation`, never your `REPO_NAME`),
commits and pushes. See [`docs/testing.md`](docs/testing.md).

## Configuration

All configuration is done through environment variables.

| Variable           | Default                       | Description                                                        |
| ------------------ | ----------------------------- | ------------------------------------------------------------------ |
| `GH_TOKEN`         | **required**                  | GitHub token used by `gh` and for `git push`.                      |
| `REPO_NAME`        | `daily-log`                   | Name of the private repository to create/use.                      |
| `COMMIT_TIME`      | `12:00`                       | Daily run time, `HH:MM`, interpreted in **UTC**.                   |
| `MIN_COMMITS`      | `1`                           | Minimum commits per day.                                           |
| `MAX_COMMITS`      | `5`                           | Maximum commits per day.                                           |
| `LOG_FILE`         | `activity.log`                | File (inside the repo) that receives one new line per commit.      |
| `DATA_DIR`         | `/data`                       | Where the repository is cloned. Mount a volume here.               |
| `GIT_AUTHOR_NAME`  | GitHub login                  | Commit author name.                                                |
| `GIT_AUTHOR_EMAIL` | `<id>+<login>@users.noreply.github.com` | Commit author email. Must be linked to your account.     |
| `RUN_ON_START`     | `false`                       | If `true`, perform a run immediately on startup (if none today).   |
| `CATCH_UP`         | `true`                        | If started after today's `COMMIT_TIME` with no run today, run now. |
| `RUST_LOG`         | `info`                        | Log level.                                                         |
| `NOTIFY_ENABLED`   | `true`                        | Master switch: `false` turns off emails and healthcheck pings.     |
| `SMTP_HOST`        | unset (no email)              | SMTP server for notification emails, e.g. `smtp.gmail.com`.        |
| `SMTP_PORT`        | `465`                         | `465` = implicit TLS, anything else = STARTTLS (e.g. `587`).       |
| `SMTP_TLS`         | by port                       | `implicit`, `starttls`, or `none` (localhost only, for tests).     |
| `SMTP_USERNAME` / `SMTP_PASSWORD` | unset          | SMTP login (Gmail: your address + an app password).               |
| `NOTIFY_EMAIL_FROM`| `SMTP_USERNAME`               | Sender address.                                                    |
| `NOTIFY_EMAIL_TO`  | sender address                | Recipient address.                                                 |
| `HEALTHCHECK_URL`  | unset (no pings)              | Dead-man's-switch ping URL, e.g. `https://hc-ping.com/<uuid>`.     |
| `HEALTHCHECK_INTERVAL_MINUTES` | `5`               | Minutes between pings.                                             |

Full details: [`docs/configuration.md`](docs/configuration.md).

## Notifications (optional)

- **Email** (`SMTP_*`): one email per day when the commits are pushed, with the SHAs and log
  lines; a failing day is reported once, plus once more when it recovers; and one when the
  service is stopped (`systemctl stop`, `docker stop`, reboot, shutdown). Subjects start with
  `[auto-git-commit] <date>:` so a mail filter can label them and skip the inbox.
- **Machine off / killed** (`HEALTHCHECK_URL`): a process that loses power can't report it, so the
  service pings an external dead-man's switch such as [healthchecks.io](https://healthchecks.io)
  every few minutes; it alerts you when the pings stop.

Setup guide: [Set up notifications](docs/tutorials/notifications.md).

## Requirements for commits to show up on your profile

GitHub only counts a commit on your contribution graph when:

- The commit **author email** is a verified email on your account (or your GitHub
  `noreply` address – the default).
- The commit is on the repository's **default branch**.
- For private repositories, **"Private contributions"** is enabled in your profile settings.
  Otherwise private commits are not shown. Step-by-step guide:
  [Enable private contributions](docs/tutorials/enable-private-contributions.md).

## Token permissions

- **Classic PAT:** `repo` scope.
- **Fine-grained PAT:** *All repositories* access with **Administration: Read & write**
  (to create the repo), **Contents: Read & write** (to push) and **Metadata: Read**.

See [`docs/deployment.md`](docs/deployment.md).

## Documentation

- [`docs/architecture.md`](docs/architecture.md) – components, run flow, idempotency, failure handling
- [`docs/configuration.md`](docs/configuration.md) – every setting in detail
- [`docs/deployment.md`](docs/deployment.md) – Docker image, tokens, running on any machine
- [`docs/testing.md`](docs/testing.md) – unit tests, `simulate`, E2E tests, CI
- [`docs/tutorials/`](docs/tutorials/) – step-by-step GitHub setup guides
  - [Enable private contributions](docs/tutorials/enable-private-contributions.md)
- [`widgets/`](widgets/) – collection of Markdown widgets (streaks, contribution graphs, stats
  cards, snake, 3D calendar) to show your activity on your profile README
- [`CLAUDE.md`](CLAUDE.md) – guidance for AI coding assistants working in this repo

## Development

```bash
cargo build
cargo test                                  # unit + offline end-to-end tests
E2E_GITHUB=1 cargo test --test e2e github   # end-to-end against real GitHub
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Running locally requires `git` and `gh` on your `PATH` and `GH_TOKEN` exported
(e.g. `export GH_TOKEN=$(gh auth token)`). For a manual end-to-end test use a throwaway repo:

```bash
REPO_NAME=agct-test DATA_DIR=/tmp/agct cargo run -- once
```
