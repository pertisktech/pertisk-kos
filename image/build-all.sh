#!/usr/bin/env bash
# Build amd64 + arm64 initramfs artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PERTISK_PLATFORM=linux/amd64 "${ROOT}/image/build-initramfs.sh"
PERTISK_PLATFORM=linux/arm64 "${ROOT}/image/build-initramfs.sh"

echo "==> multi-arch artifacts"
ls -lh "${ROOT}/out"/initramfs-*.cpio.gz
