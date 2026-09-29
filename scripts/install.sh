#!/usr/bin/env bash
# Install auto-git-commit-tool as a systemd service.
#
#   scripts/install.sh [docker|native] [--reconfigure]
#
#   docker  (default) build the Docker image and run it from systemd.
#           Host requirements: docker, gh, systemd.
#   native  run a binary built on this machine (target/release, built with cargo if missing).
#           Host requirements: gh, git, systemd (+ cargo if the binary isn't built yet).
#
# Run it as your normal user: it uses sudo only for system changes, and reads the GitHub token
# from $GH_TOKEN or, if unset, from your logged-in gh CLI (`gh auth token`).
#
# Settings: any of REPO_NAME, COMMIT_TIME, MIN_COMMITS, MAX_COMMITS, LOG_FILE, GIT_AUTHOR_NAME,
# GIT_AUTHOR_EMAIL, RUN_ON_START, CATCH_UP, RUST_LOG exported when running this script are written
# to /etc/auto-git-commit-tool/env. An existing env file is kept unless --reconfigure is given.
set -euo pipefail

readonly SERVICE=auto-git-commit-tool
readonly IMAGE=auto-git-commit-tool:latest
readonly ENV_DIR=/etc/auto-git-commit-tool
readonly ENV_FILE=$ENV_DIR/env
readonly UNIT_FILE=/etc/systemd/system/$SERVICE.service
readonly BIN_DEST=/usr/local/bin/$SERVICE
readonly SETTINGS=(REPO_NAME COMMIT_TIME MIN_COMMITS MAX_COMMITS LOG_FILE GIT_AUTHOR_NAME
  GIT_AUTHOR_EMAIL RUN_ON_START CATCH_UP RUST_LOG)

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT

log() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not installed${2:+ ($2)}"; }

mode=docker
reconfigure=false
for arg in "$@"; do
  case $arg in
    docker | native) mode=$arg ;;
    --reconfigure) reconfigure=true ;;
    -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument '$arg' (try --help)" ;;
  esac
done

SUDO=()
if [[ $EUID -ne 0 ]]; then
  need sudo
  SUDO=(sudo)
fi

# --- Dependencies ----------------------------------------------------------------------------
need systemctl "systemd is required to run the service"
need gh "https://cli.github.com"
if [[ $mode == docker ]]; then
  need docker "https://docs.docker.com/engine/install/"
  DOCKER=(docker)
  if ! docker info >/dev/null 2>&1; then
    DOCKER=("${SUDO[@]}" docker)
    "${DOCKER[@]}" info >/dev/null 2>&1 || die "cannot talk to the Docker daemon; is it running?"
  fi
else
  need git
fi

# --- Token -----------------------------------------------------------------------------------
# The token only ever travels through environment variables and stdin, never argv.
if [[ -z ${GH_TOKEN:-} ]]; then
  gh auth status --hostname github.com >/dev/null 2>&1 \
    || die "GH_TOKEN is not set and gh is not logged in; run 'gh auth login' or export GH_TOKEN"
  GH_TOKEN=$(gh auth token --hostname github.com)
fi
export GH_TOKEN
login=$(gh api user --jq .login 2>/dev/null) || die "GitHub rejected the token"
log "GitHub account: $login"

# --- Build / install the program -------------------------------------------------------------
if [[ $mode == docker ]]; then
  log "Building Docker image $IMAGE"
  "${DOCKER[@]}" build --tag "$IMAGE" "$ROOT"
else
  bin=$ROOT/target/release/$SERVICE
  if [[ ! -x $bin ]]; then
    need cargo "the binary is not built yet; install Rust from https://rustup.rs"
    log "Building release binary"
    (cd "$ROOT" && cargo build --release --locked)
  fi
  log "Installing $bin -> $BIN_DEST"
  "${SUDO[@]}" install -m 0755 "$bin" "$BIN_DEST"
fi

# --- Environment file ------------------------------------------------------------------------
# The env dir is root-only (0700), so test for the file with sudo.
if "${SUDO[@]}" test -f "$ENV_FILE" && [[ $reconfigure == false ]]; then
  log "Keeping existing $ENV_FILE (use --reconfigure to rewrite it)"
else
  log "Writing $ENV_FILE (mode 600, root only)"
  {
    printf '# auto-git-commit-tool settings. See docs/configuration.md.\n'
    printf 'GH_TOKEN=%s\n' "$GH_TOKEN"
    for name in "${SETTINGS[@]}"; do
      if [[ -n ${!name:-} ]]; then printf '%s=%s\n' "$name" "${!name}"; fi
    done
  } | "${SUDO[@]}" sh -c "umask 077 && mkdir -p '$ENV_DIR' && cat > '$ENV_FILE'"
fi

# --- systemd unit ----------------------------------------------------------------------------
log "Installing $UNIT_FILE ($mode mode)"
sed -e "s|@DOCKER@|$(command -v docker || true)|g" -e "s|@BIN@|$BIN_DEST|g" \
  "$ROOT/deploy/systemd/$mode.service" | "${SUDO[@]}" tee "$UNIT_FILE" >/dev/null

"${SUDO[@]}" systemctl daemon-reload
"${SUDO[@]}" systemctl enable "$SERVICE" >/dev/null
"${SUDO[@]}" systemctl restart "$SERVICE"

log "Done. $SERVICE is running and will start on boot."
echo "    Logs:    journalctl -u $SERVICE -f"
if [[ $mode == docker ]]; then
  echo "    Status:  ${DOCKER[*]} exec $SERVICE auto-git-commit-tool status"
fi
echo "    Remove:  scripts/uninstall.sh [--purge]"
