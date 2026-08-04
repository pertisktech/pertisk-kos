#!/usr/bin/env bash
# Populate a raw disk image with Pertisk GPT layout, systemd-boot ESP, and STATE.
# Intended to run as root inside a Linux container (see build-cloud-image.sh).
set -euo pipefail

DISK="${PERTISK_DISK:?PERTISK_DISK required}"
BOOT_ASSETS="${PERTISK_BOOT_ASSETS:?PERTISK_BOOT_ASSETS required}"
SEED_CONFIG="${PERTISK_SEED_CONFIG:?PERTISK_SEED_CONFIG required}"
ARCH="${PERTISK_ARCH:-amd64}"

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

# Map partitions via loop + partx (partition nodes are not always auto-created).
LOOP="$(losetup --find --show -P "${DISK}")"
cleanup() {
  sync || true
  for d in /mnt/pertisk-efi /mnt/pertisk-state /mnt/pertisk-boot-a /mnt/pertisk-meta; do
    umount "$d" 2>/dev/null || true
  done
  # Detach any kpartx mappings first.
  kpartx -d "${LOOP}" 2>/dev/null || true
  losetup -d "${LOOP}" 2>/dev/null || true
}
trap cleanup EXIT

partprobe "${LOOP}" 2>/dev/null || true
partx -u "${LOOP}" 2>/dev/null || true
kpartx -av "${LOOP}" 2>/dev/null || true

# Resolve partition device nodes (losetup -P vs kpartx /dev/mapper).
part_dev() {
  local n="$1"
  if [[ -e "${LOOP}p${n}" ]]; then
    echo "${LOOP}p${n}"
  elif [[ -e "/dev/mapper/$(basename "${LOOP}")p${n}" ]]; then
    echo "/dev/mapper/$(basename "${LOOP}")p${n}"
  else
    return 1
  fi
}

for i in $(seq 1 50); do
  part_dev 1 >/dev/null 2>&1 && break
  sleep 0.1
done
P1="$(part_dev 1)" || {
  echo "loop partitions did not appear for ${LOOP}" >&2
  ls -la "${LOOP}"* /dev/mapper 2>/dev/null || true
  exit 1
}
P2="$(part_dev 2)"
P3="$(part_dev 3)"
P4="$(part_dev 4)"
P5="$(part_dev 5)"
P6="$(part_dev 6)"

echo "==> formatting"
# Small partitions: quiet + nodiscard (avoids thrashing sparse bind-mounts on Docker Desktop).
mkfs.vfat -F 32 -n EFI "${P1}"
mkfs.ext4 -F -q -L BOOT_A -E nodiscard "${P2}"
mkfs.ext4 -F -q -L BOOT_B -E nodiscard "${P3}"
mkfs.ext4 -F -q -L META -E nodiscard "${P4}"
mkfs.ext4 -F -q -L STATE -E nodiscard "${P5}"
# Leave EPHEMERAL unformatted. Fast cloud builds populate a small disk then
# `qemu-img resize`; first guest boot grows GPT and mkfs.ext4 at final size
# (resize2fs after largefile4 keeps too few inodes → ENOSPC on image pulls).
echo "    EPHEMERAL (unformatted; mkfs on first boot after grow)"

mkdir -p /mnt/pertisk-efi /mnt/pertisk-state /mnt/pertisk-boot-a /mnt/pertisk-meta
mount "${P1}" /mnt/pertisk-efi
mount "${P5}" /mnt/pertisk-state
mount "${P2}" /mnt/pertisk-boot-a
mount "${P4}" /mnt/pertisk-meta
# Do not mount EPHEMERAL during populate — nothing is seeded there.
echo "==> ESP systemd-boot + slot A"
mkdir -p /mnt/pertisk-efi/EFI/BOOT /mnt/pertisk-efi/EFI/systemd \
  /mnt/pertisk-efi/loader/entries /mnt/pertisk-efi/pertisk/A
cp "${BOOT_ASSETS}/${EFI_NAME}" "/mnt/pertisk-efi/EFI/BOOT/${EFI_NAME}"
case "${ARCH}" in
  amd64) cp "${BOOT_ASSETS}/${EFI_NAME}" /mnt/pertisk-efi/EFI/systemd/systemd-bootx64.efi ;;
  arm64) cp "${BOOT_ASSETS}/${EFI_NAME}" /mnt/pertisk-efi/EFI/systemd/systemd-bootaa64.efi ;;
esac
cp "${BOOT_ASSETS}/kernel" /mnt/pertisk-efi/pertisk/A/kernel
cp "${BOOT_ASSETS}/initramfs" /mnt/pertisk-efi/pertisk/A/initramfs
# Also stage on BOOT_A partition for future use.
cp "${BOOT_ASSETS}/kernel" /mnt/pertisk-boot-a/kernel
cp "${BOOT_ASSETS}/initramfs" /mnt/pertisk-boot-a/initramfs

# Last console= becomes /dev/console for userspace — keep ttyS0 last for Proxmox serial.
# IPv4-only vs dual-stack is decided at runtime (sysctl), not via cmdline.
CMDLINE="${PERTISK_CMDLINE:-console=tty0 console=ttyS0 rdinit=/init}"
cat >/mnt/pertisk-efi/loader/entries/pertisk-a.conf <<EOF
title Pertisk KOS (slot A)
linux /pertisk/A/kernel
initrd /pertisk/A/initramfs
options ${CMDLINE}
EOF
# timeout 0: skip systemd-boot menu countdown. With Proxmox vga=serial0 the
# menu/countdown text is garbled on Serial; auto-boot is correct for cloud.
cat >/mnt/pertisk-efi/loader/loader.conf <<EOF
default pertisk-a.conf
timeout 0
console-mode keep
editor no
EOF

echo "==> STATE seed"
mkdir -p /mnt/pertisk-state/machine \
  /mnt/pertisk-state/secrets \
  /mnt/pertisk-state/log \
  /mnt/pertisk-state/slots
cp "${SEED_CONFIG}" /mnt/pertisk-state/config.yaml
# Optional: override hostname to match Proxmox VM name (PERTISK_HOSTNAME).
if [[ -n "${PERTISK_HOSTNAME:-}" ]]; then
  if command -v sed >/dev/null 2>&1; then
    sed -i "s/^\\([[:space:]]*hostname:[[:space:]]*\\).*/\\1${PERTISK_HOSTNAME}/" \
      /mnt/pertisk-state/config.yaml
    echo "    hostname -> ${PERTISK_HOSTNAME}"
  fi
fi
cat >/mnt/pertisk-state/boot-meta.json <<EOF
{
  "active": "a",
  "next": "a",
  "previous_good": "a",
  "boot_attempts": 0,
  "boot_ok": true,
  "pending_version": null,
  "active_version": "0.1.0"
}
EOF

# Cloud marker for operators.
cat >/mnt/pertisk-state/machine/image.json <<EOF
{
  "format": "pertisk-cloud",
  "arch": "${ARCH}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF

echo "==> done"
df -h /mnt/pertisk-efi /mnt/pertisk-state
