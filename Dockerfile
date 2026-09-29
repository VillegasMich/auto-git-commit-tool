# syntax=docker/dockerfile:1

# Builder and runtime share the same Debian release so the binary's glibc matches.
FROM rust:1-slim-trixie AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# Build dependencies in their own layer so source edits don't rebuild them.
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
 && cargo build --release --locked \
 && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git gnupg tini \
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
ENV DATA_DIR=/data
VOLUME /data
# tini is PID 1: it forwards SIGTERM to the service and reaps orphaned git helper processes.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/auto-git-commit-tool"]
CMD ["daemon"]
