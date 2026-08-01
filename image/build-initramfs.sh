#!/usr/bin/env bash
# Build initramfs for one or more architectures.
# Usage:
#   ./image/build-initramfs.sh              # linux/amd64
#   PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
#   ./image/build-all.sh                    # both
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
OVERLAY="${ROOT}/image/runtime-overlay"
mkdir -p "${OUT}" "${OVERLAY}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build the initramfs" >&2
  exit 1
fi

if [[ ! -f "${ROOT}/Cargo.lock" ]]; then
  (cd "${ROOT}" && cargo generate-lockfile)
fi

find "${OVERLAY}" -mindepth 1 ! -name '.keep' -exec rm -rf {} + 2>/dev/null || true
if [[ "${PERTISK_EMBED_RUNTIME:-0}" == "1" ]]; then
  if [[ ! -x "${OUT}/runtime/usr/local/bin/containerd" ]]; then
    echo "PERTISK_EMBED_RUNTIME=1 but out/runtime missing; run ./image/fetch-runtime.sh" >&2
    exit 1
  fi
  echo "==> embedding runtime binaries into initramfs"
  cp -a "${OUT}/runtime/." "${OVERLAY}/"
fi

PLATFORM="${PERTISK_PLATFORM:-linux/amd64}"
case "${PLATFORM}" in
  linux/amd64) ARCH_SUFFIX=amd64 ;;
  linux/arm64) ARCH_SUFFIX=arm64 ;;
  *) echo "unsupported PERTISK_PLATFORM=${PLATFORM}" >&2; exit 1 ;;
esac

ARTIFACT="${OUT}/initramfs-${ARCH_SUFFIX}.cpio.gz"

echo "==> building initramfs (platform=${PLATFORM})"
docker build \
  --platform "${PLATFORM}" \
  -f "${ROOT}/image/Dockerfile.initramfs" \
  --target export \
  -o "type=local,dest=${OUT}/.initramfs-tmp-${ARCH_SUFFIX}" \
  "${ROOT}"

mv "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}/initramfs.cpio.gz" "${ARTIFACT}"
rm -rf "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}"
# Keep legacy name for amd64 QEMU scripts.
if [[ "${ARCH_SUFFIX}" == "amd64" ]]; then
  cp "${ARTIFACT}" "${OUT}/initramfs.cpio.gz"
fi

echo "==> wrote ${ARTIFACT}"
ls -lh "${ARTIFACT}"
