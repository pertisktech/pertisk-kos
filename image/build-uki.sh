#!/usr/bin/env bash
# Build a Unified Kernel Image (UKI) via systemd-ukify in Docker.
#
#   ./image/fetch-kernel.sh
#   ./image/build-initramfs.sh
#   ./image/build-uki.sh                  # → out/uki/pertisk-amd64.efi
#   PERTISK_ARCH=arm64 ./image/build-uki.sh
#
# Optional Secure Boot signing:
#   ./scripts/gen-secureboot-keys.sh
#   PERTISK_SB_KEY=out/secureboot/db.key PERTISK_SB_CERT=out/secureboot/db.crt \
#     ./image/build-uki.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
UKI_OUT="${OUT}/uki"
ARCH="${PERTISK_ARCH:-amd64}"
CMDLINE="${PERTISK_CMDLINE:-console=tty0 console=ttyS0 rdinit=/init}"
VERSION="${PERTISK_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)}"

mkdir -p "${UKI_OUT}"

case "${ARCH}" in
  amd64 | x86_64)
    ARCH=amd64
    PLATFORM=linux/amd64
    KERNEL="${PERTISK_KERNEL:-${OUT}/bzImage}"
    INITRD="${PERTISK_INITRAMFS:-${OUT}/initramfs-amd64.cpio.gz}"
    [[ -f "${INITRD}" ]] || INITRD="${OUT}/initramfs.cpio.gz"
    STUB_NAME=linuxx64.efi.stub
    ;;
  arm64 | aarch64)
    ARCH=arm64
    PLATFORM=linux/arm64
    KERNEL="${PERTISK_KERNEL:-${OUT}/vmlinuz-arm64}"
    INITRD="${PERTISK_INITRAMFS:-${OUT}/initramfs-arm64.cpio.gz}"
    STUB_NAME=linuxaa64.efi.stub
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

[[ -f "${KERNEL}" ]] || {
  echo "missing kernel ${KERNEL}; run ./image/fetch-kernel.sh" >&2
  exit 1
}
[[ -f "${INITRD}" ]] || {
  echo "missing initramfs ${INITRD}; run ./image/build-initramfs.sh" >&2
  exit 1
}

OUTPUT="${UKI_OUT}/pertisk-${ARCH}.efi"
DOCKER_VOLS=(
  -v "${KERNEL}:/work/kernel:ro"
  -v "${INITRD}:/work/initrd:ro"
  -v "${UKI_OUT}:/out"
)

SIGN_FLAG=0
if [[ -n "${PERTISK_SB_KEY:-}" && -n "${PERTISK_SB_CERT:-}" ]]; then
  [[ -f "${PERTISK_SB_KEY}" && -f "${PERTISK_SB_CERT}" ]] || {
    echo "Secure Boot key/cert not found" >&2
    exit 1
  }
  DOCKER_VOLS+=(-v "${PERTISK_SB_KEY}:/keys/db.key:ro" -v "${PERTISK_SB_CERT}:/keys/db.crt:ro")
  SIGN_FLAG=1
  echo "==> Secure Boot signing enabled"
fi

echo "==> building UKI (${ARCH}) version=${VERSION}"
echo "    kernel=${KERNEL}"
echo "    initrd=${INITRD}"
echo "    cmdline=${CMDLINE}"

docker run --rm --platform "${PLATFORM}" \
  "${DOCKER_VOLS[@]}" \
  -e "STUB_NAME=${STUB_NAME}" \
  -e "CMDLINE=${CMDLINE}" \
  -e "VERSION=${VERSION}" \
  -e "ARCH=${ARCH}" \
  -e "SIGN_FLAG=${SIGN_FLAG}" \
  ubuntu:24.04 \
  bash -c '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq systemd-ukify systemd-boot-efi binutils sbsigntool >/dev/null
    STUB="/usr/lib/systemd/boot/efi/${STUB_NAME}"
    test -f "${STUB}"
    UKIFY_ARGS=(
      build
      --stub="${STUB}"
      --linux=/work/kernel
      --initrd=/work/initrd
      --cmdline="${CMDLINE}"
      --os-release="PRETTY_NAME=pertisk-kos ${VERSION}
NAME=pertisk-kos
VERSION=${VERSION}
VERSION_ID=${VERSION}
ID=pertisk-kos
"
      --uname="pertisk-${VERSION}"
      --output="/out/pertisk-${ARCH}.efi"
    )
    if [[ "${SIGN_FLAG}" == "1" ]]; then
      UKIFY_ARGS+=(
        --secureboot-private-key=/keys/db.key
        --secureboot-certificate=/keys/db.crt
      )
    fi
    ukify "${UKIFY_ARGS[@]}"
    ls -lh "/out/pertisk-${ARCH}.efi"
  '

cp "${OUTPUT}" "${UKI_OUT}/pertisk-${ARCH}-v${VERSION}.efi"
echo "==> wrote ${OUTPUT}"
ls -lh "${OUTPUT}" "${UKI_OUT}/pertisk-${ARCH}-v${VERSION}.efi"
