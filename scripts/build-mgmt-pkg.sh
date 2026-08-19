#!/usr/bin/env bash
# Build pertisk-mgmt Linux packages (RPM + DEB) via Docker buildx.
#
#   ./scripts/build-mgmt-pkg.sh
#   VERSION=0.2.0 ./scripts/build-mgmt-pkg.sh
#   PKG_PLATFORMS=linux/amd64 ./scripts/build-mgmt-pkg.sh
#   make mgmt-pkg
#
# Output: out/pkg/pertisk-mgmt_*-*.deb and out/pkg/pertisk-mgmt-*-*.rpm
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)}"
OUT_DIR="${OUT_DIR:-${ROOT}/out/pkg}"
PKG_PLATFORMS="${PKG_PLATFORMS:-linux/amd64,linux/arm64}"
DOCKERFILE="${ROOT}/packaging/mgmt/Dockerfile.pkg"

command -v docker >/dev/null 2>&1 || {
  echo "docker is required to build pertisk-mgmt packages" >&2
  exit 1
}

mkdir -p "${OUT_DIR}"

NET_ARGS=()
if [[ "$(uname -s)" == Linux ]]; then
  NET_ARGS+=(--network host)
fi

HAS_BUILDX=0
if docker buildx version >/dev/null 2>&1; then
  HAS_BUILDX=1
fi

IFS=',' read -r -a PLATFORMS <<< "${PKG_PLATFORMS}"
for raw in "${PLATFORMS[@]}"; do
  plat="$(echo "${raw}" | xargs)"
  [[ -n "${plat}" ]] || continue
  echo "==> build pertisk-mgmt packages VERSION=${VERSION} PLATFORM=${plat} → ${OUT_DIR}"
  # bash 3.2 + set -u: empty "${arr[@]}" is unbound (macOS). Only expand when set.
  if [[ "${HAS_BUILDX}" -eq 1 ]]; then
    docker buildx build \
      ${NET_ARGS[@]+"${NET_ARGS[@]}"} \
      --platform "${plat}" \
      --build-arg "VERSION=${VERSION}" \
      -f "${DOCKERFILE}" \
      --target export \
      -o "type=local,dest=${OUT_DIR}" \
      "${ROOT}"
  else
    tag="pertisk-mgmt-pkg:${VERSION}-$(echo "${plat}" | tr '/' '-')"
    docker build \
      ${NET_ARGS[@]+"${NET_ARGS[@]}"} \
      --platform "${plat}" \
      --build-arg "VERSION=${VERSION}" \
      -f "${DOCKERFILE}" \
      --target pkg \
      -t "${tag}" \
      "${ROOT}"
    cid="$(docker create "${tag}")"
    docker cp "${cid}:/out/." "${OUT_DIR}/"
    docker rm -f "${cid}" >/dev/null
  fi
done

# Keep out/rpm for existing lab/deploy scripts that look there.
mkdir -p "${ROOT}/out/rpm"
shopt -s nullglob
rpms=("${OUT_DIR}"/pertisk-mgmt-*.rpm)
if [[ ${#rpms[@]} -gt 0 ]]; then
  cp -f "${rpms[@]}" "${ROOT}/out/rpm/"
fi

echo "==> package artifacts"
ls -lh "${OUT_DIR}"/pertisk-mgmt*.{rpm,deb} "${OUT_DIR}"/pertiskctl*.{rpm,deb} "${OUT_DIR}"/pertiskctl-linux-* 2>/dev/null || ls -lh "${OUT_DIR}"
echo ""
echo "Install mgmt RPM:  sudo rpm -Uvh ${OUT_DIR}/pertisk-mgmt-*.rpm"
echo "Install mgmt DEB:  sudo apt-get install -y ${OUT_DIR}/pertisk-mgmt_*.deb"
echo "CLI only RPM:      sudo rpm -Uvh ${OUT_DIR}/pertiskctl-*.rpm"
echo "CLI only DEB:      sudo apt-get install -y ${OUT_DIR}/pertiskctl_*.deb"
echo "CLI binary:        ${OUT_DIR}/pertiskctl-linux-{amd64,arm64}"
echo "See docs/MGMT.md"
