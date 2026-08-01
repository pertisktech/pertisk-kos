#!/usr/bin/env bash
# Fetch a prebuilt Linux kernel suitable for QEMU -kernel smoke tests.
# Usage:
#   ./image/fetch-kernel.sh                 # amd64 → out/bzImage
#   PERTISK_ARCH=arm64 ./image/fetch-kernel.sh  # → out/vmlinuz-arm64
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

ARCH="${PERTISK_ARCH:-amd64}"
case "${ARCH}" in
  amd64)
    PLATFORM=linux/amd64
    KERNEL_OUT="${OUT}/bzImage"
    ;;
  arm64)
    PLATFORM=linux/arm64
    KERNEL_OUT="${OUT}/vmlinuz-arm64"
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

if [[ -f "${KERNEL_OUT}" ]]; then
  echo "==> kernel already present: ${KERNEL_OUT}"
  ls -lh "${KERNEL_OUT}"
  exit 0
fi

echo "==> extracting linux virt kernel via alpine (${ARCH})"
docker run --rm --platform "${PLATFORM}" -v "${OUT}:/out" alpine:3.20 sh -c "
  set -e
  apk add --no-cache linux-virt >/dev/null
  img=\$(ls /boot/vmlinuz* | head -1)
  cp \"\$img\" /out/$(basename "${KERNEL_OUT}")
  echo \"copied \$img\"
"

ls -lh "${KERNEL_OUT}"
echo "==> wrote ${KERNEL_OUT}"
