#!/usr/bin/env bash
# Fetch systemd-boot EFI binary into out/bootloader/.
# Usage:
#   ./image/fetch-bootloader.sh
#   PERTISK_ARCH=arm64 ./image/fetch-bootloader.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/bootloader"
mkdir -p "${OUT}"

ARCH="${PERTISK_ARCH:-amd64}"
case "${ARCH}" in
  amd64)
    PLATFORM=linux/amd64
    EFI_NAME=BOOTX64.EFI
    SRC_NAME=systemd-bootx64.efi
    ;;
  arm64)
    PLATFORM=linux/arm64
    EFI_NAME=BOOTAA64.EFI
    SRC_NAME=systemd-bootaa64.efi
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

if [[ -f "${OUT}/${EFI_NAME}" ]]; then
  echo "==> bootloader already present: ${OUT}/${EFI_NAME}"
  ls -lh "${OUT}/${EFI_NAME}"
  exit 0
fi

echo "==> extracting systemd-boot via alpine (${ARCH})"
docker run --rm --platform "${PLATFORM}" -v "${OUT}:/out" alpine:3.20 sh -c "
  set -e
  apk add --no-cache systemd-boot >/dev/null
  src=\$(find /usr -name '${SRC_NAME}' | head -1)
  if [ -z \"\$src\" ]; then
    echo 'systemd-boot EFI not found in alpine package' >&2
    exit 1
  fi
  cp \"\$src\" /out/${EFI_NAME}
  cp \"\$src\" /out/${SRC_NAME}
  echo \"copied \$src\"
"

ls -lh "${OUT}/${EFI_NAME}"
echo "==> wrote ${OUT}/${EFI_NAME}"
