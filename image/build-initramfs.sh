#!/usr/bin/env bash
# Build initramfs for one or more architectures.
# Usage:
#   ./image/build-initramfs.sh
#   PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
#   PERTISK_VERSION=0.2.0 PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
#   PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
#   make build VERSION=0.2.0 ARCH=arm64
#   ./image/build-all.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
OVERLAY="${ROOT}/image/runtime-overlay"
BOOT_OVERLAY="${ROOT}/image/boot-overlay"
mkdir -p "${OUT}" "${OVERLAY}" "${BOOT_OVERLAY}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build the initramfs" >&2
  exit 1
fi

if [[ ! -f "${ROOT}/Cargo.lock" ]]; then
  (cd "${ROOT}" && cargo generate-lockfile)
fi

# Version baked into pertiskd via PERTISK_BUILD_VERSION. Default: workspace Cargo.toml.
DEFAULT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
VERSION="${PERTISK_VERSION:-${DEFAULT_VERSION}}"

find "${OVERLAY}" -mindepth 1 ! -name '.keep' -exec rm -rf {} + 2>/dev/null || true
find "${BOOT_OVERLAY}" -mindepth 1 ! -name '.keep' -exec rm -rf {} + 2>/dev/null || true

if [[ "${PERTISK_EMBED_RUNTIME:-0}" == "1" ]]; then
  if [[ ! -x "${OUT}/runtime/usr/local/bin/containerd" ]]; then
    echo "PERTISK_EMBED_RUNTIME=1 but out/runtime missing; run ./image/fetch-runtime.sh" >&2
    exit 1
  fi
  echo "==> embedding runtime binaries into initramfs"
  cp -a "${OUT}/runtime/." "${OVERLAY}/"
fi

# Prefer PERTISK_ARCH (amd64|arm64); fall back to PERTISK_PLATFORM (linux/...).
if [[ -n "${PERTISK_ARCH:-}" ]]; then
  case "${PERTISK_ARCH}" in
    amd64|x86_64) ARCH_SUFFIX=amd64; PLATFORM=linux/amd64 ;;
    arm64|aarch64) ARCH_SUFFIX=arm64; PLATFORM=linux/arm64 ;;
    *) echo "unsupported PERTISK_ARCH=${PERTISK_ARCH}" >&2; exit 1 ;;
  esac
else
  PLATFORM="${PERTISK_PLATFORM:-linux/amd64}"
  case "${PLATFORM}" in
    linux/amd64) ARCH_SUFFIX=amd64 ;;
    linux/arm64) ARCH_SUFFIX=arm64 ;;
    *) echo "unsupported PERTISK_PLATFORM=${PLATFORM}" >&2; exit 1 ;;
  esac
fi

if [[ "${PERTISK_EMBED_BOOT:-0}" == "1" ]]; then
  echo "==> staging installer boot assets (kernel + systemd-boot)"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/fetch-kernel.sh"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/fetch-bootloader.sh"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/stage-boot-assets.sh"
fi

ARTIFACT="${OUT}/initramfs-${ARCH_SUFFIX}.cpio.gz"
VERSIONED="${OUT}/initramfs-${ARCH_SUFFIX}-v${VERSION}.cpio.gz"

echo "==> building initramfs (version=${VERSION} platform=${PLATFORM})"
docker build \
  --platform "${PLATFORM}" \
  --build-arg "VERSION=${VERSION}" \
  -f "${ROOT}/image/Dockerfile.initramfs" \
  --target export \
  -o "type=local,dest=${OUT}/.initramfs-tmp-${ARCH_SUFFIX}" \
  "${ROOT}"

mv "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}/initramfs.cpio.gz" "${ARTIFACT}"
rm -rf "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}"
cp "${ARTIFACT}" "${VERSIONED}"
# Keep legacy name for amd64 QEMU scripts.
if [[ "${ARCH_SUFFIX}" == "amd64" ]]; then
  cp "${ARTIFACT}" "${OUT}/initramfs.cpio.gz"
fi

echo "==> wrote ${ARTIFACT}"
echo "==> wrote ${VERSIONED}"
ls -lh "${ARTIFACT}" "${VERSIONED}"
