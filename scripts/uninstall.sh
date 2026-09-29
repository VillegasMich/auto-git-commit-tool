#!/usr/bin/env bash
# Remove the auto-git-commit-tool systemd service.
#
#   scripts/uninstall.sh [--purge]
#
# By default the env file (token), the Docker image/volume and the local clone are kept, so a
# reinstall picks up where it left off. --purge deletes them too. The GitHub repository itself is
# never touched.
set -euo pipefail

readonly SERVICE=auto-git-commit-tool

purge=false
case ${1:-} in
  "") ;;
  --purge) purge=true ;;
  -h | --help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "error: unknown argument '$1'" >&2; exit 1 ;;
esac

SUDO=()
[[ $EUID -ne 0 ]] && SUDO=(sudo)

"${SUDO[@]}" systemctl disable --now "$SERVICE" 2>/dev/null || true
"${SUDO[@]}" rm -f "/etc/systemd/system/$SERVICE.service"
"${SUDO[@]}" systemctl daemon-reload
echo "==> Service removed"

if [[ $purge == true ]]; then
  "${SUDO[@]}" rm -rf /etc/auto-git-commit-tool "/usr/local/bin/$SERVICE" \
    "/var/lib/$SERVICE" "/var/lib/private/$SERVICE"
  if command -v docker >/dev/null 2>&1; then
    "${SUDO[@]}" docker rm --force "$SERVICE" >/dev/null 2>&1 || true
    "${SUDO[@]}" docker volume rm auto-git-commit-data >/dev/null 2>&1 || true
    "${SUDO[@]}" docker image rm auto-git-commit-tool:latest >/dev/null 2>&1 || true
  fi
  echo "==> Purged config, token, binary, local clone and Docker image/volume"
fi
