#!/usr/bin/env bash
# Boot initramfs with a virtio disk + user-mode NIC for M2 install/net smoke.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
KERNEL="${OUT}/bzImage"
INITRD="${OUT}/initramfs.cpio.gz"
DISK="${OUT}/pertisk-disk.raw"

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "qemu-system-x86_64 not found. Install with: brew install qemu" >&2
  exit 1
fi

[[ -f "${KERNEL}" ]] || { echo "missing kernel; run ./image/fetch-kernel.sh" >&2; exit 1; }
[[ -f "${INITRD}" ]] || { echo "missing initrd; run ./image/build-initramfs.sh" >&2; exit 1; }
[[ -f "${DISK}" ]] || { echo "missing disk; run ./image/create-disk.sh" >&2; exit 1; }

echo "==> qemu disk boot (Ctrl-A X to exit)"
exec qemu-system-x86_64 \
  -machine q35 \
  -cpu max \
  -m 1024M \
  -nographic \
  -no-reboot \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="${DISK}",if=virtio,format=raw \
  -kernel "${KERNEL}" \
  -initrd "${INITRD}" \
  -append "console=ttyS0 rdinit=/init -- --smoke"
