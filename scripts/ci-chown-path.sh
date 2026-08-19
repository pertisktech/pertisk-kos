#!/usr/bin/env bash
# chown a directory tree back to the invoking user after Docker wrote as root.
#
#   ./scripts/ci-chown-path.sh /path/to/out
set -euo pipefail

target="${1:?path required}"
if [[ ! -e "$target" ]]; then
  exit 0
fi
if [[ "$(uname -s)" != Linux ]]; then
  exit 0
fi
uid="$(id -u)"
gid="$(id -g)"
if command -v docker >/dev/null 2>&1; then
  docker run --rm \
    -v "${target}:/t" \
    alpine:3.20 \
    chown -R "${uid}:${gid}" /t
  exit 0
fi
if command -v sudo >/dev/null 2>&1; then
  sudo chown -R "${uid}:${gid}" "$target"
fi
