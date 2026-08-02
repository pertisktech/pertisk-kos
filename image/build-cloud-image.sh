#!/usr/bin/env bash
# Build a pre-installed cloud/QEMU disk image (raw + qcow2).
#
# Prerequisites: Docker, and boot assets (kernel + initramfs + EFI):
#   PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
#   # or: fetch-kernel + fetch-bootloader + stage, then copy initramfs
#
# Usage:
#   ./image/build-cloud-image.sh
#   PERTISK_DISK_GB=20 PERTISK_ARCH=amd64 ./image/build-cloud-image.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
ASSETS="${OUT}/cloud-boot"
SIZE_GB="${PERTISK_DISK_GB:-8}"
ARCH="${PERTISK_ARCH:-amd64}"
SEED="${PERTISK_SEED_CONFIG:-${ROOT}/examples/worker-cloud.yaml}"
# Guest hostname (defaults to Proxmox VM name convention).
HOSTNAME_SEED="${PERTISK_HOSTNAME:-${PROXMOX_VM_NAME:-pertisk-cp-1}}"
RAW="${OUT}/pertisk-cloud-${ARCH}.raw"
QCOW="${OUT}/pertisk-cloud-${ARCH}.qcow2"

mkdir -p "${OUT}" "${ASSETS}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

case "${ARCH}" in
  amd64)
    PLATFORM=linux/amd64
    EFI_NAME=BOOTX64.EFI
    KERNEL_SRC="${OUT}/bzImage"
    INITRD_SRC="${OUT}/initramfs.cpio.gz"
    [[ -f "${INITRD_SRC}" ]] || INITRD_SRC="${OUT}/initramfs-amd64.cpio.gz"
    ;;
  arm64)
    PLATFORM=linux/arm64
    EFI_NAME=BOOTAA64.EFI
    KERNEL_SRC="${OUT}/vmlinuz-arm64"
    INITRD_SRC="${OUT}/initramfs-arm64.cpio.gz"
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

echo "==> assembling boot assets"
[[ -f "${KERNEL_SRC}" ]] || {
  echo "missing ${KERNEL_SRC}; run ./image/fetch-kernel.sh" >&2
  exit 1
}
[[ -f "${OUT}/bootloader/${EFI_NAME}" ]] || {
  echo "missing bootloader; run ./image/fetch-bootloader.sh" >&2
  exit 1
}
[[ -f "${INITRD_SRC}" ]] || {
  echo "missing ${INITRD_SRC}; run ./image/build-initramfs.sh" >&2
  exit 1
}
[[ -f "${SEED}" ]] || {
  echo "missing seed config ${SEED}" >&2
  exit 1
}

cp "${KERNEL_SRC}" "${ASSETS}/kernel"
cp "${INITRD_SRC}" "${ASSETS}/initramfs"
cp "${OUT}/bootloader/${EFI_NAME}" "${ASSETS}/${EFI_NAME}"

echo "==> creating sparse raw disk ${RAW} (${SIZE_GB}G)"
rm -f "${RAW}" "${QCOW}"
dd if=/dev/zero of="${RAW}" bs=1m count=0 seek=$((SIZE_GB * 1024)) status=none

echo "==> populating GPT / ESP / STATE (Docker privileged), hostname=${HOSTNAME_SEED}"
docker run --rm --privileged \
  --platform "${PLATFORM}" \
  -e PERTISK_DISK=/work/disk.raw \
  -e PERTISK_BOOT_ASSETS=/work/boot \
  -e PERTISK_SEED_CONFIG=/work/config.yaml \
  -e PERTISK_ARCH="${ARCH}" \
  -e PERTISK_HOSTNAME="${HOSTNAME_SEED}" \
  -v "${RAW}:/work/disk.raw" \
  -v "${ASSETS}:/work/boot:ro" \
  -v "${SEED}:/work/config.yaml:ro" \
  -v "${ROOT}/image/cloud/populate-disk.sh:/work/populate-disk.sh:ro" \
  alpine:3.20 \
  sh -c 'apk add --no-cache sgdisk e2fsprogs dosfstools util-linux multipath-tools parted >/dev/null && sh /work/populate-disk.sh'

echo "==> converting qcow2"
docker run --rm --platform "${PLATFORM}" \
  -v "${OUT}:/out" \
  alpine:3.20 \
  sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img convert -f raw -O qcow2 /out/$(basename "${RAW}") /out/$(basename "${QCOW}") && qemu-img info /out/$(basename "${QCOW}")"

ls -lh "${RAW}" "${QCOW}"
echo "==> cloud image ready"
echo "    raw:  ${RAW}"
echo "    qcow: ${QCOW}"
echo "Boot (UEFI): PERTISK_DISK=${RAW} ./image/run-qemu-uefi.sh  # or point script at this disk"
echo "AWS: upload raw/qcow → import as AMI (uefi boot mode)"
echo "GCP: create image from raw tar; Azure: vhd convert from raw"
