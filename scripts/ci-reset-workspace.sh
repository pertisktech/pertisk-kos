#!/usr/bin/env bash
# Empty a workspace that still has root-owned files from Docker (guest/image
# builds). actions/checkout cannot rmdir those as the runner user.
#
#   ./scripts/ci-reset-workspace.sh [dir]
set -euo pipefail

ws="${1:-${GITHUB_WORKSPACE:-$PWD}}"
if [[ ! -d "$ws" ]]; then
  echo "workspace not found: ${ws}"
  exit 0
fi

echo "==> reset workspace ${ws} (remove leftover Docker root files)"
if command -v docker >/dev/null 2>&1; then
  docker run --rm -v "${ws}:/w" alpine:3.20 \
    sh -c 'find /w -mindepth 1 -maxdepth 1 -exec rm -rf {} +'
  exit 0
fi
if command -v sudo >/dev/null 2>&1; then
  sudo rm -rf "${ws:?}"/* "${ws}"/.[!.]* "${ws}"/..?* 2>/dev/null || true
  exit 0
fi
echo "cannot reset ${ws}: need docker or sudo" >&2
exit 1
