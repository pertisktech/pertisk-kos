#!/usr/bin/env bash
# Create one additional Pertisk node (worker or control plane) on Proxmox and join it.
#
# Required env (same as lab-up / upload-vm):
#   PROXMOX_URL, PROXMOX_TOKEN_ID, PROXMOX_TOKEN_SECRET, PROXMOX_NODE, PROXMOX_STORAGE
# Optional: PROXMOX_INSECURE=1, PROXMOX_SSH, PROXMOX_BRIDGE, LAB_SUBNET
#
# Example:
#   ./scripts/proxmox-add-node.sh \
#     --role worker --vmid 216 --name lab-wk-4 \
#     --memory 8192 --cores 4 --disk-gb 75 \
#     --cluster-out ./data/kubeconfigs/lab --cluster-name lab \
#     --cp-ip 10.1.1.48
set -euo pipefail

ROOT="${PERTISK_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
if [[ -n "${PERTISKCTL:-}" && -x "${PERTISKCTL}" ]]; then
  CTL="${PERTISKCTL}"
elif [[ -x "${ROOT}/out/bin/pertiskctl" ]]; then
  CTL="${ROOT}/out/bin/pertiskctl"
elif command -v pertiskctl >/dev/null 2>&1; then
  CTL="$(command -v pertiskctl)"
else
  CTL="${ROOT}/out/bin/pertiskctl"
fi
IP_TIMEOUT="${IP_TIMEOUT:-300}"
API_TIMEOUT="${API_TIMEOUT:-180}"

ROLE="worker"
VMID=""
NAME=""
MEMORY=8192
CORES=4
DISK_GB=""
DISK=""
CLUSTER_OUT=""
CLUSTER_NAME=""
CP_IP=""
CP_INDEX=""
BRIDGE="${PROXMOX_BRIDGE:-vmbr0}"

log() { printf '==> %s\n' "$*" >&2; }
die() { echo "error: $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --role) ROLE="$2"; shift 2 ;;
    --vmid) VMID="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --cluster-out) CLUSTER_OUT="$2"; shift 2 ;;
    --cluster-name) CLUSTER_NAME="$2"; shift 2 ;;
    --cp-ip) CP_IP="$2"; shift 2 ;;
    --controlplane-index) CP_INDEX="$2"; shift 2 ;;
    --bridge) BRIDGE="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *) die "unknown arg: $1" ;;
  esac
done

[[ -n "$VMID" && -n "$NAME" && -n "$CLUSTER_OUT" && -n "$CLUSTER_NAME" && -n "$CP_IP" ]] \
  || die "require --vmid --name --cluster-out --cluster-name --cp-ip"
[[ "$ROLE" == "worker" || "$ROLE" == "controlplane" ]] || die "role must be worker|controlplane"
[[ -x "$UPLOAD" ]] || chmod +x "$UPLOAD"
[[ -x "$CTL" ]] || die "pertiskctl missing at $CTL (make pertiskctl)"

: "${PROXMOX_URL:?set PROXMOX_URL}"
: "${PROXMOX_TOKEN_ID:?set PROXMOX_TOKEN_ID}"
: "${PROXMOX_TOKEN_SECRET:?set PROXMOX_TOKEN_SECRET}"
: "${PROXMOX_NODE:?set PROXMOX_NODE}"

ARCH="${PERTISK_ARCH:-amd64}"
IMAGES_DIR="${PERTISK_IMAGES_DIR:-${PROXMOX_IMAGES_DIR:-}}"
if [[ -z "$IMAGES_DIR" ]]; then
  for _img_d in /var/lib/pertisk-mgmt/images "${ROOT}/out" "${ROOT}/images"; do
    if [[ -d "$_img_d" ]]; then
      IMAGES_DIR="$_img_d"
      break
    fi
  done
fi
IMAGES_DIR="${IMAGES_DIR:-${ROOT}/out}"
unset _img_d
if [[ -z "$DISK" ]]; then
  if [[ -n "$DISK_GB" ]]; then
    for _cand in \
      "${IMAGES_DIR}/pertisk-cloud-${ARCH}-${DISK_GB}g.qcow2" \
      "${ROOT}/out/pertisk-cloud-${ARCH}-${DISK_GB}g.qcow2"; do
      if [[ -f "$_cand" ]]; then
        DISK="$_cand"
        break
      fi
    done
  fi
  if [[ -z "$DISK" ]]; then
    for _cand in \
      "${PROXMOX_DISK:-}" \
      "${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2" \
      "${ROOT}/out/pertisk-cloud-${ARCH}.qcow2"; do
      [[ -n "$_cand" && -f "$_cand" ]] || continue
      DISK="$_cand"
      break
    done
  fi
  DISK="${DISK:-${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2}"
fi
unset _cand
[[ -f "$DISK" ]] || die "disk not found: $DISK (set PROXMOX_DISK or copy qcow2 into ${IMAGES_DIR}/)"
log "disk=${DISK}"

# Auto subnet / optional SSH (API disk import by default — same as lab-up).
PVE_HOST="${PROXMOX_URL#*://}"
PVE_HOST="${PVE_HOST%%:*}"
if [[ "${PROXMOX_NO_SSH:-0}" == "1" ]]; then
  unset PROXMOX_SSH || true
elif [[ -z "${PROXMOX_SSH:-}" && "${PROXMOX_SSH_AUTO:-0}" == "1" && "$PVE_HOST" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  export PROXMOX_SSH="root@${PVE_HOST}"
  log "auto PROXMOX_SSH=${PROXMOX_SSH} (PROXMOX_SSH_AUTO=1)"
fi
if [[ -z "${LAB_SUBNET:-}" && "${PVE_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
  LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
  log "auto LAB_SUBNET=${LAB_SUBNET}"
fi
if [[ -z "${PROXMOX_SSH:-}" && -z "${PROXMOX_UPLOAD_STORAGE:-}" ]]; then
  export PROXMOX_UPLOAD_STORAGE=local
fi

CURL=(curl -sS)
[[ "${PROXMOX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
AUTH="Authorization: PVEAPIToken=${PROXMOX_TOKEN_ID}=${PROXMOX_TOKEN_SECRET}"
BASE="${PROXMOX_URL%/}/api2/json"
NODE="${PROXMOX_NODE}"

api_get() { "${CURL[@]}" -H "${AUTH}" "${BASE}$1"; }

vm_mac() {
  local vmid="$1" net0 mac
  net0="$(api_get "/nodes/${NODE}/qemu/${vmid}/config" | jq -r '.data.net0 // empty')"
  [[ -n "$net0" ]] || die "VM ${vmid}: no net0"
  if [[ "$net0" =~ ([0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}) ]]; then
    mac="${BASH_REMATCH[1]}"
  else
    die "VM ${vmid}: net0 has no MAC yet (${net0})"
  fi
  echo "${mac}" | tr 'A-F' 'a-f'
}

arp_ip_for_mac() {
  local mac="$1" out=""
  mac="$(echo "$mac" | tr 'A-F' 'a-f')"
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    out="$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o BatchMode=yes "${PROXMOX_SSH}" \
      "ip -4 neigh show | awk 'BEGIN{IGNORECASE=1} \$0 ~ /${mac}/ {print \$1; exit}'" \
      2>/dev/null || true)"
  else
    out="$(ip -4 neigh show 2>/dev/null | awk -v m="$mac" 'BEGIN{IGNORECASE=1} $0 ~ m {print $1; exit}' || true)"
    if [[ -z "$out" ]] && command -v arp >/dev/null 2>&1; then
      out="$(arp -an 2>/dev/null | tr '[:upper:]' '[:lower:]' | grep -F "$mac" \
        | sed -n 's/.*(\([0-9.]*\)).*/\1/p' | head -1 || true)"
    fi
  fi
  if [[ "$out" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$out"
  fi
}

nudge_arp_subnet() {
  local cidr="$1" base
  [[ -n "$cidr" ]] || return 0
  base="${cidr%/*}"; base="${base%.*}"
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    log "nudge ARP on ${PROXMOX_SSH} for ${base}.0/24"
    ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 -o BatchMode=yes "${PROXMOX_SSH}" \
      "base=${base}; for i in \$(seq 1 254); do ping -c1 -W1 \${base}.\$i >/dev/null 2>&1 & done; wait" \
      >/dev/null 2>&1 || true
  else
    log "nudge ARP locally for ${base}.0/24 (no PROXMOX_SSH)"
    (
      local i
      for i in $(seq 1 254); do
        ping -c1 -W1 "${base}.${i}" >/dev/null 2>&1 &
        if (( i % 64 == 0 )); then wait || true; fi
      done
      wait || true
    ) >/dev/null 2>&1 || true
  fi
}

scan_api_subnet_for_mac() {
  local mac="$1" cidr="$2" base i
  [[ -n "$cidr" ]] || return 0
  mac="$(echo "$mac" | tr 'A-F' 'a-f')"
  base="${cidr%/*}"; base="${base%.*}"
  log "scan :50000 on ${base}.0/24 (parallel) for MAC ${mac}"
  (
    for i in $(seq 1 254); do
      if command -v nc >/dev/null 2>&1; then
        nc -z -w 1 "${base}.${i}" 50000 >/dev/null 2>&1 &
      else
        timeout 1 bash -c "echo >/dev/tcp/${base}.${i}/50000" 2>/dev/null &
      fi
      if (( i % 80 == 0 )); then wait || true; fi
    done
    wait || true
  ) >/dev/null 2>&1 || true
  arp_ip_for_mac "$mac"
}

api_reachable() {
  local ip="$1"
  if command -v nc >/dev/null 2>&1; then
    nc -z -w 2 "$ip" 50000 >/dev/null 2>&1
  else
    "${CURL[@]}" --connect-timeout 2 "http://${ip}:50000" >/dev/null 2>&1 || return 1
  fi
}

wait_ip() {
  local vmid="$1" label="$2" mac ip="" deadline nudged=0
  mac="$(vm_mac "$vmid")"
  log "VM ${vmid} (${label}) MAC=${mac} — waiting for DHCP IP (timeout ${IP_TIMEOUT}s)"
  deadline=$((SECONDS + IP_TIMEOUT))
  while (( SECONDS < deadline )); do
    ip="$(arp_ip_for_mac "$mac" || true)"
    if [[ -z "$ip" && -n "${LAB_SUBNET:-}" ]]; then
      if [[ "$nudged" == "0" ]] || (( SECONDS % 45 < 3 )); then
        nudged=1
        nudge_arp_subnet "$LAB_SUBNET"
        ip="$(arp_ip_for_mac "$mac" || true)"
        if [[ -z "$ip" ]]; then
          ip="$(scan_api_subnet_for_mac "$mac" "$LAB_SUBNET" || true)"
        fi
      fi
    fi
    if [[ -n "$ip" ]] && api_reachable "$ip"; then
      log "VM ${vmid} → ${ip} (API :50000 up)"
      echo "$ip"
      return 0
    fi
    if [[ -n "$ip" ]]; then
      log "VM ${vmid} ARP=${ip} but :50000 not ready yet..."
    else
      log "VM ${vmid} waiting for DHCP..."
    fi
    sleep 3
  done
  die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (PROXMOX_SSH=${PROXMOX_SSH:-unset} subnet=${LAB_SUBNET:-unset})"
}

wait_api() {
  local ip="$1" deadline=$((SECONDS + API_TIMEOUT))
  while (( SECONDS < deadline )); do
    if "$CTL" -e "${ip}:50000" version >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  die "pertiskctl API not ready at ${ip}:50000"
}

set_hostname_yaml() {
  local src="$1" dest="$2" host="$3"
  awk -v h="$host" '
    BEGIN { done=0 }
    /^  network:/ { innet=1 }
    innet && /^    hostname:/ && !done { print "    hostname: " h; done=1; next }
    { print }
  ' "$src" >"$dest"
}

mkdir -p "$CLUSTER_OUT"
command -v jq >/dev/null || die "jq required"

log "create ${ROLE} VMID=${VMID} name=${NAME} mem=${MEMORY} cores=${CORES} disk=${DISK} disk-gb=${DISK_GB:-image}"
UPLOAD_ARGS=(
  --vmid "$VMID"
  --name "$NAME"
  --disk "$DISK"
  --memory "$MEMORY"
  --cores "$CORES"
  --bridge "$BRIDGE"
)
# Always pass --disk-gb when set so base/fallback images are grown after import.
[[ -n "$DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$DISK_GB")
if [[ -n "$DISK_GB" && "$DISK" == *"pertisk-cloud-${ARCH}.qcow2" && "$DISK" != *"-${DISK_GB}g.qcow2" ]]; then
  log "note: using base image — will qm-resize scsi0 → ${DISK_GB}G after import"
fi
"$UPLOAD" "${UPLOAD_ARGS[@]}"

log "wait for guest Machine API"
NODE_IP="$(wait_ip "$VMID" "$NAME" | tr -d '[:space:]')"
[[ "$NODE_IP" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "wait_ip returned invalid address: ${NODE_IP}"

wait_api "$CP_IP"

if [[ "$ROLE" == "worker" ]]; then
  [[ -f "$CLUSTER_OUT/worker.yaml" ]] || die "missing $CLUSTER_OUT/worker.yaml — refresh join-config from CP"
  log "refresh worker join CA from CP ${CP_IP}"
  "$CTL" -e "${CP_IP}:50000" join-config -f "$CLUSTER_OUT/worker.yaml"
  wyaml="${CLUSTER_OUT}/worker-${NAME##*-}.yaml"
  # Prefer index from name suffix wk-N
  if [[ "$NAME" =~ wk-([0-9]+)$ ]]; then
    wyaml="${CLUSTER_OUT}/worker-${BASH_REMATCH[1]}.yaml"
  fi
  set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$NAME"
  wait_api "$NODE_IP"
  log "apply join config → ${NAME} @ ${NODE_IP}"
  "$CTL" -e "${NODE_IP}:50000" apply -f "$wyaml"
else
  idx="${CP_INDEX:-}"
  if [[ -z "$idx" && "$NAME" =~ cp-([0-9]+)$ ]]; then
    idx="${BASH_REMATCH[1]}"
  fi
  [[ -n "$idx" ]] || die "controlplane requires --controlplane-index or name …-cp-N"
  cpyaml="${CLUSTER_OUT}/controlplane-${idx}.yaml"
  [[ "$idx" == "1" ]] && cpyaml="${CLUSTER_OUT}/controlplane.yaml"
  log "get-join-config for ${NAME} (index ${idx})"
  "$CTL" -e "${CP_IP}:50000" get-join-config \
    --controlplane --controlplane-index "$idx" --cluster-name "$CLUSTER_NAME" \
    -o "$cpyaml"
  set_hostname_yaml "$cpyaml" "${cpyaml}.tmp" "$NAME"
  mv "${cpyaml}.tmp" "$cpyaml"
  wait_api "$NODE_IP"
  log "apply + join-controlplane ${NAME} @ ${NODE_IP}"
  "$CTL" -e "${NODE_IP}:50000" apply -f "$cpyaml"
  sleep 5
  wait_api "$NODE_IP"
  etcd_ep="https://${CP_IP}:2379"
  join_try=0
  until "$CTL" -e "${NODE_IP}:50000" join-controlplane --etcd-endpoints "$etcd_ep"; do
    join_try=$((join_try + 1))
    (( join_try < 5 )) || die "join-controlplane failed after ${join_try} attempts"
    log "join-controlplane retry ${join_try}/5..."
    sleep 10
    wait_api "$NODE_IP"
  done
fi

log "node ${NAME} joined ip=${NODE_IP} vmid=${VMID}"
echo "NODE_IP=${NODE_IP}"
