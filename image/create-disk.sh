#!/usr/bin/env bash
# Create an empty raw disk image for QEMU install smoke tests.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

SIZE_GB="${PERTISK_DISK_GB:-8}"
DISK="${OUT}/pertisk-disk.raw"

echo "==> creating ${DISK} (${SIZE_GB}G)"
dd if=/dev/zero of="${DISK}" bs=1m count=$((SIZE_GB * 1024)) status=none
ls -lh "${DISK}"
echo "Attach with: ./image/run-qemu-disk.sh"
