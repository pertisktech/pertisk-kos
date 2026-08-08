#!/usr/bin/env bash
# 13900HX lab deploy → almalinux@10.1.1.150
#
# Steps:
#   1. Bump VERSION below
#   2. ./deploy-13900hx.sh
#
# Default: amd64 cloud image + mgmt RPM. Also configures PROXMOX_SSH for
# reliable qcow→ZFS import (scp+qm). Use ARCH=both for arm64 too.
set -euo pipefail

# --- edit me ---
VERSION="${VERSION:-0.1.94}"
PVE="${PVE:-10.1.1.195}"   # Proxmox node for PROXMOX_SSH (disk import / qm)
# ---------------

ROOT="$(cd "$(dirname "$0")" && pwd)"
MGMT="${MGMT:-almalinux@10.1.1.150}"
CP_GB="${CP_GB:-50}"
WORKER_GB="${WORKER_GB:-75}"
# Default amd64 (this lab). Override: ARCH=arm64|both ./deploy-13900hx.sh
ARCHS=(amd64)
case "$(printf '%s' "${ARCH:-amd64}" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64) ARCHS=(amd64) ;;
  arm64|aarch64) ARCHS=(arm64) ;;
  both|all) ARCHS=(amd64 arm64) ;;
  *) echo "unsupported ARCH=${ARCH} (use amd64|arm64|both)" >&2; exit 1 ;;
esac

echo "==> deploy-13900hx → ${MGMT} version=${VERSION} arch=${ARCHS[*]} pve=${PVE}"

first=1
for a in "${ARCHS[@]}"; do
  echo ""
  echo "==> [${a}] stage + copy images"
  ARGS=(
    --mgmt "$MGMT"
    --arch "$a"
    --cp-gb "$CP_GB"
    --worker-gb "$WORKER_GB"
    --version "$VERSION"
  )
  # Install RPM once; configure SSH to PVE (ZFS importdisk + optional arm64 qm).
  if [[ "$first" == "1" ]]; then
    ARGS+=(--with-ssh --pve "$PVE")
  else
    ARGS+=(--skip-rpm)
  fi
  first=0
  "$ROOT/scripts/deploy-mgmt-lab.sh" "${ARGS[@]}"
done

docker system prune -a -f
echo "==> done (${ARCHS[*]} images @ ${MGMT}:/var/lib/pertisk-mgmt/images/)"
echo "    PROXMOX_SSH=root@${PVE} (amd64 ZFS disk import; also arm64 qm create)"
