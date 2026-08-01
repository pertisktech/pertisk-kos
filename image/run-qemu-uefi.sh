#!/usr/bin/env bash
# Boot an installed disk via OVMF (UEFI) — no -kernel/-initrd.
# Prerequisites: qemu, OVMF, out/pertisk-disk.raw (after install smoke).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
DISK="${OUT}/pertisk-disk.raw"
OVMF_CODE="${PERTISK_OVMF_CODE:-}"
OVMF_VARS_SRC="${PERTISK_OVMF_VARS:-}"

if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
  echo "qemu-system-x86_64 not found. Install with: brew install qemu" >&2
  exit 1
fi

[[ -f "${DISK}" ]] || { echo "missing ${DISK}; run install via ./image/run-qemu-disk.sh first" >&2; exit 1; }

# Locate OVMF firmware (Homebrew qemu / Linux packages).
if [[ -z "${OVMF_CODE}" ]]; then
  for candidate in \
    /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
    /usr/local/share/qemu/edk2-x86_64-code.fd \
    /usr/share/OVMF/OVMF_CODE.fd \
    /usr/share/edk2/ovmf/OVMF_CODE.fd
  do
    if [[ -f "${candidate}" ]]; then
      OVMF_CODE="${candidate}"
      break
    fi
  done
fi

if [[ -z "${OVMF_CODE}" || ! -f "${OVMF_CODE}" ]]; then
  echo "OVMF code not found. Install qemu (includes edk2) or set PERTISK_OVMF_CODE" >&2
  exit 1
fi

VARS_DST="${OUT}/ovmf-vars.fd"
if [[ -n "${OVMF_VARS_SRC}" && -f "${OVMF_VARS_SRC}" ]]; then
  cp "${OVMF_VARS_SRC}" "${VARS_DST}"
elif [[ ! -f "${VARS_DST}" ]]; then
  # Writable vars store; 64M matches common edk2 vars templates.
  for candidate in \
    /opt/homebrew/share/qemu/edk2-x86_64-vars.fd \
    /usr/local/share/qemu/edk2-x86_64-vars.fd \
    /usr/share/OVMF/OVMF_VARS.fd
  do
    if [[ -f "${candidate}" ]]; then
      cp "${candidate}" "${VARS_DST}"
      break
    fi
  done
  if [[ ! -f "${VARS_DST}" ]]; then
    dd if=/dev/zero of="${VARS_DST}" bs=1m count=64 status=none
  fi
fi

echo "==> UEFI boot from ${DISK} (Ctrl-A X to exit)"
exec qemu-system-x86_64 \
  -machine q35 \
  -cpu max \
  -m 1024M \
  -nographic \
  -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
  -drive if=pflash,format=raw,file="${VARS_DST}" \
  -netdev user,id=net0 \
  -device virtio-net-pci,netdev=net0 \
  -drive file="${DISK}",if=virtio,format=raw
