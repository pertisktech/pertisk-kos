#!/usr/bin/env bash
# pertisk-vms lab-up: same bootstrap as proxmox-lab-up.sh with PROVIDER_KIND=pertisk-vms.
#
#   export PERTISK_VMS_URL=https://10.1.1.80:7443
#   export PERTISK_VMS_USER=admin
#   export PERTISK_VMS_PASSWORD=…
#   export PERTISK_VMS_STORAGE=replica
#   export PERTISK_VMS_NETWORK=vmbr0
#   export PERTISK_VMS_INSECURE=1
#   ./scripts/pertisk-vms-lab-up.sh --skip-build --cp-vmid 210 --workers 1
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PROVIDER_KIND=pertisk-vms
export CREATE_VMS="${ROOT}/scripts/pertisk-vms-create-cluster-vms.sh"
if [[ -n "${PERTISK_VMS_DISK:-}" ]]; then
  export PROXMOX_DISK="${PERTISK_VMS_DISK}"
fi
exec "${ROOT}/scripts/proxmox-lab-up.sh" "$@"
