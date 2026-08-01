#!/usr/bin/env bash
# Build amd64 + arm64 initramfs artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

DEFAULT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
VERSION="${PERTISK_VERSION:-${DEFAULT_VERSION}}"

PERTISK_VERSION="${VERSION}" PERTISK_PLATFORM=linux/amd64 "${ROOT}/image/build-initramfs.sh"
PERTISK_VERSION="${VERSION}" PERTISK_PLATFORM=linux/arm64 "${ROOT}/image/build-initramfs.sh"

echo "==> multi-arch artifacts (version=${VERSION})"
ls -lh "${ROOT}/out"/initramfs-*.cpio.gz
