# Deployment

The service is designed to run as a Docker container on any machine (home server, VPS,
Raspberry Pi, NAS…). Only Docker and a GitHub token are needed on the host.

## Docker image

Multi-stage build:

1. **Builder** – `rust:<version>-slim`, `cargo build --release`.
2. **Runtime** – `debian:bookworm-slim` with:
   - `ca-certificates`
   - `git`
   - `gh` (installed from the official GitHub CLI apt repository)
   - the compiled binary as the entrypoint

Reference `Dockerfile`:

```dockerfile
FROM rust:1-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git gnupg \
 && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
      -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
 && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
      > /etc/apt/sources.list.d/github-cli.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends gh \
 && apt-get purge -y curl gnupg && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/auto-git-commit-tool /usr/local/bin/auto-git-commit-tool
RUN useradd -m -u 1000 app && mkdir /data && chown app /data
USER app
VOLUME /data
ENTRYPOINT ["/usr/local/bin/auto-git-commit-tool"]
```

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
GH_TOKEN=... DATA_DIR=$HOME/.local/share/auto-git-commit ./target/release/auto-git-commit-tool
```

For a host service, wrap it in a systemd unit with `Restart=always` and an `EnvironmentFile=`.
