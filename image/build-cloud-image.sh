#!/usr/bin/env bash
# Build a pre-installed cloud/QEMU disk image (raw + qcow2).
#
# Fast path: populate a small base disk (default 4G), convert to qcow2, then
# `qemu-img resize` to PERTISK_DISK_GB. EPHEMERAL is grown on first guest boot.
# This avoids mkfs/convert of 50–75G images (which hang Docker Desktop on macOS).
#
# Prerequisites: Docker, and boot assets (kernel + initramfs + EFI):
#   PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
#
# Usage:
#   ./image/build-cloud-image.sh
#   PERTISK_DISK_GB=75 PERTISK_ARCH=amd64 ./image/build-cloud-image.sh
#   PERTISK_BUILD_DISK_GB=4 PERTISK_DISK_GB=50 ./image/build-cloud-image.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
ASSETS="${OUT}/cloud-boot"
TARGET_GB="${PERTISK_DISK_GB:-8}"
# Populate+convert this many GiB only (must fit ESP+BOOT_A/B+META+STATE ≈ 3.1GiB).
BUILD_GB="${PERTISK_BUILD_DISK_GB:-4}"
if [[ "$TARGET_GB" -lt "$BUILD_GB" ]]; then
  BUILD_GB="$TARGET_GB"
fi
ARCH="${PERTISK_ARCH:-amd64}"
SEED="${PERTISK_SEED_CONFIG:-${ROOT}/examples/worker-cloud.yaml}"
DEFAULT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
VERSION="${PERTISK_VERSION:-${DEFAULT_VERSION:-0.1.0}}"
# Guest hostname: explicit env wins; else short GUID + role (cp|wk) from seed type.
short_host_id() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-' | cut -c1-6
  elif command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 3
  else
    echo "$(date +%s)$(echo $$)" | cksum | awk '{printf "%06x", $1 % 0x1000000}'
  fi
}
seed_host_role() {
  if [[ -n "${PERTISK_HOSTNAME_ROLE:-}" ]]; then
    echo "${PERTISK_HOSTNAME_ROLE}"
    return
  fi
  if [[ -f "${SEED}" ]] && grep -Eq '^[[:space:]]*type:[[:space:]]*controlplane[[:space:]]*$' "${SEED}"; then
    echo cp
  else
    echo wk
  fi
}
if [[ -n "${PERTISK_HOSTNAME:-}" ]]; then
  HOSTNAME_SEED="${PERTISK_HOSTNAME}"
elif [[ -n "${PROXMOX_VM_NAME:-}" ]]; then
  HOSTNAME_SEED="${PROXMOX_VM_NAME}"
else
  HOSTNAME_SEED="$(short_host_id)-$(seed_host_role)"
fi
RAW="${OUT}/pertisk-cloud-${ARCH}.raw"
QCOW="${OUT}/pertisk-cloud-${ARCH}.qcow2"

mkdir -p "${OUT}" "${ASSETS}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

DOCKER_NET=()
if [[ "$(uname -s)" == Linux ]]; then
  DOCKER_NET+=(--network host)
fi
APK_RETRY=( -v "${ROOT}/image/apk-retry.sh:/apk-retry.sh:ro" )

case "${ARCH}" in
  amd64)
    EFI_NAME=BOOTX64.EFI
    KERNEL_SRC="${OUT}/bzImage"
    INITRD_SRC="${OUT}/initramfs.cpio.gz"
    [[ -f "${INITRD_SRC}" ]] || INITRD_SRC="${OUT}/initramfs-amd64.cpio.gz"
    ;;
  arm64)
    EFI_NAME=BOOTAA64.EFI
    KERNEL_SRC="${OUT}/vmlinuz-arm64"
    INITRD_SRC="${OUT}/initramfs-arm64.cpio.gz"
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

# Populate/qemu-img never execute guest binaries. Always use the host CPU so
# we do not depend on QEMU binfmt (broken on the self-hosted runner).
case "$(uname -m)" in
  x86_64 | amd64) HOST_PLATFORM=linux/amd64 ;;
  aarch64 | arm64) HOST_PLATFORM=linux/arm64 ;;
  *) HOST_PLATFORM=linux/amd64 ;;
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

echo "==> creating sparse raw disk ${RAW} (${BUILD_GB}G populate; target ${TARGET_GB}G)"
rm -f "${RAW}" "${QCOW}"
# Block size as a byte count: GNU dd rejects BSD's 1m; BSD may reject GNU's 1M.
dd if=/dev/zero of="${RAW}" bs=1048576 count=0 seek=$((BUILD_GB * 1024)) status=none

echo "==> populating GPT / ESP / STATE (no loop devices), hostname=${HOSTNAME_SEED}"
# Bind the whole out/ dir. Populate writes partition images next to the raw disk
# and dd's them in — losetup is unavailable in Docker on this runner (no udev).
docker run --rm \
  --platform "${HOST_PLATFORM}" \
  ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
  -e PERTISK_DISK="/work/out/$(basename "${RAW}")" \
  -e PERTISK_BOOT_ASSETS=/work/boot \
  -e PERTISK_SEED_CONFIG=/work/config.yaml \
  -e PERTISK_ARCH="${ARCH}" \
  -e PERTISK_VERSION="${VERSION}" \
  -e PERTISK_HOSTNAME="${HOSTNAME_SEED}" \
  -v "${OUT}:/work/out" \
  -v "${ASSETS}:/work/boot:ro" \
  -v "${SEED}:/work/config.yaml:ro" \
  -v "${ROOT}/image/cloud/populate-disk.sh:/work/populate-disk.sh:ro" \
  ${APK_RETRY[@]+"${APK_RETRY[@]}"} \
  alpine:3.20 \
  sh -c 'sh /apk-retry.sh sgdisk e2fsprogs e2fsprogs-extra dosfstools mtools && sh /work/populate-disk.sh'

echo "==> converting qcow2 (${BUILD_GB}G)"
RAW_BASE="$(basename "${RAW}")"
QCOW_BASE="$(basename "${QCOW}")"
# qemu-img is arch-independent for convert/resize — always run in amd64 container.
if command -v qemu-img >/dev/null 2>&1; then
  qemu-img convert -p -f raw -O qcow2 "${RAW}" "${QCOW}"
else
  docker run --rm \
    --platform "${HOST_PLATFORM}" \
    ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
    -v "${OUT}:/out" \
    ${APK_RETRY[@]+"${APK_RETRY[@]}"} \
    alpine:3.20 \
    sh -c "sh /apk-retry.sh qemu-img && qemu-img convert -p -f raw -O qcow2 /out/${RAW_BASE} /out/${QCOW_BASE}"
fi
if [[ "${PERTISK_KEEP_RAW:-0}" != "1" ]]; then
  rm -f "${RAW}"
  echo "    removed ${RAW} (set PERTISK_KEEP_RAW=1 to retain)"
fi

if [[ "$TARGET_GB" -gt "$BUILD_GB" ]]; then
  echo "==> resizing qcow2 ${BUILD_GB}G → ${TARGET_GB}G (EPHEMERAL grows on first boot)"
  if command -v qemu-img >/dev/null 2>&1; then
    qemu-img resize "${QCOW}" "${TARGET_GB}G"
    qemu-img info "${QCOW}"
  else
    docker run --rm \
      --platform "${HOST_PLATFORM}" \
      ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
      -v "${OUT}:/out" \
      ${APK_RETRY[@]+"${APK_RETRY[@]}"} \
      alpine:3.20 \
      sh -c "sh /apk-retry.sh qemu-img && qemu-img resize /out/${QCOW_BASE} ${TARGET_GB}G && qemu-img info /out/${QCOW_BASE}"
  fi
else
  if command -v qemu-img >/dev/null 2>&1; then
    qemu-img info "${QCOW}"
  fi
fi

ls -lh "${QCOW}"
[[ -f "${RAW}" ]] && ls -lh "${RAW}" || true
echo "==> cloud image ready"
echo "    qcow: ${QCOW} (virtual ${TARGET_GB}G; populated ${BUILD_GB}G)"
[[ -f "${RAW}" ]] && echo "    raw:  ${RAW}"
echo "Boot (UEFI): PERTISK_DISK=${QCOW} ./image/run-qemu-uefi.sh  # or keep raw via PERTISK_KEEP_RAW=1"
echo "AWS: upload raw/qcow → import as AMI (uefi boot mode)"
echo "GCP: create image from raw tar; Azure: vhd convert from raw"
