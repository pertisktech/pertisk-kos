#!/usr/bin/env bash
# Download pinned containerd + kubelet (linux/amd64) into out/runtime/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out/runtime"
mkdir -p "${OUT}/usr/local/bin" "${OUT}/opt/cni/bin"

CONTAINERD_VER="${CONTAINERD_VER:-2.0.5}"
K8S_VER="${K8S_VER:-v1.32.5}"
ARCH="${PERTISK_ARCH:-amd64}"

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

echo "==> runtime tree ready at ${OUT}"
find "${OUT}" -type f | sort
