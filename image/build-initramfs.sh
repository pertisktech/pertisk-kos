#!/usr/bin/env bash
# Build M1 initramfs (pertiskd as /init) via Docker and write to out/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build the initramfs" >&2
  exit 1
fi

# Ensure lockfile exists for reproducible Docker builds.
if [[ ! -f "${ROOT}/Cargo.lock" ]]; then
  (cd "${ROOT}" && cargo generate-lockfile)
fi

echo "==> building initramfs (x86_64 musl pertiskd)"
docker build \
  -f "${ROOT}/image/Dockerfile.initramfs" \
  --target export \
  -o "type=local,dest=${OUT}" \
  "${ROOT}"

echo "==> wrote ${OUT}/initramfs.cpio.gz"
ls -lh "${OUT}/initramfs.cpio.gz"
