#!/usr/bin/env bash
# Fetch a prebuilt Linux bzImage suitable for QEMU -kernel smoke tests.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

# Alpine virt kernel (small, virtio-friendly). Pin a known release tarball asset.
# Fallback: copy from alpine package index via docker if curl URL changes.
KERNEL_OUT="${OUT}/bzImage"

if [[ -f "${KERNEL_OUT}" ]]; then
  echo "==> kernel already present: ${KERNEL_OUT}"
  ls -lh "${KERNEL_OUT}"
  exit 0
fi

echo "==> extracting linux virt kernel via alpine container"
docker run --rm -v "${OUT}:/out" alpine:3.20 sh -c '
  set -e
  apk add --no-cache linux-virt >/dev/null
  # Alpine installs vmlinuz under /boot
  img=$(ls /boot/vmlinuz* | head -1)
  cp "$img" /out/bzImage
  echo "copied $img"
'

ls -lh "${KERNEL_OUT}"
echo "==> wrote ${KERNEL_OUT}"
