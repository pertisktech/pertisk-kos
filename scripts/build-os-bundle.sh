#!/usr/bin/env bash
# Stage + sign an A/B OS upgrade bundle (kernel, initramfs, manifest.json, manifest.sig).
# Kubernetes is not changed. Recreating VMs from a new qcow2 is a reinstall, not this path.
#
#   make os-trust
#   make os-bundle VERSION=0.2.86 ARCH=amd64
#   make os-bundle SKIP_BUILD=1   # re-sign existing out/ kernel + initramfs
#
# Trust: OS_TRUST_SK / OS_TRUST_PK (default out/secrets/os-trust.{sk,pk}).
# The matching public key must already be on each node: STATE/secrets/os-trust.pk
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
ARCH="${PERTISK_ARCH:-amd64}"
VERSION="${PERTISK_VERSION:-}"
PROFILE="${PERTISK_IMAGE_PROFILE:-production}"
SKIP_BUILD="${SKIP_BUILD:-0}"
OS_TRUST_SK="${OS_TRUST_SK:-${OUT}/secrets/os-trust.sk}"
OS_TRUST_PK="${OS_TRUST_PK:-${OUT}/secrets/os-trust.pk}"

if [[ -z "${VERSION}" ]]; then
  VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
fi

case "${ARCH}" in
  amd64|x86_64) ARCH=amd64; KERNEL_SRC="${OUT}/bzImage" ;;
  arm64|aarch64) ARCH=arm64; KERNEL_SRC="${OUT}/vmlinuz-arm64" ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}; use amd64 or arm64" >&2
    exit 1
    ;;
esac

if [[ "${PROFILE}" == "debug" ]]; then
  INITRD_SRC="${OUT}/initramfs-${ARCH}-debug.cpio.gz"
else
  INITRD_SRC="${OUT}/initramfs-${ARCH}.cpio.gz"
  if [[ "${ARCH}" == "amd64" && ! -f "${INITRD_SRC}" ]]; then
    INITRD_SRC="${OUT}/initramfs.cpio.gz"
  fi
fi

DEST="${OUT}/os-bundle-${ARCH}-v${VERSION}"
ZIP="${OUT}/os-bundle-${ARCH}-v${VERSION}.zip"

if [[ ! -f "${OS_TRUST_SK}" ]]; then
  echo "missing signing key ${OS_TRUST_SK}" >&2
  echo "generate once (keep .sk offline; copy .pk to STATE/secrets/os-trust.pk):" >&2
  echo "  make os-trust" >&2
  exit 1
fi
if [[ ! -f "${OS_TRUST_PK}" ]]; then
  echo "missing public key ${OS_TRUST_PK} (make os-trust)" >&2
  exit 1
fi

if [[ ! -f "${KERNEL_SRC}" ]]; then
  echo "missing ${KERNEL_SRC}; run make os-bundle (without SKIP_BUILD=1) or make fetch-kernel ARCH=${ARCH}" >&2
  exit 1
fi
if [[ ! -f "${INITRD_SRC}" ]]; then
  echo "missing ${INITRD_SRC}; run make os-bundle (without SKIP_BUILD=1)" >&2
  exit 1
fi

SIGN="${ROOT}/target/release/pertisk-sign"
if [[ ! -x "${SIGN}" ]]; then
  echo "==> build pertisk-sign"
  (cd "${ROOT}" && PERTISK_BUILD_VERSION="${VERSION}" cargo build --release -p pertisk-update --bin pertisk-sign)
fi

rm -rf "${DEST}"
mkdir -p "${DEST}"
cp "${KERNEL_SRC}" "${DEST}/kernel"
cp "${INITRD_SRC}" "${DEST}/initramfs"
cp "${OS_TRUST_PK}" "${DEST}/os-trust.pk"

echo "==> sign OS bundle VERSION=${VERSION} ARCH=${ARCH} → ${DEST}"
"${SIGN}" sign --bundle "${DEST}" --version "${VERSION}" --secret "${OS_TRUST_SK}"
"${SIGN}" verify --bundle "${DEST}" --public "${OS_TRUST_PK}"

write_zip() {
  local zip_path="$1"
  rm -f "${zip_path}"
  if command -v zip >/dev/null 2>&1; then
    (cd "${DEST}" && zip -q "${zip_path}" kernel initramfs manifest.json manifest.sig os-trust.pk)
    return
  fi
  python3 - "${DEST}" "${zip_path}" <<'PY'
import sys, zipfile
from pathlib import Path
src, dest = Path(sys.argv[1]), Path(sys.argv[2])
names = ("kernel", "initramfs", "manifest.json", "manifest.sig", "os-trust.pk")
with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_DEFLATED) as z:
    for name in names:
        p = src / name
        if p.is_file():
            z.write(p, name)
PY
}

write_zip "${ZIP}"

echo "==> OS A/B upgrade bundle (Kubernetes is not changed)"
echo "    zip includes os-trust.pk; the upgrade job installs it on STATE if missing"
echo "    keep ${OS_TRUST_SK} offline; recreating VMs from a new qcow2 is a reinstall"
ls -lh "${DEST}"/{kernel,initramfs,manifest.json,manifest.sig,os-trust.pk} "${ZIP}"
echo "==> upload ${ZIP}"
echo "    or the files in ${DEST}"
echo "    to Cluster → Upgrade → OS A/B upgrade"
