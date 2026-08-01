#!/usr/bin/env bash
# Boot an installed disk via UEFI — no -kernel/-initrd.
# Prerequisites: qemu + edk2 firmware, disk from install or cloud image build.
#
#   ./image/run-qemu-uefi.sh
#   PERTISK_DISK=out/pertisk-cloud-amd64.raw ./image/run-qemu-uefi.sh
#   PERTISK_DISK=out/pertisk-cloud-arm64.raw ./image/run-qemu-uefi.sh
#   PERTISK_ARCH=arm64 PERTISK_DISK=out/pertisk-cloud-arm64.raw ./image/run-qemu-uefi.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
DISK="${PERTISK_DISK:-${OUT}/pertisk-disk.raw}"
OVMF_CODE="${PERTISK_OVMF_CODE:-}"
OVMF_VARS_SRC="${PERTISK_OVMF_VARS:-}"

# Infer arch from env or disk filename (*-arm64* / *-amd64*).
ARCH="${PERTISK_ARCH:-}"
if [[ -z "${ARCH}" ]]; then
  case "${DISK}" in
    *arm64* | *aarch64*) ARCH=arm64 ;;
    *amd64* | *x86_64*) ARCH=amd64 ;;
    *) ARCH=amd64 ;;
  esac
fi
case "${ARCH}" in
  amd64 | x86_64) ARCH=amd64 ;;
  arm64 | aarch64) ARCH=arm64 ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH} (use amd64 or arm64)" >&2
    exit 1
    ;;
esac

[[ -f "${DISK}" ]] || {
  echo "missing ${DISK}; build with ./image/build-cloud-image.sh or install via ./image/run-qemu-disk.sh" >&2
  exit 1
}

find_file() {
  local f
  for f in "$@"; do
    if [[ -f "${f}" ]]; then
      echo "${f}"
      return 0
    fi
  done
  return 1
}

if [[ "${ARCH}" == "arm64" ]]; then
  QEMU_BIN=qemu-system-aarch64
  if ! command -v "${QEMU_BIN}" >/dev/null 2>&1; then
    echo "${QEMU_BIN} not found. Install with: brew install qemu" >&2
    exit 1
  fi
  if [[ -z "${OVMF_CODE}" ]]; then
    OVMF_CODE="$(find_file \
      /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
      /usr/local/share/qemu/edk2-aarch64-code.fd \
      /usr/share/AAVMF/AAVMF_CODE.fd \
      /usr/share/edk2/aarch64/QEMU_EFI.fd)" || true
  fi
  DEFAULT_VARS_CANDIDATES=(
    /opt/homebrew/share/qemu/edk2-arm-vars.fd
    /usr/local/share/qemu/edk2-arm-vars.fd
    /usr/share/AAVMF/AAVMF_VARS.fd
  )
  VARS_DST="${OUT}/ovmf-vars-arm64.fd"
  MACHINE_ARGS=(-machine virt -cpu max -m 2048M)
  NET_ARGS=(-netdev user,id=net0 -device virtio-net-pci,netdev=net0)
  DISK_ARGS=(-drive file="${DISK}",if=virtio,format=raw)
else
  QEMU_BIN=qemu-system-x86_64
  if ! command -v "${QEMU_BIN}" >/dev/null 2>&1; then
    echo "${QEMU_BIN} not found. Install with: brew install qemu" >&2
    exit 1
  fi
  if [[ -z "${OVMF_CODE}" ]]; then
    OVMF_CODE="$(find_file \
      /opt/homebrew/share/qemu/edk2-x86_64-code.fd \
      /usr/local/share/qemu/edk2-x86_64-code.fd \
      /usr/share/OVMF/OVMF_CODE.fd \
      /usr/share/edk2/ovmf/OVMF_CODE.fd)" || true
  fi
  # Homebrew ships no x86_64-vars; i386-vars (~528KiB) keeps CODE+VARS under q35's 8MiB cap.
  DEFAULT_VARS_CANDIDATES=(
    /opt/homebrew/share/qemu/edk2-i386-vars.fd
    /usr/local/share/qemu/edk2-i386-vars.fd
    /opt/homebrew/share/qemu/edk2-x86_64-vars.fd
    /usr/local/share/qemu/edk2-x86_64-vars.fd
    /usr/share/OVMF/OVMF_VARS.fd
  )
  VARS_DST="${OUT}/ovmf-vars-amd64.fd"
  MACHINE_ARGS=(-machine q35 -cpu max -m 1024M)
  NET_ARGS=(-netdev user,id=net0 -device virtio-net-pci,netdev=net0)
  DISK_ARGS=(-drive file="${DISK}",if=virtio,format=raw)
fi

if [[ -z "${OVMF_CODE}" || ! -f "${OVMF_CODE}" ]]; then
  echo "UEFI firmware not found for ${ARCH}. Install qemu (edk2) or set PERTISK_OVMF_CODE" >&2
  exit 1
fi

# Migrate legacy shared vars path; drop oversized x86 stub (64MiB broke q35).
if [[ "${ARCH}" == "amd64" && -f "${OUT}/ovmf-vars.fd" && ! -f "${VARS_DST}" ]]; then
  legacy_size="$(wc -c <"${OUT}/ovmf-vars.fd" | tr -d ' ')"
  if [[ "${legacy_size}" -le 4194304 ]]; then
    mv "${OUT}/ovmf-vars.fd" "${VARS_DST}"
  else
    echo "==> removing oversized legacy ${OUT}/ovmf-vars.fd (${legacy_size} bytes)"
    rm -f "${OUT}/ovmf-vars.fd"
  fi
fi

if [[ -n "${OVMF_VARS_SRC}" && -f "${OVMF_VARS_SRC}" ]]; then
  cp "${OVMF_VARS_SRC}" "${VARS_DST}"
elif [[ ! -f "${VARS_DST}" ]]; then
  vars_src="$(find_file "${DEFAULT_VARS_CANDIDATES[@]}")" || true
  if [[ -n "${vars_src}" ]]; then
    cp "${vars_src}" "${VARS_DST}"
  elif [[ "${ARCH}" == "arm64" ]]; then
    # Match Homebrew aarch64 CODE size (64MiB).
    dd if=/dev/zero of="${VARS_DST}" bs=1m count=64 status=none
  else
    # Keep CODE (~3.5MiB) + VARS under q35's 8MiB firmware budget.
    dd if=/dev/zero of="${VARS_DST}" bs=1k count=528 status=none
  fi
fi

echo "==> UEFI boot (${ARCH}) from ${DISK} via ${QEMU_BIN} (Ctrl-A X to exit)"
echo "    code=${OVMF_CODE}"
echo "    vars=${VARS_DST}"
exec "${QEMU_BIN}" \
  "${MACHINE_ARGS[@]}" \
  -nographic \
  -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
  -drive if=pflash,format=raw,file="${VARS_DST}" \
  "${NET_ARGS[@]}" \
  "${DISK_ARGS[@]}"
