# Deployment

The service is designed to run as a systemd service on any Linux machine (home server, VPS,
Raspberry Pi, NAS…), started on boot. Two modes:

| Mode               | What systemd runs                        | Host requirements            |
| ------------------ | ---------------------------------------- | ---------------------------- |
| `docker` (default) | `docker run … auto-git-commit-tool`      | systemd, Docker, `gh`        |
| `native`           | a binary built on the machine with cargo | systemd, `gh`, `git`, (cargo)|

On the host, `gh` is used by the installer to read and validate your token (`gh auth token`).
Inside the container the image ships its own `git` and `gh`.

## Install as a systemd service

```bash
gh auth login                       # once, if you don't export GH_TOKEN yourself
export COMMIT_TIME=15:30            # optional settings, see configuration.md
scripts/install.sh                  # docker mode
# or
cargo build --release && scripts/install.sh native
```

Run the script as your normal user; it calls `sudo` for system changes. It:

1. Checks the host requirements for the chosen mode.
2. Takes the token from `$GH_TOKEN`, or from your `gh` login, and checks it against GitHub.
3. Builds the image (`auto-git-commit-tool:latest`), or pulls [`IMAGE`](configuration.md#image-default-auto-git-commit-toollatest-built-locally)
   if it is set to a published image, or installs `target/release/auto-git-commit-tool` to
   `/usr/local/bin` (building it with cargo if it isn't built yet).
4. Writes `/etc/auto-git-commit-tool/env` (root-only, mode 600) with `GH_TOKEN` and any exported
   settings. An existing file is kept unless you pass `--reconfigure`.
5. Installs `/etc/systemd/system/auto-git-commit-tool.service` from
   [`deploy/systemd/`](../deploy/systemd/), enables it on boot and (re)starts it.

Re-run it after pulling new code to rebuild and restart (or see [Upgrading](#upgrading) to use a
published image). Switching mode is just running it with the other mode (the unit name stays the
same).

```bash
systemctl status auto-git-commit-tool
journalctl -u auto-git-commit-tool -f
sudo docker exec auto-git-commit-tool auto-git-commit-tool status   # docker mode
scripts/uninstall.sh            # remove the service, keep token/data
scripts/uninstall.sh --purge    # also delete env file, binary, clone, image and volume
```

To change settings later: `sudoedit /etc/auto-git-commit-tool/env && sudo systemctl restart auto-git-commit-tool`.

### Upgrading

To run a [published release](#publishing-to-docker-hub) instead of building locally, set `IMAGE`
in the env file and re-run the installer:

```bash
sudoedit /etc/auto-git-commit-tool/env      # IMAGE=<user>/auto-git-commit-tool:1.3.0
scripts/install.sh                          # pulls the image, then restarts the service on it
# or, without editing the file (the exported value is written to it):
IMAGE=<user>/auto-git-commit-tool:1.3.0 scripts/install.sh
```

The pull happens before the restart, so a typo or a missing tag fails the script and leaves the
running version alone. The restart stops the old container gracefully; the clone in the
`auto-git-commit-data` volume is reused and a day's commits are never doubled, so upgrading at
any time is safe. To roll back, set the previous tag and re-run. Removing `IMAGE` (or
`--reconfigure` without exporting it) goes back to the locally built image.

A plain `sudo systemctl restart auto-git-commit-tool` also picks up a changed `IMAGE` (Docker
pulls a tag that isn't present yet), but never re-pulls a tag it already has, such as `latest`.
Requires a unit installed by this version of the script (one with `EnvironmentFile=`); re-run
the installer once if yours is older.

### Units

- **docker** – `Requires=docker.service`, removes a stale container left by an unclean stop,
  runs `$IMAGE` (from the env file, default `auto-git-commit-tool:latest`) in the foreground with
  `--env-file` and the `auto-git-commit-data` volume,
  and `--hostname %H` (the host's name, shown in notification emails), `Restart=always`. `systemctl stop` → `docker stop --time 30` → `SIGTERM` to the service.
- **native** – `DynamicUser=yes` with `StateDirectory=auto-git-commit-tool`: the clone and the
  git/gh config live in `/var/lib/auto-git-commit-tool` (`HOME` points there, so your own
  `~/.gitconfig` is never touched), sandboxed with `ProtectSystem=strict`/`ProtectHome=yes`.

Both wait for `network-online.target`, and the service itself retries GitHub for ~25 minutes at
startup, so booting with a slow network is fine. After a reboot it catches up on a missed day
([architecture.md](architecture.md#recovery-after-restart-or-crash)).

## Docker image

Multi-stage build ([`Dockerfile`](../Dockerfile)):

1. **Builder** – `rust:1-slim-trixie`, `cargo build --release --locked` (dependencies cached in
   their own layer).
2. **Runtime** – `debian:trixie-slim` (same Debian release as the builder, so glibc matches) with:
   - `ca-certificates`, `git`, `tini`
   - `gh` (installed from the official GitHub CLI apt repository)
   - the compiled binary, run as non-root user `app` (uid 1000) under `tini`
     (`ENTRYPOINT tini -- auto-git-commit-tool`, default `CMD daemon`)

Other commands run in the same image:
`docker run --rm --env-file .env -v auto-git-commit-data:/data auto-git-commit-tool status`.

Multi-arch (e.g. for Raspberry Pi):

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t <you>/auto-git-commit-tool --push .
```

## Publishing to Docker Hub

CI (`docker` job in [`ci.yml`](../.github/workflows/ci.yml)) pushes the image when a GitHub
release is **published**, or when run manually on a release tag with *publish* (what the
[Release workflow](#releasing) does). The release tag must be semver (`v1.2.3` or `1.2.3`); it becomes the
tags `1.2.3`, `1.2` and `latest` (no `latest` for pre-releases like `v1.3.0-rc.1`), for
`linux/amd64` and `linux/arm64`. Every other run only builds and does a dry-run push.

One-time setup:

1. Docker Hub → *Account settings* → *Personal access tokens* → *Generate new token*, access
   **Read & Write**. Copy it (it's shown once).
2. GitHub repo → *Settings* → *Secrets and variables* → *Actions*:
   - *Secrets* tab → `DOCKERHUB_TOKEN` = the token.
   - *Variables* tab → `DOCKERHUB_USERNAME` = your Docker Hub username.
   - Optional variable `DOCKERHUB_IMAGE` (e.g. `myorg/auto-git-commit-tool`); defaults to
     `<DOCKERHUB_USERNAME>/auto-git-commit-tool`. The Docker Hub repository is created on first
     push (public on free plans) if it doesn't exist.

Or with `gh`:

```bash
gh secret set DOCKERHUB_TOKEN          # paste the token when prompted
gh variable set DOCKERHUB_USERNAME --body <you>
```

The job fails with an error if the secret or variable is missing. Secrets are not exposed to pull
requests from forks, and only the publish path logs in.

## Releasing

### From GitHub Actions (recommended)

*Actions* → **Release** → *Run workflow* on `main` ([`release.yml`](../.github/workflows/release.yml)),
or:

```bash
gh workflow run release.yml                       # automatic bump if needed (below)
gh workflow run release.yml -f bump=minor         # force patch, minor or major
gh workflow run release.yml -f version=1.3.0-rc.1 # exact version
gh workflow run release.yml -f dry_run=true       # show the version and diff only
```

The job:

1. Refuses to run on anything but `main`, or if CI hasn't passed on the commit.
2. If `v<version>` from `Cargo.toml` is already tagged, bumps it with
   [`scripts/bump-version.sh`](../scripts/bump-version.sh), which also updates `Cargo.lock`, and
   pushes `chore(release): bump version to X.Y.Z` to `main` as `github-actions[bot]`. A version
   that isn't released yet is released as is. With `bump=auto` (default) the bump comes from the
   [Conventional Commits](https://www.conventionalcommits.org/) since that tag (merge commits
   ignored); the biggest one wins, and the job log lists each commit's:

   | Commits                                               | `1.x.y` and up | `0.x.y`  |
   | ----------------------------------------------------- | -------------- | -------- |
   | breaking: `type!:` or a `BREAKING CHANGE:` footer     | major          | minor    |
   | `feat`, `chore`                                       | minor          | minor    |
   | anything else (`fix`, `docs`, ..., non-conventional)  | patch          | patch    |

   With no commits since the tag the job fails (nothing to release). `bump=patch|minor|major`
   forces a bump, `version` sets an exact one.
3. Runs `scripts/release.sh --yes` (below): tag + GitHub release with generated notes.
4. Runs CI on the new tag with `publish=true` and waits for it, so the job only goes green once
   the image is on Docker Hub. (A release created with the workflow's `GITHUB_TOKEN` doesn't
   trigger CI's `release` event; a manual run does.)

It uses the built-in `GITHUB_TOKEN` (no extra secret), so it needs Docker Hub settings above and
`main` must accept pushes from GitHub Actions: with branch protection or rulesets on `main`, allow
the GitHub Actions app to bypass them. The bump commit itself doesn't trigger a push CI run (also
a `GITHUB_TOKEN` effect); the run on the tag tests it. If the job fails after pushing the bump,
run CI on `main` (*Actions* → *CI* → *Run workflow*), then rerun **Release**: the bumped version
isn't released yet, so it's released as is.

To republish an existing release's image: *Actions* → *CI* → *Run workflow*, pick the tag, tick
*publish*.

### Locally

From an up-to-date, clean `main` with [`scripts/release.sh`](../scripts/release.sh)
(`--dry-run` to only check, `-y` to skip the prompt). It tags the current commit as
`v<version>` from `Cargo.toml` and runs `gh release create --generate-notes` (versions like
`1.3.0-rc.1` become pre-releases), and CI publishes the image on the `release` event. It refuses a
version that was already released, or a `Cargo.lock` that doesn't match: run
`scripts/bump-version.sh auto` (or `patch`, `minor`, `major`, an exact version), commit both files,
push, then rerun.

## GitHub token

Create a token at <https://github.com/settings/tokens>.

| Token type      | Required permissions                                                                 |
| --------------- | ------------------------------------------------------------------------------------ |
| Classic PAT     | `repo`                                                                               |
| Fine-grained    | Repository access: **All repositories**; Administration: **R/W**; Contents: **R/W**; Metadata: **R** |

"All repositories" is needed for fine-grained tokens because the repository may not exist yet.
If you create the repo manually first, a fine-grained token scoped to just that repo with
Contents R/W is enough.

Pass the token only via environment (`-e GH_TOKEN=...`, `--env-file`, or Docker secrets).
Do not bake it into the image.

Set an expiration you'll remember to rotate — when the token expires, the service will fail
preflight on the next restart and pushes will start failing (visible in the logs).

## Running

```bash
docker run -d \
  --name auto-git-commit \
  --restart unless-stopped \
  --hostname "$(hostname)" \
  --env-file .env \
  -v auto-git-commit-data:/data \
  auto-git-commit-tool
```

`--hostname` is optional; it makes notification emails name the machine instead of the
container ID.

`--restart unless-stopped` makes it survive host reboots, turning it into a de-facto system service.
The systemd install above is the recommended way; plain `docker run` is fine when you'd rather
not use systemd.

Check it:

```bash
docker logs -f auto-git-commit
```

## Notifications

Optional emails after each daily run and on clean stops, plus a dead-man's switch
(healthchecks.io) that alerts you when the machine is off: see the
[notifications tutorial](tutorials/notifications.md). Verify the settings with
`notify-test` (systemd + Docker:
`docker run --rm --env-file /etc/auto-git-commit-tool/env auto-git-commit-tool notify-test`, as
root).

Both units stop the service before the network goes down on shutdown (`After=network-online.target`),
so the "service stopped" email still goes out on a reboot.

## Making commits visible on your profile

1. Commit email must be linked to your account (default noreply address is).
2. Commits must land on the default branch (the service always uses it).
3. Enable **Private contributions** on your profile, since the repo is private
   ([tutorial](tutorials/enable-private-contributions.md)).
   Private contributions show as counts only; repository contents are not exposed.

## Running without Docker

Requirements: Rust toolchain, `git`, `gh`.

```bash
cargo build --release
export GH_TOKEN=$(gh auth token) DATA_DIR=$HOME/.local/share/auto-git-commit
./target/release/auto-git-commit-tool check    # validate setup
./target/release/auto-git-commit-tool status   # read-only report
./target/release/auto-git-commit-tool once     # today's run now (idempotent)
./target/release/auto-git-commit-tool          # daemon
```

Note: startup runs `gh auth setup-git`, which registers `gh` as git credential helper for
github.com in `~/.gitconfig`. The native systemd unit avoids touching your config by giving the
service its own `HOME`.

For a host service use `scripts/install.sh native` (see above).
