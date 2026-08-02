#!/usr/bin/env bash
# Create 1 control-plane + N worker VMs on Proxmox from a Pertisk cloud qcow2.
# Does NOT bootstrap Kubernetes — use pertiskctl gen config / apply / bootstrap.
#
# Auth: same as proxmox-upload-vm.sh (PROXMOX_URL, PROXMOX_TOKEN_*, …).
# If PROXMOX_URL is unset, loads assignments from ./proxmox.sh (skips its `exec`).
#
# Example:
#   ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"

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

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cp-vmid) CP_VMID="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --prefix) NAME_PREFIX="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,14p' "$0"
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

cat <<EOF

VMs created (same cloud image for CP and workers). Next:

  # Discover CP IP from Serial / DHCP, then:
  make pertiskctl
  ./out/bin/pertiskctl gen config lab-ha https://<CP_IP>:6443 -o ./out/cluster
  # Edit hostnames in YAMLs if needed, then:
  ./out/bin/pertiskctl -e <CP_IP>:50000 apply -f ./out/cluster/controlplane.yaml
  ./out/bin/pertiskctl -e <CP_IP>:50000 bootstrap
  ./out/bin/pertiskctl -e <CP_IP>:50000 kubeconfig -f ./out/cluster/admin.conf
  ./out/bin/pertiskctl -e <CP_IP>:50000 join-config -f ./out/cluster/worker.yaml
  # Apply bootstrap token Secret (written on CP) then workers:
  #   kubectl --kubeconfig ./out/cluster/admin.conf apply -f ...
  # Apply worker.yaml to each worker :50000
  # Install CNI: kubectl apply -f examples/cni/kube-flannel.yaml

See docs/PROXMOX.md (Talos-shaped cluster section).
EOF
