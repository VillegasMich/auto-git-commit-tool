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
3. Builds the image (`auto-git-commit-tool:latest`), or installs `target/release/auto-git-commit-tool`
   to `/usr/local/bin` (building it with cargo if it isn't built yet).
4. Writes `/etc/auto-git-commit-tool/env` (root-only, mode 600) with `GH_TOKEN` and any exported
   settings. An existing file is kept unless you pass `--reconfigure`.
5. Installs `/etc/systemd/system/auto-git-commit-tool.service` from
   [`deploy/systemd/`](../deploy/systemd/), enables it on boot and (re)starts it.

Re-run it after pulling new code to rebuild and restart. Switching mode is just running it with the
other mode (the unit name stays the same).

```bash
systemctl status auto-git-commit-tool
journalctl -u auto-git-commit-tool -f
sudo docker exec auto-git-commit-tool auto-git-commit-tool status   # docker mode
scripts/uninstall.sh            # remove the service, keep token/data
scripts/uninstall.sh --purge    # also delete env file, binary, clone, image and volume
```

To change settings later: `sudoedit /etc/auto-git-commit-tool/env && sudo systemctl restart auto-git-commit-tool`.

### Units

- **docker** – `Requires=docker.service`, removes a stale container left by an unclean stop,
  runs the container in the foreground with `--env-file` and the `auto-git-commit-data` volume,
  `Restart=always`. `systemctl stop` → `docker stop --time 30` → `SIGTERM` to the service.
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
  --env-file .env \
  -v auto-git-commit-data:/data \
  auto-git-commit-tool
```

`--restart unless-stopped` makes it survive host reboots, turning it into a de-facto system service.
The systemd install above is the recommended way; plain `docker run` is fine when you'd rather
not use systemd.

Check it:

```bash
docker logs -f auto-git-commit
```

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
