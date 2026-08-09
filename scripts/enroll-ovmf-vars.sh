#!/usr/bin/env bash
# Enroll Pertisk Secure Boot keys into an OVMF/AAVMF vars template (lab / CI).
#
# Requires: virt-fw-vars (pip install virt-firmware | apt install python3-virt-firmware)
#           openssl (via gen-secureboot-keys.sh)
#
#   ./scripts/gen-secureboot-keys.sh          # once
#   ./scripts/enroll-ovmf-vars.sh             # → out/secureboot/OVMF_VARS.secboot.fd
#   ./scripts/enroll-ovmf-vars.sh --arch arm64
#
# Boot with enrolled vars:
#   PERTISK_OVMF_VARS=out/secureboot/OVMF_VARS.secboot.fd ./image/run-qemu-uefi.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KEYS="${ROOT}/out/secureboot"
ARCH="${PERTISK_ARCH:-amd64}"
INPUT="${PERTISK_OVMF_VARS_TEMPLATE:-}"
OUTPUT=""
SECURE_BOOT=1
PRINT_ONLY=0

OWNER_GUID="${PERTISK_SB_OWNER_GUID:-8f3e2a1b-5c4d-4e6f-a7b8-9c0d1e2f3a4b}"

usage() {
  cat <<'EOF'
Usage: enroll-ovmf-vars.sh [--arch amd64|arm64] [--input VARS.fd] [--output VARS.fd]
                           [--no-secure-boot] [--print]

Enrolls out/secureboot/{PK,KEK,db}.crt into a blank OVMF/AAVMF variable store
and enables Secure Boot (unless --no-secure-boot).

Env:
  PERTISK_ARCH                 default amd64
  PERTISK_OVMF_VARS_TEMPLATE   blank vars template (auto-detected if unset)
  PERTISK_SB_OWNER_GUID        owner GUID for PK/KEK/db (lab default set)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch)
      ARCH="$2"
      shift 2
      ;;
    --input)
      INPUT="$2"
      shift 2
      ;;
    --output)
      OUTPUT="$2"
      shift 2
      ;;
    --no-secure-boot)
      SECURE_BOOT=0
      shift
      ;;
    --print)
      PRINT_ONLY=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

case "${ARCH}" in
  amd64 | x86_64) ARCH=amd64 ;;
  arm64 | aarch64) ARCH=arm64 ;;
  *)
    echo "unsupported arch: ${ARCH}" >&2
    exit 1
    ;;
esac

if [[ -z "${OUTPUT}" ]]; then
  if [[ "${ARCH}" == "arm64" ]]; then
    OUTPUT="${KEYS}/AAVMF_VARS.secboot.fd"
  else
    OUTPUT="${KEYS}/OVMF_VARS.secboot.fd"
  fi
fi

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

if ! command -v virt-fw-vars >/dev/null 2>&1; then
  echo "virt-fw-vars not found. Install python3-virt-firmware, e.g.:" >&2
  echo "  pip install virt-firmware" >&2
  echo "  # or: apt install python3-virt-firmware" >&2
  exit 1
fi

if [[ ! -f "${KEYS}/PK.crt" || ! -f "${KEYS}/KEK.crt" || ! -f "${KEYS}/db.crt" ]]; then
  echo "==> generating Secure Boot test keys"
  "${ROOT}/scripts/gen-secureboot-keys.sh"
fi

qemu_share() {
  local p
  for p in \
    "${PERTISK_QEMU_SHARE:-}" \
    /opt/homebrew/share/qemu \
    /usr/local/share/qemu \
    "$(brew --prefix qemu 2>/dev/null)/share/qemu"; do
    [[ -n "${p}" && -d "${p}" ]] && echo "${p}"
  done
}

if [[ -z "${INPUT}" ]]; then
  CANDIDATES=()
  while IFS= read -r share; do
    if [[ "${ARCH}" == "arm64" ]]; then
      CANDIDATES+=("${share}/edk2-arm-vars.fd" "${share}/edk2-aarch64-vars.fd")
    else
      CANDIDATES+=(
        "${share}/edk2-i386-vars.fd"
        "${share}/edk2-x86_64-vars.fd"
        "${share}/OVMF_VARS.fd"
      )
    fi
  done < <(qemu_share)
  if [[ "${ARCH}" == "arm64" ]]; then
    CANDIDATES+=(
      /usr/share/AAVMF/AAVMF_VARS.fd
      /usr/share/edk2/aarch64/vars-template-pflash.raw
    )
  else
    CANDIDATES+=(
      /usr/share/OVMF/OVMF_VARS.fd
      /usr/share/edk2/ovmf/OVMF_VARS.fd
      /usr/share/qemu/OVMF_VARS.fd
    )
  fi
  INPUT="$(find_file "${CANDIDATES[@]}")" || true
fi

if [[ -z "${INPUT}" || ! -f "${INPUT}" ]]; then
  echo "blank OVMF/AAVMF vars template not found for ${ARCH}." >&2
  echo "Install qemu/edk2 or set PERTISK_OVMF_VARS_TEMPLATE=/path/to/VARS.fd" >&2
  exit 1
fi

mkdir -p "$(dirname "${OUTPUT}")"

ARGS=(
  --input "${INPUT}"
  --output "${OUTPUT}"
  --set-pk "${OWNER_GUID}" "${KEYS}/PK.crt"
  --add-kek "${OWNER_GUID}" "${KEYS}/KEK.crt"
  --add-db "${OWNER_GUID}" "${KEYS}/db.crt"
  --microsoft-db none
  --microsoft-kek none
)

if [[ "${SECURE_BOOT}" -eq 1 ]]; then
  ARGS+=(--secure-boot)
fi

echo "==> enrolling PK/KEK/db into ${OUTPUT}"
echo "    template=${INPUT}"
echo "    arch=${ARCH} owner=${OWNER_GUID} secure-boot=${SECURE_BOOT}"
virt-fw-vars "${ARGS[@]}"

if [[ "${PRINT_ONLY}" -eq 1 ]] || [[ "${PERTISK_SB_PRINT:-0}" == "1" ]]; then
  echo "==> varstore summary"
  virt-fw-vars --input "${OUTPUT}" --print 2>/dev/null | head -80 || true
fi

cat <<EOF
==> done

Boot QEMU with enrolled vars (db-signed UKI required when Secure Boot is on):

  PERTISK_SB_KEY=${KEYS}/db.key PERTISK_SB_CERT=${KEYS}/db.crt make uki ARCH=${ARCH}
  PERTISK_OVMF_VARS=${OUTPUT} ./image/run-qemu-uefi.sh

See docs/SECURE_BOOT.md.
EOF
