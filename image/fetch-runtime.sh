#!/usr/bin/env bash
# Download pinned containerd + kubelet (linux/amd64|arm64) into out/runtime/.
# Official containerd/kubelet binaries are dynamically linked against glibc; the
# Pertisk rootfs is musl-based, so we also vendor the glibc loader + shared libs.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/runtime"
mkdir -p "${OUT}/usr/local/bin" "${OUT}/opt/cni/bin"

CONTAINERD_VER="${CONTAINERD_VER:-2.0.5}"
K8S_VER="${K8S_VER:-v1.32.5}"
ARCH="${PERTISK_ARCH:-amd64}"

case "${ARCH}" in
  amd64|x86_64) ARCH=amd64; PLATFORM=linux/amd64 ;;
  arm64|aarch64) ARCH=arm64; PLATFORM=linux/arm64 ;;
  *) echo "unsupported PERTISK_ARCH=${ARCH}" >&2; exit 1 ;;
esac

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

echo "==> runc"
RUNC_VER="${RUNC_VER:-v1.2.6}"
curl -fsSL \
  "https://github.com/opencontainers/runc/releases/download/${RUNC_VER}/runc.${ARCH}" \
  -o "${OUT}/usr/local/bin/runc"
chmod +x "${OUT}/usr/local/bin/runc"

echo "==> CNI plugins (loopback, bridge, host-local, portmap)"
CNI_VER="${CNI_VER:-v1.6.2}"
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
docker run --rm --platform "${PLATFORM}" \
  -v "${OUT}:/out" \
  debian:bookworm-slim \
  bash -c '
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq libc6 >/dev/null
mkdir -p /out/lib64 /out/lib/x86_64-linux-gnu /out/lib/aarch64-linux-gnu

copy_deps() {
  local bin="$1"
  # Static binaries: ldd exits non-zero / says "not a dynamic executable".
  if ! ldd "$bin" >/tmp/ldd.out 2>/dev/null; then
    return 0
  fi
  while read -r line; do
    # "libfoo.so.1 => /lib/.../libfoo.so.1 (0x...)" or "/lib64/ld-linux-... (0x...)"
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
      /lib/aarch64-linux-gnu/*|/usr/lib/aarch64-linux-gnu/*)
        mkdir -p /out/lib/aarch64-linux-gnu
        cp -aL "$lib" "/out/lib/aarch64-linux-gnu/$(basename "$lib")"
        ;;
      /lib/*)
        # e.g. /lib/ld-linux-aarch64.so.1
        mkdir -p /out/lib
        cp -aL "$lib" "/out/lib/$(basename "$lib")"
        ;;
    esac
  done < /tmp/ldd.out
}

for b in containerd kubelet ctr containerd-shim-runc-v2; do
  copy_deps "/out/usr/local/bin/$b"
done
# Ensure interpreter path exists even if ldd formatting differs.
if [[ -e /lib64/ld-linux-x86-64.so.2 ]]; then
  mkdir -p /out/lib64
  cp -aL /lib64/ld-linux-x86-64.so.2 /out/lib64/
fi
if [[ -e /lib/ld-linux-aarch64.so.1 ]]; then
  mkdir -p /out/lib
  cp -aL /lib/ld-linux-aarch64.so.1 /out/lib/
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
find "${OUT}" -type f | sort
ls -lh "${OUT}/usr/local/bin/"
du -sh "${OUT}"
