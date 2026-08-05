#!/usr/bin/env bash
# Build (optional) + stage Pertisk cloud qcow2s for RPM mgmt hosts.
#
# On a build machine (needs Docker / make cloud deps):
#   ./scripts/stage-cloud-images.sh
#   ./scripts/stage-cloud-images.sh --skip-build --cp-gb 50 --worker-gb 75
#   DEST=/tmp/pertisk-images ./scripts/stage-cloud-images.sh
#
# Then copy to the mgmt host images dir (any transport — scp/rsync/USB/HTTP):
#   scp "$DEST"/pertisk-cloud-amd64*.qcow2 almalinux@mgmt:/tmp/
#   ssh almalinux@mgmt 'sudo mv /tmp/pertisk-cloud-amd64*.qcow2 /var/lib/pertisk-mgmt/images/
#     && sudo chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images'
#
# Disk import to Proxmox does NOT require SSH if you set on the mgmt host:
#   PROXMOX_NO_SSH=1
#   PROXMOX_UPLOAD_STORAGE=local   # directory storage with content=import
# (ZFS-only labs are more reliable with PROXMOX_SSH=root@<pve>.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="${PERTISK_ARCH:-${ARCH:-amd64}}"
DEST="${DEST:-${ROOT}/out}"
CP_GB="${CP_GB:-50}"
WORKER_GB="${WORKER_GB:-75}"
SKIP_BUILD=0

usage() {
  sed -n '2,18p' "$0"
  cat <<EOF

Flags:
  --skip-build     reuse existing out/pertisk-cloud-\${ARCH}.qcow2
  --cp-gb N        control-plane virtual size (default ${CP_GB})
  --worker-gb N    worker virtual size (default ${WORKER_GB})
  --arch ARCH      amd64|arm64 (default ${ARCH})
  --dest DIR       output directory (default ${DEST}; env DEST)
  -h, --help
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --cp-gb) CP_GB="$2"; shift 2 ;;
    --worker-gb) WORKER_GB="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

BASE="${ROOT}/out/pertisk-cloud-${ARCH}.qcow2"
mkdir -p "$DEST"

if [[ "$SKIP_BUILD" != "1" ]]; then
  echo "==> make cloud ARCH=${ARCH}"
  make -C "$ROOT" cloud ARCH="$ARCH"
fi
[[ -f "$BASE" ]] || {
  echo "ERROR: missing ${BASE} — run without --skip-build, or make cloud first" >&2
  exit 1
}

resize_to() {
  local src="$1" dest="$2" gb="$3"
  echo "==> ${dest} (${gb}G)"
  if [[ "$(cd "$(dirname "$src")" && pwd)/$(basename "$src")" == "$(cd "$(dirname "$dest")" && pwd)/$(basename "$dest")" ]]; then
    echo "ERROR: resize src and dest are the same path: $dest" >&2
    exit 1
  fi
  cp -f "$src" "$dest"
  if command -v qemu-img >/dev/null 2>&1; then
    qemu-img resize "$dest" "${gb}G"
  else
    docker run --rm -v "$(cd "$(dirname "$dest")" && pwd):/work" alpine:3.20 \
      sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img resize /work/$(basename "$dest") ${gb}G"
  fi
}

# DEST often equals out/ — avoid `cp identical file` failing under set -e.
if [[ "$(cd "$(dirname "$BASE")" && pwd)/$(basename "$BASE")" != "$(cd "$DEST" && pwd)/pertisk-cloud-${ARCH}.qcow2" ]]; then
  cp -f "$BASE" "${DEST}/pertisk-cloud-${ARCH}.qcow2"
else
  echo "==> base already at ${DEST}/pertisk-cloud-${ARCH}.qcow2"
fi
resize_to "$BASE" "${DEST}/pertisk-cloud-${ARCH}-${CP_GB}g.qcow2" "$CP_GB"
if [[ "$WORKER_GB" == "$CP_GB" ]]; then
  cp -f "${DEST}/pertisk-cloud-${ARCH}-${CP_GB}g.qcow2" \
    "${DEST}/pertisk-cloud-${ARCH}-${WORKER_GB}g.qcow2"
else
  resize_to "$BASE" "${DEST}/pertisk-cloud-${ARCH}-${WORKER_GB}g.qcow2" "$WORKER_GB"
fi

echo
echo "==> staged in ${DEST}:"
ls -lh "${DEST}/pertisk-cloud-${ARCH}"*.qcow2
echo
echo "Install on mgmt host:"
echo "  scp ${DEST}/pertisk-cloud-${ARCH}*.qcow2 USER@MGMT:/tmp/"
echo "  ssh USER@MGMT 'sudo bash -c \"mkdir -p /var/lib/pertisk-mgmt/images && mv /tmp/pertisk-cloud-${ARCH}*.qcow2 /var/lib/pertisk-mgmt/images/ && chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images\"'"
echo
echo "Proxmox disk import (pick one in /etc/pertisk-mgmt/pertisk-mgmt.env):"
echo "  # A) SSH (best for local-zfs):"
echo "  PROXMOX_SSH=root@<pve-ip>"
echo "  # B) API only (no SSH) — upload via directory storage, import to VM storage:"
echo "  PROXMOX_NO_SSH=1"
echo "  PROXMOX_UPLOAD_STORAGE=local"
