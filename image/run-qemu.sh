#!/usr/bin/env bash
# Boot M1 initramfs under QEMU (serial console).
# Prerequisites: qemu-system-x86_64, out/bzImage, out/initramfs.cpio.gz
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
KERNEL="${OUT}/bzImage"
INITRD="${OUT}/initramfs.cpio.gz"

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "qemu-system-x86_64 not found. Install with: brew install qemu" >&2
  exit 1
fi

if [[ ! -f "${KERNEL}" ]]; then
  echo "missing ${KERNEL}; run: ./image/fetch-kernel.sh" >&2
  exit 1
fi

if [[ ! -f "${INITRD}" ]]; then
  echo "missing ${INITRD}; run: ./image/build-initramfs.sh" >&2
  exit 1
fi

# pertiskd --smoke exits after STATE+config; good for CI serial capture.
# Override with PERTISK_QEMU_CMDLINE if you want a long-running supervise loop.
APPEND="${PERTISK_QEMU_CMDLINE:-console=ttyS0 pertiskd.smoke=1}"

echo "==> qemu boot (Ctrl-A X to exit)"
exec qemu-system-x86_64 \
  -machine q35 \
  -cpu qemu64 \
  -m 512M \
  -nographic \
  -no-reboot \
  -kernel "${KERNEL}" \
  -initrd "${INITRD}" \
  -append "console=ttyS0 rdinit=/init -- --smoke --state-dir=/system/state"
