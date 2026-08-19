#!/usr/bin/env bash
# Download containerd + kubelet (linux/amd64|arm64) into out/runtime/.
# Official containerd/kubelet binaries are dynamically linked against glibc; the
# Pertisk rootfs is musl-based, so we also vendor the glibc loader + shared libs.
#
# Versions (override any pin; use `latest` to resolve at fetch time):
#   K8S_VER=v1.36.3|latest          (default: latest → dl.k8s.io/release/stable.txt)
#   CONTAINERD_VER=2.0.5|latest     (default: latest GitHub release, strip leading v)
#   RUNC_VER=v1.2.6|latest
#   CNI_VER=v1.6.2|latest
#   PERTISK_RUNTIME_LATEST=1        force all components to latest
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/runtime"
mkdir -p "${OUT}/usr/local/bin" "${OUT}/opt/cni/bin"

ARCH="${PERTISK_ARCH:-amd64}"

case "${ARCH}" in
  amd64|x86_64) ARCH=amd64; PLATFORM=linux/amd64 ;;
  arm64|aarch64) ARCH=arm64; PLATFORM=linux/arm64 ;;
  *) echo "unsupported PERTISK_ARCH=${ARCH}" >&2; exit 1 ;;
esac

# Resolve "latest" / empty via public APIs. Fail closed if lookup fails.
gh_latest_tag() {
  local repo="$1"
  curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -1
}

resolve_k8s() {
  local v="${1:-}"
  if [[ -z "${v}" || "${v}" == "latest" ]]; then
    v="$(curl -fsSL https://dl.k8s.io/release/stable.txt | tr -d '[:space:]')"
  fi
  case "${v}" in
    v*) echo "${v}" ;;
    *) echo "v${v}" ;;
  esac
}

resolve_containerd() {
  local v="${1:-}"
  if [[ -z "${v}" || "${v}" == "latest" ]]; then
    v="$(gh_latest_tag containerd/containerd)"
  fi
  # Tarball name uses bare semver (no leading v).
  echo "${v#v}"
}

resolve_tag() {
  local repo="$1"
  local v="${2:-}"
  if [[ -z "${v}" || "${v}" == "latest" ]]; then
    v="$(gh_latest_tag "${repo}")"
  fi
  case "${v}" in
    v*) echo "${v}" ;;
    *) echo "v${v}" ;;
  esac
}

if [[ "${PERTISK_RUNTIME_LATEST:-0}" == "1" ]]; then
  K8S_VER=latest
  CONTAINERD_VER=latest
  RUNC_VER=latest
  CNI_VER=latest
fi

# Defaults: latest stable (override with explicit pins for reproducible builds).
K8S_VER="$(resolve_k8s "${K8S_VER:-latest}")"
CONTAINERD_VER="$(resolve_containerd "${CONTAINERD_VER:-latest}")"
RUNC_VER="$(resolve_tag opencontainers/runc "${RUNC_VER:-latest}")"
CNI_VER="$(resolve_tag containernetworking/plugins "${CNI_VER:-latest}")"

if [[ -z "${K8S_VER}" || -z "${CONTAINERD_VER}" || -z "${RUNC_VER}" || -z "${CNI_VER}" ]]; then
  echo "failed to resolve one or more runtime versions" >&2
  echo "  K8S_VER=${K8S_VER:-?} CONTAINERD_VER=${CONTAINERD_VER:-?} RUNC_VER=${RUNC_VER:-?} CNI_VER=${CNI_VER:-?}" >&2
  exit 1
fi

echo "==> versions: kubelet=${K8S_VER} containerd=${CONTAINERD_VER} runc=${RUNC_VER} cni=${CNI_VER} arch=${ARCH}"
{
  echo "K8S_VER=${K8S_VER}"
  echo "CONTAINERD_VER=${CONTAINERD_VER}"
  echo "RUNC_VER=${RUNC_VER}"
  echo "CNI_VER=${CNI_VER}"
  echo "ARCH=${ARCH}"
  echo "FETCHED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "${OUT}/versions.txt"

echo "==> containerd ${CONTAINERD_VER} (${ARCH})"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

curl -fsSL \
  "https://github.com/containerd/containerd/releases/download/v${CONTAINERD_VER}/containerd-${CONTAINERD_VER}-linux-${ARCH}.tar.gz" \
  -o "${TMP}/containerd.tgz"
tar -xzf "${TMP}/containerd.tgz" -C "${TMP}"
cp "${TMP}/bin/containerd" "${TMP}/bin/containerd-shim-runc-v2" "${TMP}/bin/ctr" "${OUT}/usr/local/bin/"
chmod +x "${OUT}/usr/local/bin/"*

echo "==> kubelet ${K8S_VER}"
curl -fsSL \
  "https://dl.k8s.io/release/${K8S_VER}/bin/linux/${ARCH}/kubelet" \
  -o "${OUT}/usr/local/bin/kubelet"
chmod +x "${OUT}/usr/local/bin/kubelet"

echo "==> runc ${RUNC_VER}"
curl -fsSL \
  "https://github.com/opencontainers/runc/releases/download/${RUNC_VER}/runc.${ARCH}" \
  -o "${OUT}/usr/local/bin/runc"
chmod +x "${OUT}/usr/local/bin/runc"

echo "==> CNI plugins ${CNI_VER} (loopback, bridge, host-local, portmap)"
curl -fsSL \
  "https://github.com/containernetworking/plugins/releases/download/${CNI_VER}/cni-plugins-linux-${ARCH}-${CNI_VER}.tgz" \
  -o "${TMP}/cni.tgz"
tar -xzf "${TMP}/cni.tgz" -C "${OUT}/opt/cni/bin" \
  ./loopback ./bridge ./host-local ./portmap
chmod +x "${OUT}/opt/cni/bin/"*

echo "==> vendoring glibc loader + libs for containerd/kubelet (${PLATFORM})"
if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to vendor glibc for runtime binaries" >&2
  exit 1
fi
DOCKER_NET=()
if [[ "$(uname -s)" == Linux ]]; then
  DOCKER_NET+=(--network host)
fi
# Always run amd64 container. For arm64, cross-install libc6:arm64 and copy the
# foreign-arch libs without ever executing aarch64 code (QEMU binfmt unavailable).
docker run --rm \
  ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
  -v "${OUT}:/out" \
  -v "${ROOT}/image/apt-retry.sh:/apt-retry.sh:ro" \
  -e "TARGET_ARCH=${ARCH}" \
  debian:bookworm-slim \
  bash -c '
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
TARGET_ARCH="${TARGET_ARCH}"
if [ "${TARGET_ARCH}" = "amd64" ]; then
  sh /apt-retry.sh libc6
else
  dpkg --add-architecture arm64
  sh /apt-retry.sh libc6:arm64
fi
mkdir -p /out/lib64 /out/lib/x86_64-linux-gnu /out/lib/aarch64-linux-gnu /out/lib

if [ "${TARGET_ARCH}" = "amd64" ]; then
  # Native: use ldd to find deps.
  copy_deps() {
    local bin="$1"
    if ! ldd "$bin" >/tmp/ldd.out 2>/dev/null; then
      return 0
    fi
    while read -r line; do
      lib=""
      if [[ "$line" == *" => /"* ]]; then
        lib="${line#* => }"
        lib="${lib%% *}"
      elif [[ "$line" == /* ]]; then
        lib="${line%% *}"
      fi
      [[ -n "$lib" && -e "$lib" ]] || continue
      case "$lib" in
        /lib64/*)
          mkdir -p /out/lib64
          cp -aL "$lib" "/out/lib64/$(basename "$lib")"
          ;;
        /lib/x86_64-linux-gnu/*|/usr/lib/x86_64-linux-gnu/*)
          mkdir -p /out/lib/x86_64-linux-gnu
          cp -aL "$lib" "/out/lib/x86_64-linux-gnu/$(basename "$lib")"
          ;;
        /lib/*)
          mkdir -p /out/lib
          cp -aL "$lib" "/out/lib/$(basename "$lib")"
          ;;
      esac
    done < /tmp/ldd.out
  }
  for b in containerd kubelet ctr containerd-shim-runc-v2; do
    copy_deps "/out/usr/local/bin/$b"
  done
  if [[ -e /lib64/ld-linux-x86-64.so.2 ]]; then
    mkdir -p /out/lib64
    cp -aL /lib64/ld-linux-x86-64.so.2 /out/lib64/
  fi
else
  # arm64: cannot ldd the foreign binaries. Copy all glibc libs from the
  # cross-installed libc6:arm64 package.
  for lib in /lib/aarch64-linux-gnu/lib*.so* /usr/lib/aarch64-linux-gnu/lib*.so*; do
    [ -e "$lib" ] || continue
    mkdir -p /out/lib/aarch64-linux-gnu
    cp -aL "$lib" "/out/lib/aarch64-linux-gnu/$(basename "$lib")" 2>/dev/null || true
  done
  if [ -e /lib/ld-linux-aarch64.so.1 ]; then
    mkdir -p /out/lib
    cp -aL /lib/ld-linux-aarch64.so.1 /out/lib/
  fi
fi
'

# Fail loudly if glibc loader is missing — without it, containerd shows as
# "absent" on the node (dynamic linker can't exec the binary).
case "${ARCH}" in
  amd64)
    if [[ ! -e "${OUT}/lib64/ld-linux-x86-64.so.2" ]]; then
      echo "ERROR: missing ${OUT}/lib64/ld-linux-x86-64.so.2 after glibc vendor" >&2
      exit 1
    fi
    if [[ ! -e "${OUT}/lib/x86_64-linux-gnu/libc.so.6" ]]; then
      echo "ERROR: missing glibc libc.so.6 under ${OUT}/lib/x86_64-linux-gnu/" >&2
      exit 1
    fi
    ;;
  arm64)
    if [[ ! -e "${OUT}/lib/ld-linux-aarch64.so.1" ]]; then
      echo "ERROR: missing ${OUT}/lib/ld-linux-aarch64.so.1 after glibc vendor" >&2
      exit 1
    fi
    if [[ ! -e "${OUT}/lib/aarch64-linux-gnu/libc.so.6" ]]; then
      echo "ERROR: missing glibc libc.so.6 under ${OUT}/lib/aarch64-linux-gnu/" >&2
      exit 1
    fi
    ;;
esac

echo "==> runtime tree ready at ${OUT}"
cat "${OUT}/versions.txt"
find "${OUT}" -type f | sort
ls -lh "${OUT}/usr/local/bin/"
du -sh "${OUT}"
