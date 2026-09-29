# auto-git-commit-tool

A small, long-running service that keeps your GitHub contribution graph active by making
**1–5 automated commits every day** to a **private repository** on your account.

It is written in Rust, ships as a Docker image, and uses the [GitHub CLI (`gh`)](https://cli.github.com/)
as a hard dependency for authentication and repository management.

> **Status:** specification / early development. This README describes the intended behavior.
> See [`docs/`](docs/) for the detailed design.

## How it works

1. **Startup** – the service verifies that `git` and `gh` are installed and that `gh` is authenticated
   (via the `GH_TOKEN` environment variable).
2. **Repository bootstrap** – it looks for the target repository (default: `daily-log`) on the
   authenticated account. If it does not exist, it is created as **private** with `gh repo create`.
   The repository is then cloned (or pulled, if already cloned) into the data directory.
3. **Scheduling** – like a cron job, the service sleeps until the configured time of day
   (default `12:00` UTC) and wakes up once per day.
4. **Daily run** – it picks a random number of commits between `MIN_COMMITS` and `MAX_COMMITS`
   (default 1–5). For each commit it appends one line to a text file and commits it, then pushes
   all commits in one `git push`.

Each line in the log file records the date and the exact UTC execution time:

```text
2026-09-28 | 2026-09-28T12:00:00.418273Z | commit 1/3
2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3
2026-09-28 | 2026-09-28T12:00:05.117690Z | commit 3/3
```

## Quick start

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
| `RUST_LOG`         | `info`                        | Log level.                                                         |

Full details: [`docs/configuration.md`](docs/configuration.md).

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
- [`docs/tutorials/`](docs/tutorials/) – step-by-step GitHub setup guides
  - [Enable private contributions](docs/tutorials/enable-private-contributions.md)
- [`CLAUDE.md`](CLAUDE.md) – guidance for AI coding assistants working in this repo

## Development

```bash
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```

Running locally requires `git` and `gh` on your `PATH` and `GH_TOKEN` exported.
