#!/usr/bin/env bash
# Build initramfs for one or more architectures.
# Usage:
#   ./image/build-initramfs.sh
#   PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
#   PERTISK_VERSION=0.2.0 PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
#   PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
#   PERTISK_IMAGE_PROFILE=debug ./image/build-initramfs.sh   # + BusyBox ash
#   make build PROFILE=production|debug
#   ./image/build-all.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
OVERLAY="${ROOT}/image/runtime-overlay"
BOOT_OVERLAY="${ROOT}/image/boot-overlay"
mkdir -p "${OUT}" "${OVERLAY}" "${BOOT_OVERLAY}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build the initramfs" >&2
  exit 1
fi

if [[ ! -f "${ROOT}/Cargo.lock" ]]; then
  (cd "${ROOT}" && cargo generate-lockfile)
fi

# Version baked into pertiskd via PERTISK_BUILD_VERSION. Default: workspace Cargo.toml.
DEFAULT_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
VERSION="${PERTISK_VERSION:-${DEFAULT_VERSION}}"

find "${OVERLAY}" -mindepth 1 ! -name '.keep' -exec rm -rf {} + 2>/dev/null || true
find "${BOOT_OVERLAY}" -mindepth 1 ! -name '.keep' -exec rm -rf {} + 2>/dev/null || true

if [[ "${PERTISK_EMBED_RUNTIME:-0}" == "1" ]]; then
  if [[ ! -x "${OUT}/runtime/usr/local/bin/containerd" ]]; then
    echo "PERTISK_EMBED_RUNTIME=1 but out/runtime missing; run ./image/fetch-runtime.sh" >&2
    exit 1
  fi
  # containerd/kubelet are glibc-linked; without the loader they never start
  # and the dashboard reports containerd=absent.
  case "${PERTISK_ARCH:-amd64}" in
    amd64|x86_64)
      if [[ ! -e "${OUT}/runtime/lib64/ld-linux-x86-64.so.2" ]]; then
        echo "PERTISK_EMBED_RUNTIME=1 but glibc loader missing; re-run: make fetch-runtime ARCH=amd64" >&2
        exit 1
      fi
      ;;
    arm64|aarch64)
      if [[ ! -e "${OUT}/runtime/lib/ld-linux-aarch64.so.1" ]]; then
        echo "PERTISK_EMBED_RUNTIME=1 but glibc loader missing; re-run: make fetch-runtime ARCH=arm64" >&2
        exit 1
      fi
      ;;
  esac
  echo "==> embedding runtime binaries + glibc into initramfs"
  cp -a "${OUT}/runtime/." "${OVERLAY}/"
fi

# Prefer PERTISK_ARCH (amd64|arm64); fall back to PERTISK_PLATFORM (linux/...).
if [[ -n "${PERTISK_ARCH:-}" ]]; then
  case "${PERTISK_ARCH}" in
    amd64|x86_64) ARCH_SUFFIX=amd64; PLATFORM=linux/amd64 ;;
    arm64|aarch64) ARCH_SUFFIX=arm64; PLATFORM=linux/arm64 ;;
    *) echo "unsupported PERTISK_ARCH=${PERTISK_ARCH}" >&2; exit 1 ;;
  esac
else
  PLATFORM="${PERTISK_PLATFORM:-linux/amd64}"
  case "${PLATFORM}" in
    linux/amd64) ARCH_SUFFIX=amd64 ;;
    linux/arm64) ARCH_SUFFIX=arm64 ;;
    *) echo "unsupported PERTISK_PLATFORM=${PLATFORM}" >&2; exit 1 ;;
  esac
fi

# production (default): no BusyBox. debug: BusyBox ash for recovery.
IMAGE_PROFILE="${PERTISK_IMAGE_PROFILE:-production}"
case "${IMAGE_PROFILE}" in
  production | debug) ;;
  *)
    echo "unsupported PERTISK_IMAGE_PROFILE=${IMAGE_PROFILE} (use production|debug)" >&2
    exit 1
    ;;
esac

if [[ "${PERTISK_EMBED_BOOT:-0}" == "1" ]]; then
  echo "==> staging installer boot assets (kernel + systemd-boot)"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/fetch-kernel.sh"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/fetch-bootloader.sh"
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/stage-boot-assets.sh"
else
  # Still need virtio_net.ko etc. even when the kernel itself is not embedded.
  PERTISK_ARCH="${ARCH_SUFFIX}" "${ROOT}/image/fetch-kernel.sh"
fi

MODULES_SRC="${OUT}/modules-${ARCH_SUFFIX}"
if [[ -d "${MODULES_SRC}" ]]; then
  echo "==> embedding kernel modules from ${MODULES_SRC}"
  mkdir -p "${OVERLAY}/lib/pertisk/modules"
  cp -a "${MODULES_SRC}/." "${OVERLAY}/lib/pertisk/modules/"
else
  echo "WARNING: ${MODULES_SRC} missing — virtio NIC/disk modules unavailable" >&2
fi

ARTIFACT="${OUT}/initramfs-${ARCH_SUFFIX}.cpio.gz"
VERSIONED="${OUT}/initramfs-${ARCH_SUFFIX}-v${VERSION}.cpio.gz"
if [[ "${IMAGE_PROFILE}" == "debug" ]]; then
  ARTIFACT="${OUT}/initramfs-${ARCH_SUFFIX}-debug.cpio.gz"
  VERSIONED="${OUT}/initramfs-${ARCH_SUFFIX}-debug-v${VERSION}.cpio.gz"
fi

echo "==> building initramfs (version=${VERSION} platform=${PLATFORM} profile=${IMAGE_PROFILE})"
# BuildKit required for Dockerfile cache mounts (cargo registry/target).
export DOCKER_BUILDKIT=1
# Prefer docker-driver builder: docker-container instances (multiarch) can
# hit a poisoned registry-1.docker.io A record (TLS SAN *.zerovar.com).
if [[ -z "${BUILDX_BUILDER:-}" ]] && docker buildx inspect desktop-linux >/dev/null 2>&1; then
  export BUILDX_BUILDER=desktop-linux
fi
DOCKER_NET=()
if [[ "$(uname -s)" == Linux ]]; then
  # Self-hosted CI: docker-bridge DNS to Alpine/Debian CDNs often flakes.
  DOCKER_NET+=(--network host)
fi
docker build \
  ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
  --platform "${PLATFORM}" \
  --build-arg "VERSION=${VERSION}" \
  --build-arg "IMAGE_PROFILE=${IMAGE_PROFILE}" \
  -f "${ROOT}/image/Dockerfile.initramfs" \
  --target export \
  -o "type=local,dest=${OUT}/.initramfs-tmp-${ARCH_SUFFIX}" \
  "${ROOT}"

mv "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}/initramfs.cpio.gz" "${ARTIFACT}"
rm -rf "${OUT}/.initramfs-tmp-${ARCH_SUFFIX}"
cp "${ARTIFACT}" "${VERSIONED}"
# Keep legacy name for amd64 QEMU scripts (production only).
if [[ "${ARCH_SUFFIX}" == "amd64" && "${IMAGE_PROFILE}" == "production" ]]; then
  cp "${ARTIFACT}" "${OUT}/initramfs.cpio.gz"
fi

echo "==> wrote ${ARTIFACT}"
echo "==> wrote ${VERSIONED}"
ls -lh "${ARTIFACT}" "${VERSIONED}"
