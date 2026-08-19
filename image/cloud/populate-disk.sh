#!/usr/bin/env bash
# Populate a raw disk image with Pertisk GPT layout, systemd-boot ESP, and STATE.
# Runs in Docker without loop devices (mtools + debugfs + dd). See build-cloud-image.sh.
set -euo pipefail

DISK="${PERTISK_DISK:?PERTISK_DISK required}"
BOOT_ASSETS="${PERTISK_BOOT_ASSETS:?PERTISK_BOOT_ASSETS required}"
SEED_CONFIG="${PERTISK_SEED_CONFIG:?PERTISK_SEED_CONFIG required}"
ARCH="${PERTISK_ARCH:-amd64}"
VERSION="${PERTISK_VERSION:-0.1.0}"

case "${ARCH}" in
  amd64) EFI_NAME=BOOTX64.EFI ;;
  arm64) EFI_NAME=BOOTAA64.EFI ;;
  *) echo "unsupported arch ${ARCH}" >&2; exit 1 ;;
esac

for f in kernel initramfs "${EFI_NAME}"; do
  [[ -f "${BOOT_ASSETS}/${f}" ]] || {
    echo "missing ${BOOT_ASSETS}/${f}" >&2
    exit 1
  }
done
[[ -f "${SEED_CONFIG}" ]] || {
  echo "missing seed config ${SEED_CONFIG}" >&2
  exit 1
}

echo "==> partitioning ${DISK}"
sgdisk --zap-all "${DISK}" >/dev/null
sgdisk -o "${DISK}" >/dev/null
# Sizes match crates/pertisk-disk plan defaults (MiB).
# ESP must fit systemd-boot + kernel + runtime-embedded initramfs (~170MiB+).
ESP_MIB="${PERTISK_ESP_MIB:-512}"
BOOT_MIB="${PERTISK_BOOT_MIB:-768}"
sgdisk -n "1:0:+${ESP_MIB}M" -t 1:EF00 -c 1:EFI "${DISK}" >/dev/null
sgdisk -n "2:0:+${BOOT_MIB}M" -t 2:8300 -c 2:BOOT_A "${DISK}" >/dev/null
sgdisk -n "3:0:+${BOOT_MIB}M" -t 3:8300 -c 3:BOOT_B "${DISK}" >/dev/null
sgdisk -n 4:0:+32M -t 4:8300 -c 4:META "${DISK}" >/dev/null
sgdisk -n 5:0:+1024M -t 5:8300 -c 5:STATE "${DISK}" >/dev/null
sgdisk -n 6:0:0 -t 6:8300 -c 6:EPHEMERAL "${DISK}" >/dev/null
sgdisk -p "${DISK}" || true

# Fail early if boot assets cannot fit on ESP (vfat usable ≈ partition size).
# Use [ ] not (( )) — Alpine/busybox ash treats ((var)) poorly ("NEED_BYTES: not found").
asset_bytes() { wc -c <"$1" | tr -d ' \n'; }
NEED_BYTES=$(( $(asset_bytes "${BOOT_ASSETS}/kernel") \
  + $(asset_bytes "${BOOT_ASSETS}/initramfs") \
  + $(asset_bytes "${BOOT_ASSETS}/${EFI_NAME}") \
  + 2 * 1024 * 1024 ))
ESP_BYTES=$((ESP_MIB * 1024 * 1024))
if [ "${NEED_BYTES}" -gt "${ESP_BYTES}" ]; then
  echo "boot assets (~$((NEED_BYTES / 1024 / 1024))MiB) exceed ESP (${ESP_MIB}MiB); set PERTISK_ESP_MIB=..." >&2
  exit 1
fi

# Do not use losetup. Docker on self-hosted runners often has /dev/loop-control
# but no udev, so LOOP_CTL_GET_FREE allocates loop0 and then fails with
# "device node /dev/loop0 (7:0) is lost". Format partition-sized files, fill
# them with mtools/debugfs, and dd them into the GPT image.
STAGING="$(dirname "${DISK}")/.populate-$$"
mkdir -p "${STAGING}"
cleanup() { rm -rf "${STAGING}"; }
trap cleanup EXIT

part_first() {
  sgdisk -i "$1" "${DISK}" | sed -n 's/^First sector: \([0-9][0-9]*\).*/\1/p' | head -1
}

part_last() {
  sgdisk -i "$1" "${DISK}" | sed -n 's/^Last sector: \([0-9][0-9]*\).*/\1/p' | head -1
}

part_bytes() {
  local first last
  first="$(part_first "$1")"
  last="$(part_last "$1")"
  if [ -z "${first}" ] || [ -z "${last}" ]; then
    echo "could not read GPT partition $1 on ${DISK}" >&2
    sgdisk -i "$1" "${DISK}" >&2 || true
    exit 1
  fi
  echo $(( (last - first + 1) * 512 ))
}

# GPT partitions are 1MiB-aligned (2048 sectors).
burn_part() {
  local n="$1" img="$2"
  local first seek
  first="$(part_first "$n")"
  seek=$((first / 2048))
  echo "    dd partition ${n} → ${DISK} @ ${seek}MiB"
  dd if="${img}" of="${DISK}" bs=1048576 seek="${seek}" conv=notrunc
  rm -f "${img}"
}

make_fat() {
  local img="$1" label="$2" bytes="$3"
  truncate -s "${bytes}" "${img}"
  mkfs.vfat -F 32 -n "${label}" "${img}" >/dev/null
}

make_ext4() {
  local img="$1" label="$2" bytes="$3"
  truncate -s "${bytes}" "${img}"
  mkfs.ext4 -F -q -L "${label}" -E nodiscard "${img}"
}

debugfs_apply() {
  local img="$1" cmds="${STAGING}/debugfs.cmd"
  cat >"${cmds}"
  if ! debugfs -w -f "${cmds}" "${img}"; then
    echo "debugfs failed on ${img}:" >&2
    cat "${cmds}" >&2
    exit 1
  fi
}

export MTOOLS_SKIP_CHECK=1

echo "==> formatting partitions (no loop devices)"
ESP_IMG="${STAGING}/esp.img"
BOOTA_IMG="${STAGING}/boot-a.img"
BOOTB_IMG="${STAGING}/boot-b.img"
META_IMG="${STAGING}/meta.img"
STATE_IMG="${STAGING}/state.img"
make_fat "${ESP_IMG}" EFI "$(part_bytes 1)"
make_ext4 "${BOOTA_IMG}" BOOT_A "$(part_bytes 2)"
make_ext4 "${BOOTB_IMG}" BOOT_B "$(part_bytes 3)"
make_ext4 "${META_IMG}" META "$(part_bytes 4)"
make_ext4 "${STATE_IMG}" STATE "$(part_bytes 5)"
echo "    EPHEMERAL (unformatted; mkfs on first boot after grow)"

echo "==> ESP systemd-boot + slot A"
for d in EFI EFI/BOOT EFI/systemd loader loader/entries pertisk pertisk/A; do
  mmd -i "${ESP_IMG}" "::${d}"
done
mcopy -i "${ESP_IMG}" "${BOOT_ASSETS}/${EFI_NAME}" "::EFI/BOOT/${EFI_NAME}"
case "${ARCH}" in
  amd64) mcopy -i "${ESP_IMG}" "${BOOT_ASSETS}/${EFI_NAME}" ::EFI/systemd/systemd-bootx64.efi ;;
  arm64) mcopy -i "${ESP_IMG}" "${BOOT_ASSETS}/${EFI_NAME}" ::EFI/systemd/systemd-bootaa64.efi ;;
esac
mcopy -i "${ESP_IMG}" "${BOOT_ASSETS}/kernel" ::pertisk/A/kernel
mcopy -i "${ESP_IMG}" "${BOOT_ASSETS}/initramfs" ::pertisk/A/initramfs

echo "==> BOOT_A kernel + initramfs"
debugfs_apply "${BOOTA_IMG}" <<EOF
write ${BOOT_ASSETS}/kernel kernel
write ${BOOT_ASSETS}/initramfs initramfs
EOF

# Last console= becomes /dev/console for userspace.
# amd64/Proxmox serial: ttyS0. aarch64 virt (PL011): ttyAMA0 (keep ttyS0 too for Proxmox serial0).
case "${ARCH}" in
  arm64|aarch64)
    # ttyAMA0 = PL011 (QEMU/Proxmox virt). earlycon so EFI→kernel handoff is visible.
    # arm64.nopauth: host CPUs with QARMA3 PAuth (e.g. Cortex-A720) can trip older
    # userspace; disable PAC in the guest kernel as a safe default for virt.
    CMDLINE="${PERTISK_CMDLINE:-earlycon=pl011,0x09000000 console=tty0 console=ttyAMA0 console=ttyS0 arm64.nopauth rdinit=/init}"
    ;;
  *)
    CMDLINE="${PERTISK_CMDLINE:-console=tty0 console=ttyS0 rdinit=/init}"
    ;;
esac
cat >"${STAGING}/pertisk-a.conf" <<EOF
title Pertisk KOS (slot A)
linux /pertisk/A/kernel
initrd /pertisk/A/initramfs
options ${CMDLINE}
EOF
# timeout 0: skip systemd-boot menu countdown. With Proxmox vga=serial0 the
# menu/countdown text is garbled on Serial; auto-boot is correct for cloud.
cat >"${STAGING}/loader.conf" <<EOF
default pertisk-a.conf
timeout 0
console-mode keep
editor no
EOF
mcopy -i "${ESP_IMG}" "${STAGING}/pertisk-a.conf" ::loader/entries/pertisk-a.conf
mcopy -i "${ESP_IMG}" "${STAGING}/loader.conf" ::loader/loader.conf

echo "==> STATE seed"
cp "${SEED_CONFIG}" "${STAGING}/config.yaml"
if [[ -n "${PERTISK_HOSTNAME:-}" ]]; then
  if command -v sed >/dev/null 2>&1; then
    sed -i "s/^\\([[:space:]]*hostname:[[:space:]]*\\).*/\\1${PERTISK_HOSTNAME}/" \
      "${STAGING}/config.yaml"
    echo "    hostname -> ${PERTISK_HOSTNAME}"
  fi
fi
cat >"${STAGING}/boot-meta.json" <<EOF
{
  "active": "a",
  "next": "a",
  "previous_good": "a",
  "boot_attempts": 0,
  "boot_ok": true,
  "pending_version": null,
  "active_version": "${VERSION}"
}
EOF
cat >"${STAGING}/image.json" <<EOF
{
  "format": "pertisk-cloud",
  "arch": "${ARCH}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
debugfs_apply "${STATE_IMG}" <<EOF
mkdir machine
mkdir secrets
mkdir log
mkdir slots
write ${STAGING}/config.yaml config.yaml
write ${STAGING}/boot-meta.json boot-meta.json
cd machine
write ${STAGING}/image.json image.json
EOF

echo "==> writing partitions into ${DISK}"
burn_part 1 "${ESP_IMG}"
burn_part 2 "${BOOTA_IMG}"
burn_part 3 "${BOOTB_IMG}"
burn_part 4 "${META_IMG}"
burn_part 5 "${STATE_IMG}"

echo "==> done"
ls -lh "${DISK}"
sgdisk -p "${DISK}" || true
