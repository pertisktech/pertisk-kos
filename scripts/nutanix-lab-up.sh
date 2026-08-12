#!/usr/bin/env bash
# Nutanix AHV lab-up: same bootstrap as proxmox-lab-up.sh with PROVIDER_KIND=nutanix.
#
#   export NUTANIX_URL=https://10.1.1.50:9440
#   export NUTANIX_USER=admin
#   export NUTANIX_PASSWORD=…
#   export NUTANIX_STORAGE=SelfServiceContainer
#   export NUTANIX_NETWORK=vlan.100
#   export NUTANIX_INSECURE=1
#   ./scripts/nutanix-lab-up.sh --skip-build --cp-vmid 210 --workers 1
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PROVIDER_KIND=nutanix
export CREATE_VMS="${ROOT}/scripts/nutanix-create-cluster-vms.sh"
if [[ -n "${NUTANIX_DISK:-}" ]]; then
  export PROXMOX_DISK="${NUTANIX_DISK}"
fi
exec "${ROOT}/scripts/proxmox-lab-up.sh" "$@"
