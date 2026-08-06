#!/usr/bin/env bash
# ESXi lab-up: same bootstrap as proxmox-lab-up.sh with PROVIDER_KIND=vsphere.
#
#   export VSPHERE_URL=https://10.1.1.20
#   export VSPHERE_USER=root
#   export VSPHERE_PASSWORD=…
#   export VSPHERE_DATASTORE=datastore1
#   export VSPHERE_NETWORK='VM Network'
#   export VSPHERE_INSECURE=1
#   ./scripts/vsphere-lab-up.sh --skip-build --cp-vmid 210 --workers 1
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PROVIDER_KIND=vsphere
export CREATE_VMS="${ROOT}/scripts/vsphere-create-cluster-vms.sh"
# Prefer VSPHERE_DISK; fall back to PROXMOX_DISK / images dir inside lab-up.
if [[ -n "${VSPHERE_DISK:-}" ]]; then
  export PROXMOX_DISK="${VSPHERE_DISK}"
fi
exec "${ROOT}/scripts/proxmox-lab-up.sh" "$@"
