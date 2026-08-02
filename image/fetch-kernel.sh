#!/usr/bin/env bash
# Fetch a prebuilt Linux kernel + essential virtio modules (Alpine linux-virt).
# Usage:
#   ./image/fetch-kernel.sh                 # amd64 → out/bzImage + out/modules-amd64/
#   PERTISK_ARCH=arm64 ./image/fetch-kernel.sh  # → out/vmlinuz-arm64 + out/modules-arm64/
#   PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh  # re-download
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

ARCH="${PERTISK_ARCH:-amd64}"
case "${ARCH}" in
  amd64)
    PLATFORM=linux/amd64
    KERNEL_OUT="${OUT}/bzImage"
    MODULES_OUT="${OUT}/modules-amd64"
    ;;
  arm64)
    PLATFORM=linux/arm64
    KERNEL_OUT="${OUT}/vmlinuz-arm64"
    MODULES_OUT="${OUT}/modules-arm64"
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

NEED_KERNEL=1
NEED_MODULES=1
if [[ "${PERTISK_FORCE_KERNEL:-0}" != "1" ]]; then
  [[ -f "${KERNEL_OUT}" ]] && NEED_KERNEL=0
  [[ -f "${MODULES_OUT}/virtio_net.ko" && -f "${MODULES_OUT}/sd_mod.ko" && -f "${MODULES_OUT}/overlay.ko" && -f "${MODULES_OUT}/version" ]] && NEED_MODULES=0
fi

# Kernel and modules must come from the same linux-virt package (vermagic).
if [[ "${NEED_MODULES}" == "1" ]]; then
  NEED_KERNEL=1
fi

if [[ "${NEED_KERNEL}" == "0" && "${NEED_MODULES}" == "0" ]]; then
  echo "==> kernel + modules already present"
  ls -lh "${KERNEL_OUT}"
  ls -lh "${MODULES_OUT}"
  exit 0
fi

echo "==> extracting linux-virt kernel/modules via alpine (${ARCH})"
docker run --rm --platform "${PLATFORM}" \
  -v "${OUT}:/out" \
  -e "NEED_KERNEL=${NEED_KERNEL}" \
  -e "NEED_MODULES=${NEED_MODULES}" \
  -e "KERNEL_NAME=$(basename "${KERNEL_OUT}")" \
  -e "MODULES_NAME=$(basename "${MODULES_OUT}")" \
  alpine:3.20 sh -c '
  set -e
  apk add --no-cache linux-virt gzip >/dev/null
  KVER=$(ls /lib/modules | head -1)
  echo "KVER=$KVER"

  if [ "${NEED_KERNEL}" = "1" ]; then
    img=$(ls /boot/vmlinuz* | head -1)
    cp "$img" "/out/${KERNEL_NAME}"
    echo "copied kernel $img"
  fi

  if [ "${NEED_MODULES}" = "1" ]; then
    rm -rf "/out/${MODULES_NAME}"
    mkdir -p "/out/${MODULES_NAME}"
    # Order matters for loading: failover → net_failover → virtio_net.
    # Disk: virtio_scsi (Proxmox scsi) / virtio_blk (QEMU) + sd_mod (SCSI disk nodes).
    # overlay: containerd snapshotter / runc rootfs.
    for name in failover net_failover virtio_net virtio_scsi virtio_blk sd_mod overlay; do
      src=$(find "/lib/modules/${KVER}" -name "${name}.ko.gz" -o -name "${name}.ko" | head -1)
      if [ -z "$src" ]; then
        echo "WARNING: module ${name} not found" >&2
        continue
      fi
      case "$src" in
        *.gz) gzip -dc "$src" > "/out/${MODULES_NAME}/${name}.ko" ;;
        *) cp "$src" "/out/${MODULES_NAME}/${name}.ko" ;;
      esac
      echo "module ${name} <- $src"
    done
    printf "%s\n" "$KVER" > "/out/${MODULES_NAME}/version"
  fi
'

ls -lh "${KERNEL_OUT}"
ls -lh "${MODULES_OUT}"
echo "==> wrote ${KERNEL_OUT} and ${MODULES_OUT}"
