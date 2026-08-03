#!/usr/bin/env bash
# Create 1 control-plane + N worker VMs on Proxmox from a Pertisk cloud qcow2,
# then by default continue into lab-up (DHCP IPs → bootstrap → join → CNI).
#
# Auth: same as proxmox-upload-vm.sh (PROXMOX_URL, PROXMOX_TOKEN_*, …).
# If PROXMOX_URL is unset, loads assignments from ./proxmox.sh (skips its `exec`).
#
# Examples:
#   ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
#   ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2 --no-lab-up
#   CNI=cilium ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
LAB_UP_SH="${ROOT}/scripts/proxmox-lab-up.sh"

# Load local credentials without running proxmox.sh's trailing `exec`.
load_proxmox_sh() {
  local f="$1" line
  [[ -f "$f" ]] || return 0
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    [[ -z "${line// }" ]] && continue
    [[ "$line" =~ ^set[[:space:]] ]] && continue
    [[ "$line" =~ ^exec[[:space:]] ]] && break
    # shellcheck disable=SC2086
    eval "$line"
  done <"$f"
}

if [[ -z "${PROXMOX_URL:-}" ]]; then
  echo "==> loading Proxmox env from ${ROOT}/proxmox.sh"
  load_proxmox_sh "${ROOT}/proxmox.sh"
fi

CP_VMID="${CP_VMID:-210}"
WORKERS="${WORKERS:-2}"
NAME_PREFIX="${NAME_PREFIX:-pertisk}"
DISK="${PROXMOX_DISK:-${ROOT}/out/pertisk-cloud-amd64.qcow2}"
# Chain into bootstrap/join/CNI after VMs exist (disable with --no-lab-up).
DO_LAB_UP=1
LAB_UP_EXTRA=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cp-vmid) CP_VMID="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --prefix) NAME_PREFIX="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --lab-up) DO_LAB_UP=1; shift ;;
    --no-lab-up) DO_LAB_UP=0; shift ;;
    --cni)
      LAB_UP_EXTRA+=(--cni "$2")
      shift 2
      ;;
    --subnet)
      LAB_UP_EXTRA+=(--subnet "$2")
      shift 2
      ;;
    -h|--help)
      sed -n '2,16p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

if [[ -z "${PROXMOX_URL:-}" ]]; then
  echo "PROXMOX_URL unset. Copy proxmox.sh.example → proxmox.sh and fill token, or export PROXMOX_*." >&2
  exit 1
fi

if [[ ! -f "$DISK" ]]; then
  echo "disk not found: $DISK (build with: make cloud ARCH=amd64)" >&2
  exit 1
fi
if [[ ! -x "$UPLOAD" ]]; then
  chmod +x "$UPLOAD" || true
fi

echo "==> control-plane VMID=${CP_VMID} name=${NAME_PREFIX}-cp-1 disk=${DISK}"
"$UPLOAD" --vmid "$CP_VMID" --name "${NAME_PREFIX}-cp-1" --disk "$DISK"

for i in $(seq 1 "$WORKERS"); do
  wvid=$((CP_VMID + i))
  echo "==> worker VMID=${wvid} name=${NAME_PREFIX}-wk-${i}"
  "$UPLOAD" --vmid "$wvid" --name "${NAME_PREFIX}-wk-${i}" --disk "$DISK"
done

echo "==> VMs created (CP=${CP_VMID}, workers=${WORKERS})"

if [[ "$DO_LAB_UP" != "1" ]]; then
  cat <<EOF

Stopped after VM create (--no-lab-up). Continue with:
  ./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cp-vmid ${CP_VMID} --workers ${WORKERS}
EOF
  exit 0
fi

echo "==> continuing → lab-up (IPs → cluster → CNI)"
chmod +x "$LAB_UP_SH" 2>/dev/null || true
exec "$LAB_UP_SH" --skip-build --skip-vms \
  --cp-vmid "$CP_VMID" \
  --workers "$WORKERS" \
  --prefix "$NAME_PREFIX" \
  --disk "$DISK" \
  "${LAB_UP_EXTRA[@]}"
