#!/usr/bin/env bash
# Stage kernel + systemd-boot into image/boot-overlay for initramfs embed.
# Initramfs itself is injected during the Docker pack stage.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
OVERLAY="${ROOT}/image/boot-overlay/usr/lib/pertisk/boot"
ARCH="${PERTISK_ARCH:-amd64}"

mkdir -p "${OVERLAY}"

case "${ARCH}" in
  amd64)
    KERNEL="${OUT}/bzImage"
    EFI_NAME=BOOTX64.EFI
    ;;
  arm64)
    KERNEL="${OUT}/vmlinuz-arm64"
    EFI_NAME=BOOTAA64.EFI
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

[[ -f "${KERNEL}" ]] || { echo "missing ${KERNEL}; run ./image/fetch-kernel.sh" >&2; exit 1; }
[[ -f "${OUT}/bootloader/${EFI_NAME}" ]] || {
  echo "missing bootloader; run ./image/fetch-bootloader.sh" >&2
  exit 1
}

cp "${KERNEL}" "${OVERLAY}/kernel"
cp "${OUT}/bootloader/${EFI_NAME}" "${OVERLAY}/${EFI_NAME}"
echo "==> staged boot assets in ${OVERLAY}"
ls -lh "${OVERLAY}"
