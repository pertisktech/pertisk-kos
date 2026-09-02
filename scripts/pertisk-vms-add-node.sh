#!/usr/bin/env bash
# Create one additional Pertisk node (worker or control plane) on pertisk-vms and join it.
#
# Required env:
#   PERTISK_VMS_URL, PERTISK_VMS_USER, PERTISK_VMS_PASSWORD
# Optional: PERTISK_VMS_STORAGE, PERTISK_VMS_NETWORK, PERTISK_VMS_INSECURE=1,
#           LAB_SUBNET, PERTISK_VMS_DISK, PERTISK_IMAGES_DIR
#
# Example:
#   ./scripts/pertisk-vms-add-node.sh \
#     --role worker --vmid 211 --name lab-vms-wk-2 \
#     --memory 8192 --cores 4 --disk-gb 75 \
#     --cluster-out ./data/kubeconfigs/lab-vms --cluster-name lab-vms \
#     --cp-ip 10.1.1.80
set -euo pipefail

ROOT="${PERTISK_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
UPLOAD="${ROOT}/scripts/pertisk-vms-upload-vm.sh"
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
API_AFTER_IP_TIMEOUT="${API_AFTER_IP_TIMEOUT:-900}"
API_TIMEOUT="${API_TIMEOUT:-180}"

ROLE="worker"
VMID=""
NAME=""
MEMORY=4096
CORES=2
DISK_GB=""
DISK=""
ARCH=""
CLUSTER_OUT=""
CLUSTER_NAME=""
CP_IP=""
CP_INDEX=""
NETWORK="${PERTISK_VMS_NETWORK:-vmbr0}"

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
    --arch) ARCH="$2"; shift 2 ;;
    --cluster-out) CLUSTER_OUT="$2"; shift 2 ;;
    --cluster-name) CLUSTER_NAME="$2"; shift 2 ;;
    --cp-ip) CP_IP="$2"; shift 2 ;;
    --controlplane-index) CP_INDEX="$2"; shift 2 ;;
    --bridge|--network) NETWORK="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,16p' "$0"
      cat <<EOF

  --arch amd64|arm64   guest arch (default from ARCH/PERTISK_ARCH or amd64)
  --network NAME       pertisk-vms network or host bridge (default \$PERTISK_VMS_NETWORK; --bridge accepted as alias)
EOF
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

: "${PERTISK_VMS_URL:?set PERTISK_VMS_URL}"
: "${PERTISK_VMS_USER:?set PERTISK_VMS_USER}"
: "${PERTISK_VMS_PASSWORD:?set PERTISK_VMS_PASSWORD}"
PERTISK_VMS_STORAGE="${PERTISK_VMS_STORAGE:-replica}"
NETWORK="${NETWORK:-${PERTISK_VMS_NETWORK:-vmbr0}}"
[[ -n "$NETWORK" ]] || die "set PERTISK_VMS_NETWORK or --network"
export PERTISK_VMS_NETWORK="$NETWORK"
export PERTISK_VMS_STORAGE
export PERTISK_VMS_INSECURE="${PERTISK_VMS_INSECURE:-1}"

ARCH="${ARCH:-${PERTISK_ARCH:-amd64}}"
case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) die "unsupported --arch=${ARCH} (use amd64|arm64)" ;;
esac
export PERTISK_ARCH="$ARCH"

IMAGES_DIR="${PERTISK_IMAGES_DIR:-${PERTISK_VMS_IMAGES_DIR:-}}"
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
      "${PERTISK_VMS_DISK:-}" \
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
[[ -f "$DISK" ]] || die "disk not found: $DISK (set PERTISK_VMS_DISK or copy qcow2 into ${IMAGES_DIR}/)"
log "arch=${ARCH} disk=${DISK}"

PVMS_HOST="${PERTISK_VMS_URL#*://}"
PVMS_HOST="${PVMS_HOST%%:*}"
if [[ -z "${LAB_SUBNET:-}" && "${PVMS_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
  LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
  log "auto LAB_SUBNET=${LAB_SUBNET}"
fi

TOKEN=""
pvms_curl() {
  local args=(curl -sS)
  [[ "${PERTISK_VMS_INSECURE:-0}" == "1" ]] && args+=(-k)
  "${args[@]}" "$@"
}

pvms_login() {
  TOKEN="$(pvms_curl -X POST "${PERTISK_VMS_URL%/}/v1/login" \
    -H 'Content-Type: application/json' \
    -d "{\"username\":\"${PERTISK_VMS_USER}\",\"password\":\"${PERTISK_VMS_PASSWORD}\"}" \
    | jq -r '.token // empty')"
  [[ -n "$TOKEN" ]] || die "pertisk-vms login failed"
}

pvms_get() {
  pvms_curl -H "Authorization: Bearer ${TOKEN}" -H 'Accept: application/json' \
    "${PERTISK_VMS_URL%/}$1"
}

pvms_vm_json() {
  local name="$1"
  pvms_get /v1/vms | jq -c --arg n "$name" --arg id "$name" '
    (if type=="array" then . else (.vms // []) end)
    | map(select((.spec.name == $n) or (.id|tostring) == $id))
    | .[0] // empty
  ' 2>/dev/null || true
}

pertisk_vms_vm_mac() {
  local json
  json="$(pvms_vm_json "$1")"
  [[ -n "$json" && "$json" != "null" ]] || return 0
  echo "$json" | jq -r '(.spec.nets // []) | map(.mac // empty) | map(select(. != "")) | .[0] // empty'
}

pertisk_vms_vm_ips() {
  local json
  json="$(pvms_vm_json "$1")"
  [[ -n "$json" && "$json" != "null" ]] || return 0
  echo "$json" | jq -r '(.spec.nets // []) | map(.ip // empty) | map(select(. != null and . != "")) | .[]'
}

arp_ip_for_mac() {
  local mac="$1" out="" mac_cmp
  mac="$(echo "$mac" | tr 'A-F' 'a-f')"
  mac_cmp="$(echo "$mac" | tr -d ':.-')"
  [[ -n "$mac_cmp" && ${#mac_cmp} -ge 8 ]] || return 0
  out="$(ip -4 neigh show 2>/dev/null | awk -v m="$mac" -v c="$mac_cmp" '
    BEGIN { IGNORECASE=1 }
    $0 ~ /lladdr/ {
      line=tolower($0)
      gsub(/:/, "", line); gsub(/-/, "", line); gsub(/\./, "", line)
      if (index(line, c) || tolower($0) ~ m) { print $1; exit }
    }' || true)"
  if [[ "$out" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    if [[ -n "${LAB_SUBNET:-}" ]]; then
      local base="${LAB_SUBNET%/*}"; base="${base%.*}."
      [[ "$out" == ${base}* ]] || return 0
    fi
    echo "$out"
  fi
}

nudge_arp_subnet() {
  local cidr="$1" base
  [[ -n "$cidr" ]] || return 0
  base="${cidr%/*}"; base="${base%.*}"
  log "nudge ARP locally for ${base}.0/24"
  (
    local i
    for i in $(seq 1 254); do
      ping -c1 -W1 "${base}.${i}" >/dev/null 2>&1 &
      if (( i % 80 == 0 )); then wait || true; fi
    done
    wait || true
  ) >/dev/null 2>&1 || true
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
    timeout 2 bash -c "echo >/dev/tcp/${ip}/50000" 2>/dev/null
  fi
}

guest_icmp_alive() {
  ping -c1 -W2 "$1" >/dev/null 2>&1
}

wait_ip() {
  local vmid="$1" label="$2" mac ip="" pvip="" alt="" nudged=0 saw_ip=0 last_log=0 live=0 issued=0
  local ip_deadline api_deadline=0 deadline
  mac="$(pertisk_vms_vm_mac "$label")"
  [[ -n "$mac" ]] || die "VM ${label}: no MAC from pertisk-vms yet; power on once so a MAC is assigned"
  log "VM ${vmid} (${label}) MAC=${mac} — waiting for DHCP IP (timeout ${IP_TIMEOUT}s; +${API_AFTER_IP_TIMEOUT}s after live IP for :50000)"
  ip_deadline=$((SECONDS + IP_TIMEOUT))
  while true; do
    if (( saw_ip )); then
      deadline=$api_deadline
    else
      deadline=$ip_deadline
    fi
    (( SECONDS < deadline )) || break

    ip="$(arp_ip_for_mac "$mac" || true)"
    issued=0
    pvip="$(pertisk_vms_vm_ips "$label" 2>/dev/null | head -1 || true)"
    if [[ -n "$pvip" ]]; then
      ip="$pvip"
      issued=1
    fi
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
    if [[ -n "$ip" && -n "${LAB_SUBNET:-}" ]] && ! api_reachable "$ip"; then
      if (( SECONDS % 60 < 5 )); then
        alt="$(scan_api_subnet_for_mac "$mac" "$LAB_SUBNET" || true)"
        if [[ -n "$alt" ]] && api_reachable "$alt"; then
          ip="$alt"
          issued=1
        fi
      fi
    fi
    if [[ -n "$ip" ]] && api_reachable "$ip"; then
      log "VM ${vmid} → ${ip} (API :50000 up)"
      echo "$ip"
      return 0
    fi
    if [[ -n "$ip" ]]; then
      live=0
      guest_icmp_alive "$ip" && live=1
      if (( live || issued )); then
        if (( !saw_ip )); then
          saw_ip=1
          api_deadline=$((SECONDS + API_AFTER_IP_TIMEOUT))
          last_log=$SECONDS
          if (( live )); then
            log "VM ${vmid} live=${ip} (ICMP ok) — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s)"
          else
            log "VM ${vmid} issued ${ip} — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s; ICMP optional)"
          fi
        elif (( SECONDS - last_log >= 20 )); then
          last_log=$SECONDS
          local left=$((api_deadline - SECONDS))
          (( left < 0 )) && left=0
          log "VM ${vmid} issued=${ip} but :50000 not ready yet... (${left}s left)"
        fi
      else
        saw_ip=0
        if (( SECONDS - last_log >= 15 )); then
          last_log=$SECONDS
          log "VM ${vmid} candidate IP=${ip} — waiting for ICMP…"
        fi
      fi
    else
      if (( SECONDS - last_log >= 15 )); then
        last_log=$SECONDS
        log "VM ${vmid} waiting for DHCP…"
      fi
    fi
    sleep 3
  done
  if (( saw_ip )); then
    die "timed out waiting for Machine API :50000 on ${ip:-?} (VM ${vmid} MAC=${mac})"
  fi
  die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (subnet=${LAB_SUBNET:-unset})"
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

apply_machine_yaml() {
  local ip="$1" yaml="$2" i
  [[ -f "$yaml" ]] || die "apply: missing ${yaml}"
  log "apply ${yaml##*/} → ${ip} (wait for STATE mount)"
  for i in $(seq 1 24); do
    if "$CTL" -e "${ip}:50000" apply -f "$yaml"; then
      sleep 8
      if "$CTL" -e "${ip}:50000" apply -f "$yaml"; then
        return 0
      fi
    fi
    log "apply not ready yet (try ${i}/24) — STATE may still be mounting"
    sleep 5
  done
  die "apply failed for ${ip} (${yaml})"
}

mkdir -p "$CLUSTER_OUT"
command -v jq >/dev/null || die "jq required"
pvms_login

log "create ${ROLE} VMID=${VMID} name=${NAME} arch=${ARCH} mem=${MEMORY} cores=${CORES} disk=${DISK} disk-gb=${DISK_GB:-image} network=${NETWORK}"
UPLOAD_ARGS=(
  --vmid "$VMID"
  --name "$NAME"
  --disk "$DISK"
  --memory "$MEMORY"
  --cores "$CORES"
  --network "$NETWORK"
)
[[ -n "$DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$DISK_GB")
"$UPLOAD" "${UPLOAD_ARGS[@]}"

log "wait for guest Machine API"
NODE_IP="$(wait_ip "$VMID" "$NAME" | tr -d '[:space:]')"
[[ "$NODE_IP" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "wait_ip returned invalid address: ${NODE_IP}"

wait_api "$CP_IP"
wait_api "$NODE_IP"

log "soft-reset ${NAME} @ ${NODE_IP} before join (clear leftover STATE)"
if "$CTL" -e "${NODE_IP}:50000" reset --force 2>&1; then
  sleep 8
  NODE_IP="$(wait_ip "$VMID" "$NAME" | tr -d '[:space:]')"
  wait_api "$NODE_IP"
else
  log "WARNING: reset ${NAME} failed — continuing (fresh guests are fine)"
fi

if [[ "$ROLE" == "worker" ]]; then
  [[ -f "$CLUSTER_OUT/worker.yaml" ]] || die "missing $CLUSTER_OUT/worker.yaml — refresh join-config from CP"
  log "refresh worker join CA from CP ${CP_IP}"
  "$CTL" -e "${CP_IP}:50000" join-config -f "$CLUSTER_OUT/worker.yaml"
  wyaml="${CLUSTER_OUT}/worker-${NAME##*-}.yaml"
  if [[ "$NAME" =~ wk-([0-9]+)$ ]]; then
    wyaml="${CLUSTER_OUT}/worker-${BASH_REMATCH[1]}.yaml"
  fi
  set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$NAME"
  wait_api "$NODE_IP"
  log "apply join config → ${NAME} @ ${NODE_IP}"
  apply_machine_yaml "$NODE_IP" "$wyaml"
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
  apply_machine_yaml "$NODE_IP" "$cpyaml"
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
