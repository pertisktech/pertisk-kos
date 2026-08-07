#!/usr/bin/env bash
# 13900HX lab deploy → almalinux@10.1.1.150
#
# Steps:
#   1. Bump VERSION below
#   2. ./deploy-13900hx.sh
#
# Default: builds + deploys both amd64 and arm64 cloud images, then mgmt RPM.
# Also installs pertisk-mgmt → root@PVE SSH keys (needed for arm64 guest arch).
set -euo pipefail

# --- edit me ---
VERSION="${VERSION:-0.1.70}"
PVE="${PVE:-10.1.1.194}"   # Proxmox node for PROXMOX_SSH (arm64 qm create)
# ---------------

ROOT="$(cd "$(dirname "$0")" && pwd)"
MGMT="${MGMT:-almalinux@10.1.1.150}"
CP_GB="${CP_GB:-50}"
WORKER_GB="${WORKER_GB:-75}"
# Default both; override with: ARCH=amd64 ./deploy-13900hx.sh
ARCHS=(amd64 arm64)
case "$(printf '%s' "${ARCH:-both}" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64) ARCHS=(amd64) ;;
  arm64|aarch64) ARCHS=(arm64) ;;
  both|all|"") ARCHS=(amd64 arm64) ;;
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
  # Install RPM once; configure SSH to PVE on that first pass (arm64 needs it).
  if [[ "$first" == "1" ]]; then
    ARGS+=(--with-ssh --pve "$PVE")
  else
    ARGS+=(--skip-rpm)
  fi
  first=0
  "$ROOT/scripts/deploy-mgmt-lab.sh" "${ARGS[@]}"
done

docker system prune -a -f
echo "==> done (amd64 + arm64 images @ ${MGMT}:/var/lib/pertisk-mgmt/images/)"
echo "    PROXMOX_SSH=root@${PVE} (required for arm64 guest create)"
