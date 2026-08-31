#!/usr/bin/env bash
# Create N control-plane + M worker VMs on ESXi from a Pertisk cloud qcow2,
# then by default continue into vsphere-lab-up (DHCP IPs → bootstrap → join → CNI).
#
# Auth: VSPHERE_URL, VSPHERE_USER, VSPHERE_PASSWORD, VSPHERE_DATASTORE, VSPHERE_NETWORK.
#
# Examples:
#   ./scripts/vsphere-create-cluster-vms.sh --cp-vmid 210 --workers 2
#   ./scripts/vsphere-create-cluster-vms.sh --cp-vmid 210 --controlplanes 1 --workers 0 --no-lab-up
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPLOAD="${ROOT}/scripts/vsphere-upload-vm.sh"
LAB_UP_SH="${ROOT}/scripts/vsphere-lab-up.sh"
# shellcheck source=pertisk-parallel.sh
. "$(cd "$(dirname "$0")" && pwd)/pertisk-parallel.sh"

MEMORY="${VSPHERE_MEMORY:-4096}"
CORES="${VSPHERE_CORES:-2}"
CP_MEMORY="${VSPHERE_CP_MEMORY:-}"
CP_CORES="${VSPHERE_CP_CORES:-}"
WORKER_MEMORY="${VSPHERE_WORKER_MEMORY:-}"
WORKER_CORES="${VSPHERE_WORKER_CORES:-}"
DISK_GB="${VSPHERE_DISK_GB:-}"
CP_DISK_GB="${VSPHERE_CP_DISK_GB:-}"
WORKER_DISK_GB="${VSPHERE_WORKER_DISK_GB:-}"
CP_VMID="${CP_VMID:-210}"
CONTROLPLANES="${CONTROLPLANES:-1}"
WORKERS="${WORKERS:-2}"
NAME_PREFIX="${NAME_PREFIX:-pertisk}"
DISK="${VSPHERE_DISK:-${PROXMOX_DISK:-${ROOT}/out/pertisk-cloud-amd64.qcow2}}"
CP_DISK="${VSPHERE_CP_DISK:-}"
WORKER_DISK="${VSPHERE_WORKER_DISK:-}"
DO_LAB_UP=1
LAB_UP_EXTRA=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cp-vmid) CP_VMID="$2"; shift 2 ;;
    --controlplanes) CONTROLPLANES="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --prefix) NAME_PREFIX="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --cp-disk) CP_DISK="$2"; shift 2 ;;
    --worker-disk) WORKER_DISK="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --cp-memory) CP_MEMORY="$2"; shift 2 ;;
    --cp-cores) CP_CORES="$2"; shift 2 ;;
    --worker-memory) WORKER_MEMORY="$2"; shift 2 ;;
    --worker-cores) WORKER_CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --cp-disk-gb) CP_DISK_GB="$2"; shift 2 ;;
    --worker-disk-gb) WORKER_DISK_GB="$2"; shift 2 ;;
    --arch) ARCH="$2"; PERTISK_ARCH="$2"; shift 2 ;;
    --lab-up) DO_LAB_UP=1; shift ;;
    --no-lab-up) DO_LAB_UP=0; shift ;;
    --cni) LAB_UP_EXTRA+=(--cni "$2"); shift 2 ;;
    --vip) LAB_UP_EXTRA+=(--vip "$2"); shift 2 ;;
    --subnet) LAB_UP_EXTRA+=(--subnet "$2"); shift 2 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

case "${ARCH:-${PERTISK_ARCH:-amd64}}" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported --arch=${ARCH:-} (use amd64|arm64)" >&2; exit 1 ;;
esac
export ARCH PERTISK_ARCH="$ARCH"

CP_MEMORY="${CP_MEMORY:-$MEMORY}"
CP_CORES="${CP_CORES:-$CORES}"
WORKER_MEMORY="${WORKER_MEMORY:-$MEMORY}"
WORKER_CORES="${WORKER_CORES:-$CORES}"
CP_DISK_GB="${CP_DISK_GB:-$DISK_GB}"
WORKER_DISK_GB="${WORKER_DISK_GB:-$DISK_GB}"
CP_DISK="${CP_DISK:-$DISK}"
WORKER_DISK="${WORKER_DISK:-$DISK}"

if [[ -z "${VSPHERE_URL:-}" ]]; then
  echo "VSPHERE_URL unset. Export VSPHERE_URL / VSPHERE_USER / VSPHERE_PASSWORD." >&2
  exit 1
fi

if [[ ! -f "$CP_DISK" ]]; then
  echo "CP disk not found: $CP_DISK" >&2
  exit 1
fi
if [[ ! -f "$WORKER_DISK" ]] && [[ "$WORKERS" -gt 0 ]]; then
  echo "worker disk not found: $WORKER_DISK" >&2
  exit 1
fi
chmod +x "$UPLOAD" 2>/dev/null || true

if [[ "$CONTROLPLANES" -lt 1 ]]; then
  echo "ERROR: --controlplanes must be >= 1" >&2
  exit 1
fi

# Naming: {prefix}-cp-N / {prefix}-wk-N (same as Proxmox; matches mgmt seed stubs + k8s node names).
pertisk_parallel_init

# Parse static IPs from env (space-separated list for all nodes).
# If provided, assign one to each VM in order via --ip flag.
STATIC_IPS_ARRAY=()
if [[ -n "${VSPHERE_STATIC_IPS:-}" ]]; then
  read -ra STATIC_IPS_ARRAY <<<"${VSPHERE_STATIC_IPS}"
fi
STATIC_IP_IDX=0

for i in $(seq 1 "$CONTROLPLANES"); do
  cvid=$((CP_VMID + i - 1))
  echo "==> control-plane VMID=${cvid} name=${NAME_PREFIX}-cp-${i} disk=${CP_DISK} mem=${CP_MEMORY} cores=${CP_CORES}"
  UPLOAD_ARGS=(--vmid "$cvid" --name "${NAME_PREFIX}-cp-${i}" --disk "$CP_DISK" --memory "$CP_MEMORY" --cores "$CP_CORES")
  [[ -n "$CP_DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$CP_DISK_GB")
  if [[ $STATIC_IP_IDX -lt ${#STATIC_IPS_ARRAY[@]} ]]; then
    UPLOAD_ARGS+=(--ip "${STATIC_IPS_ARRAY[$STATIC_IP_IDX]}")
    ((STATIC_IP_IDX++))
  fi
  pertisk_parallel_add "${NAME_PREFIX}-cp-${i}" "$UPLOAD" "${UPLOAD_ARGS[@]}"
done

for i in $(seq 1 "$WORKERS"); do
  wvid=$((CP_VMID + CONTROLPLANES + i - 1))
  echo "==> worker VMID=${wvid} name=${NAME_PREFIX}-wk-${i} disk=${WORKER_DISK} mem=${WORKER_MEMORY} cores=${WORKER_CORES}"
  UPLOAD_ARGS=(--vmid "$wvid" --name "${NAME_PREFIX}-wk-${i}" --disk "$WORKER_DISK" --memory "$WORKER_MEMORY" --cores "$WORKER_CORES")
  [[ -n "$WORKER_DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$WORKER_DISK_GB")
  if [[ $STATIC_IP_IDX -lt ${#STATIC_IPS_ARRAY[@]} ]]; then
    UPLOAD_ARGS+=(--ip "${STATIC_IPS_ARRAY[$STATIC_IP_IDX]}")
    ((STATIC_IP_IDX++))
  fi
  pertisk_parallel_add "${NAME_PREFIX}-wk-${i}" "$UPLOAD" "${UPLOAD_ARGS[@]}"
done
pertisk_parallel_wait

echo "==> VMs created (CP=${CP_VMID}..$((CP_VMID + CONTROLPLANES - 1)), workers=${WORKERS})"

# Ensure Autostart list includes every VM for this prefix (MoRefs change on recreate).
ENABLE_AS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/vsphere-enable-autostart.sh"
if [[ -x "$ENABLE_AS" ]] || [[ -f "$ENABLE_AS" ]]; then
  echo "==> sync host Autostart for prefix=${NAME_PREFIX}"
  chmod +x "$ENABLE_AS" 2>/dev/null || true
  "$ENABLE_AS" --prefix "$NAME_PREFIX" || echo "warn: autostart sync failed (VMs still created)" >&2
fi

if [[ "$DO_LAB_UP" != "1" ]]; then
  cat <<EOF

Stopped after VM create (--no-lab-up). Continue with:
  ./scripts/vsphere-lab-up.sh --skip-build --skip-vms --cp-vmid ${CP_VMID} \\
    --controlplanes ${CONTROLPLANES} --workers ${WORKERS} --prefix ${NAME_PREFIX}
EOF
  exit 0
fi

echo "==> continuing → vsphere-lab-up (IPs → cluster → CNI)"
chmod +x "$LAB_UP_SH" 2>/dev/null || true
LAB_ARGS=(
  --skip-build --skip-vms
  --cp-vmid "$CP_VMID"
  --controlplanes "$CONTROLPLANES"
  --workers "$WORKERS"
  --prefix "$NAME_PREFIX"
  --arch "$ARCH"
  --disk "$DISK"
  --cp-memory "$CP_MEMORY"
  --cp-cores "$CP_CORES"
  --worker-memory "$WORKER_MEMORY"
  --worker-cores "$WORKER_CORES"
)
[[ -n "$CP_DISK_GB" ]] && LAB_ARGS+=(--cp-disk-gb "$CP_DISK_GB")
[[ -n "$WORKER_DISK_GB" ]] && LAB_ARGS+=(--worker-disk-gb "$WORKER_DISK_GB")
exec "$LAB_UP_SH" "${LAB_ARGS[@]}" "${LAB_UP_EXTRA[@]}"
