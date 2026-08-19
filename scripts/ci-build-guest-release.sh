#!/usr/bin/env bash
# Build guest cloud qcow2 (+ optional signed OS A/B zips) for GitHub Release.
#
#   VERSION=0.3.6 ./scripts/ci-build-guest-release.sh
#   GUEST_ARCHES=amd64,arm64 VERSION=0.3.6 ./scripts/ci-build-guest-release.sh
#
# Copies into out/pkg/:
#   pertisk-cloud-{arch}-v{VERSION}.qcow2
#   os-bundle-{arch}-v{VERSION}.zip   (when out/secrets/.os-bundle-ready exists)
#   os-trust.pk                       (when signing)
#
# Needs Docker. Arm64 on an amd64 runner needs qemu-user / binfmt
# (CI: docker/setup-qemu-action). Privileged containers for disk populate.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)}"
ARCHES="${GUEST_ARCHES:-amd64,arm64}"
PKG="${ROOT}/out/pkg"
STAMP="${ROOT}/out/secrets/.os-bundle-ready"
SIGN=0
[[ -f "$STAMP" ]] && SIGN=1

mkdir -p "$PKG"

chown_out() {
  "${ROOT}/scripts/ci-chown-path.sh" "${ROOT}/out" || true
}
trap chown_out EXIT

preflight_platform() {
  local plat="$1"
  echo "==> docker platform check ${plat}"
  docker run --rm --platform "$plat" alpine:3.20 uname -m
}

stage_qcow() {
  local arch="$1"
  local src="${ROOT}/out/pertisk-cloud-${arch}.qcow2"
  local dest="${PKG}/pertisk-cloud-${arch}-v${VERSION}.qcow2"
  [[ -f "$src" ]] || {
    echo "missing ${src}" >&2
    exit 1
  }
  local bytes
  bytes="$(wc -c <"$src" | tr -d ' ')"
  # GitHub Release assets max 2 GiB.
  if [[ "$bytes" -gt $((1536 * 1024 * 1024)) ]]; then
    echo "::warning::${src} is large (${bytes} bytes); GitHub assets must stay under 2 GiB"
  fi
  if [[ "$bytes" -ge $((2 * 1024 * 1024 * 1024)) ]]; then
    echo "::error::${src} exceeds GitHub Release 2 GiB limit" >&2
    exit 1
  fi
  cp -f "$src" "$dest"
  ls -lh "$dest"
}

IFS=',' read -r -a LIST <<<"$ARCHES"
for raw in "${LIST[@]}"; do
  arch="$(echo "$raw" | xargs)"
  [[ -n "$arch" ]] || continue
  case "$arch" in
    amd64 | arm64) ;;
    *)
      echo "unsupported GUEST_ARCHES entry: ${arch}" >&2
      exit 1
      ;;
  esac

  if [[ "$arch" == "arm64" ]]; then
    preflight_platform linux/arm64
  fi

  echo "==> guest cloud VERSION=${VERSION} ARCH=${arch}"
  make -C "$ROOT" cloud VERSION="$VERSION" ARCH="$arch"
  stage_qcow "$arch"

  if [[ "$SIGN" == "1" ]]; then
    echo "==> OS bundle VERSION=${VERSION} ARCH=${arch} (reuse initramfs)"
    make -C "$ROOT" os-bundle VERSION="$VERSION" ARCH="$arch" SKIP_BUILD=1
    zip="${ROOT}/out/os-bundle-${arch}-v${VERSION}.zip"
    [[ -f "$zip" ]] || {
      echo "missing ${zip}" >&2
      exit 1
    }
    cp -f "$zip" "${PKG}/os-bundle-${arch}-v${VERSION}.zip"
    ls -lh "${PKG}/os-bundle-${arch}-v${VERSION}.zip"
  fi
done

if [[ "$SIGN" == "1" ]]; then
  cp -f "${ROOT}/out/secrets/os-trust.pk" "${PKG}/os-trust.pk"
fi

echo "==> guest release artifacts in ${PKG}"
ls -lh "${PKG}"/pertisk-cloud-*-v"${VERSION}".qcow2 \
  "${PKG}"/os-bundle-*-v"${VERSION}".zip \
  "${PKG}"/os-trust.pk 2>/dev/null || ls -lh "${PKG}"/pertisk-cloud-*-v"${VERSION}".qcow2
