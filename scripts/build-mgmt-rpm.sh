#!/usr/bin/env bash
# Build pertisk-mgmt RPM for linux/amd64 (API + embedded UI).
#
#   ./scripts/build-mgmt-rpm.sh
#   VERSION=0.2.0 ./scripts/build-mgmt-rpm.sh
#   make mgmt-rpm
#
# Output: out/rpm/pertisk-mgmt-<version>-1.x86_64.rpm
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)}"
OUT_DIR="${OUT_DIR:-${ROOT}/out/rpm}"
IMAGE_TAG="pertisk-mgmt-rpm:${VERSION}"

command -v docker >/dev/null 2>&1 || {
  echo "docker is required to build the linux/amd64 RPM" >&2
  exit 1
}

mkdir -p "${OUT_DIR}"
echo "==> build pertisk-mgmt RPM VERSION=${VERSION} ARCH=amd64 → ${OUT_DIR}"

# Prefer buildx for cross-platform from macOS; fall back to plain docker build.
if docker buildx version >/dev/null 2>&1; then
  docker buildx build \
    --platform linux/amd64 \
    --build-arg "VERSION=${VERSION}" \
    -f "${ROOT}/packaging/mgmt/Dockerfile.rpm" \
    --target export \
    -o "type=local,dest=${OUT_DIR}" \
    "${ROOT}"
else
  docker build \
    --platform linux/amd64 \
    --build-arg "VERSION=${VERSION}" \
    -f "${ROOT}/packaging/mgmt/Dockerfile.rpm" \
    --target pkg \
    -t "${IMAGE_TAG}" \
    "${ROOT}"
  cid="$(docker create "${IMAGE_TAG}")"
  trap 'docker rm -f "${cid}" >/dev/null 2>&1 || true' EXIT
  docker cp "${cid}:/out/." "${OUT_DIR}/"
fi

echo "==> RPM artifacts"
ls -lh "${OUT_DIR}"/pertisk-mgmt*.rpm 2>/dev/null || ls -lh "${OUT_DIR}"
echo ""
echo "Deploy (RHEL/Rocky/Alma amd64) — see docs/MGMT.md#rpm-deploy-linuxamd64:"
echo "  scp ${OUT_DIR}/pertisk-mgmt-*.rpm USER@MGMT_HOST:/tmp/"
echo "  ssh USER@MGMT_HOST 'sudo rpm -Uvh /tmp/pertisk-mgmt-*-1.x86_64.rpm && sudo systemctl enable --now pertisk-mgmt'"
echo "  # then: env (MGMT_SECRET_KEY), copy qcow2 → /var/lib/pertisk-mgmt/images, SSH key for pertisk-mgmt→PVE"
