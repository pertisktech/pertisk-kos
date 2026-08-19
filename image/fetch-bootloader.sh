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
    SRC_GLOB='systemd-bootx64.efi'
    DEB_ARCH=amd64
    ;;
  arm64)
    PLATFORM=linux/arm64
    EFI_NAME=BOOTAA64.EFI
    SRC_GLOB='systemd-bootaa64.efi'
    DEB_ARCH=arm64
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

DOCKER_NET=()
if [[ "$(uname -s)" == Linux ]]; then
  # Self-hosted CI: docker-bridge DNS to deb.debian.org often flakes.
  DOCKER_NET+=(--network host)
fi

echo "==> extracting systemd-boot EFI via Debian (${ARCH})"
docker run --rm --platform "${PLATFORM}" \
  ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
  -v "${OUT}:/out" \
  -v "${ROOT}/image/apt-retry.sh:/apt-retry.sh:ro" \
  -e "SRC_GLOB=${SRC_GLOB}" \
  -e "EFI_NAME=${EFI_NAME}" \
  debian:bookworm-slim bash -c '
  set -euo pipefail
  sh /apt-retry.sh systemd-boot-efi
  src=$(find /usr -name "${SRC_GLOB}" | head -1)
  if [ -z "${src}" ]; then
    echo "systemd-boot EFI not found" >&2
    find /usr -name "*boot*.efi" 2>/dev/null || true
    exit 1
  fi
  cp "${src}" "/out/${EFI_NAME}"
  cp "${src}" "/out/${SRC_GLOB}"
  echo "copied ${src}"
'

ls -lh "${OUT}/${EFI_NAME}"
echo "==> wrote ${OUT}/${EFI_NAME}"
