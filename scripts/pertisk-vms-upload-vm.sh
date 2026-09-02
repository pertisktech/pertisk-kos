#!/usr/bin/env bash
# Upload a Pertisk cloud qcow2 to pertisk-vms (pertiskd) and create a UEFI VM.
#
# Auth:
#   export PERTISK_VMS_URL="https://10.1.1.80:7443"
#   export PERTISK_VMS_USER="admin"
#   export PERTISK_VMS_PASSWORD="…"
#   export PERTISK_VMS_STORAGE="replica"
#   export PERTISK_VMS_NETWORK="vmbr0"
#   export PERTISK_VMS_INSECURE=1
#
#   ./scripts/pertisk-vms-upload-vm.sh --vmid 9100 --name pertisk-worker-1 \
#     --disk out/pertisk-cloud-amd64.qcow2
set -euo pipefail

VMID=""
NAME="pertisk-worker"
DISK=""
MEMORY="${PERTISK_VMS_MEMORY:-4096}"
CORES="${PERTISK_VMS_CORES:-2}"
DISK_GB="${PERTISK_VMS_DISK_GB:-}"
NETWORK="${PERTISK_VMS_NETWORK:-vmbr0}"
STORAGE="${PERTISK_VMS_STORAGE:-replica}"
START=1
IMPORT_ONLY=0
STATIC_IP=""
ARCH="${PERTISK_ARCH:-${ARCH:-amd64}}"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/pertisk-vms-upload-vm.sh --vmid ID --disk PATH [options]

Options:
  --vmid N          numeric VM id (required unless --import-only)
  --disk PATH       qcow2 path (required)
  --name NAME       VM name (default pertisk-worker)
  --arch ARCH       amd64|arm64
  --memory MB       RAM (default 4096)
  --cores N         vCPUs (default 2)
  --disk-gb N       grow cloned volume to N GiB
  --network NAME    pertisk-vms network name or bridge (default vmbr0)
  --storage NAME    replica | rbd
  --no-start        do not start after create
  --import-only     upload template volume only
  --ip CIDR_OR_IP   optional static IPv4 for NIC IPAM
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vmid) VMID="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --arch) ARCH="$2"; PERTISK_ARCH="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --network|--bridge) NETWORK="$2"; shift 2 ;;
    --storage) STORAGE="$2"; shift 2 ;;
    --no-start) START=0; shift ;;
    --import-only) IMPORT_ONLY=1; shift ;;
    --ip) STATIC_IP="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

log() { printf '==> %s\n' "$*" >&2; }
die() { echo "error: $*" >&2; exit 1; }

[[ -n "${PERTISK_VMS_URL:-}" ]] || die "PERTISK_VMS_URL unset"
[[ -n "${PERTISK_VMS_USER:-}" ]] || die "PERTISK_VMS_USER unset"
[[ -n "${PERTISK_VMS_PASSWORD:-}" ]] || die "PERTISK_VMS_PASSWORD unset"
[[ -f "$DISK" ]] || die "disk not found: ${DISK:-unset}"
command -v jq >/dev/null || die "jq required"
command -v curl >/dev/null || die "curl required"

case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) die "unsupported --arch=${ARCH}" ;;
esac

BASE="${PERTISK_VMS_URL%/}"
CURL=(curl -sS)
[[ "${PERTISK_VMS_INSECURE:-0}" == "1" ]] && CURL+=(-k)

TOKEN=""
login() {
  TOKEN="$("${CURL[@]}" -X POST "${BASE}/v1/login" \
    -H 'Content-Type: application/json' \
    -d "{\"username\":\"${PERTISK_VMS_USER}\",\"password\":\"${PERTISK_VMS_PASSWORD}\"}" \
    | jq -r '.token // empty')"
  [[ -n "$TOKEN" ]] || die "login failed"
}
auth() { echo "Authorization: Bearer ${TOKEN}"; }

api() {
  local method="$1" path="$2"
  shift 2
  "${CURL[@]}" -X "$method" "${BASE}${path}" -H "$(auth)" -H 'Accept: application/json' "$@"
}

login

tmpl_name="kos-cloud-${ARCH}"
disk_base="$(basename "$DISK")"
if [[ "$disk_base" == *arm64* ]]; then
  tmpl_name="kos-cloud-arm64"
elif [[ "$disk_base" == *amd64* ]]; then
  tmpl_name="kos-cloud-amd64"
fi

find_volume_id() {
  local want="$1"
  api GET /v1/volumes | jq -r --arg n "$want" '
    (if type=="array" then . else (.volumes // []) end)
    | map(select(.name == $n)) | .[0].id // empty
  '
}

ensure_template() {
  local id
  id="$(find_volume_id "$tmpl_name")"
  if [[ -n "$id" ]]; then
    log "reuse template volume ${tmpl_name} id=${id}"
    echo "$id"
    return 0
  fi
  log "import template ${tmpl_name} from ${DISK}"
  local resp
  resp="$("${CURL[@]}" -X POST "${BASE}/v1/volumes/import?name=$(printf '%s' "$tmpl_name" | jq -sRr @uri)&format=qcow2" \
    -H "$(auth)" -H 'Accept: application/json' \
    --data-binary @"${DISK}")" || die "volume import failed"
  echo "$resp" | jq -r '.id // empty'
}

TMPL_ID="$(ensure_template)"
[[ -n "$TMPL_ID" ]] || die "template import did not return a volume id"
if [[ "$IMPORT_ONLY" == "1" ]]; then
  log "import-only complete template=${tmpl_name} id=${TMPL_ID}"
  echo "OK template=${tmpl_name} id=${TMPL_ID}"
  exit 0
fi

[[ -n "$VMID" ]] || die "--vmid required"
[[ "$VMID" =~ ^[0-9]{3,10}$ ]] || die "vmid must be 3–10 digits (got ${VMID})"

ensure_network() {
  local id
  id="$(api GET /v1/networks | jq -r --arg n "$NETWORK" '
    (if type=="array" then . else [] end)
    | map(select(.name == $n or .bridge == $n)) | .[0].id // empty
  ')"
  if [[ -n "$id" ]]; then
    echo "$id"
    return 0
  fi
  log "create network ${NETWORK} on bridge ${NETWORK}"
  api POST /v1/networks -H 'Content-Type: application/json' -d "$(jq -n \
    --arg name "$NETWORK" --arg br "$NETWORK" \
    '{name:$name, bridge:$br, mode:"bridge", cidr:"10.88.0.0/24", dhcp:false, isolate:false}')" \
    | jq -r '.id // empty'
}

NET_ID="$(ensure_network)"
[[ -n "$NET_ID" ]] || die "could not find or create network ${NETWORK}"

# Recreate if a VM with this name or id already exists.
existing="$(api GET /v1/vms | jq -r --arg n "$NAME" --arg id "$VMID" '
  (if type=="array" then . else (.vms // []) end)
  | map(select((.spec.name == $n) or ((.id|tostring) == $id)))
  | .[0].id // empty
')"
if [[ -n "$existing" ]]; then
  log "delete existing VM ${existing}"
  api POST "/v1/vms/${existing}/stop" >/dev/null 2>&1 || true
  api DELETE "/v1/vms/${existing}" >/dev/null 2>&1 || true
fi

vol_name="${NAME}-disk"
old_vol="$(find_volume_id "$vol_name")"
if [[ -n "$old_vol" ]]; then
  log "delete leftover volume ${vol_name}"
  api DELETE "/v1/volumes/${old_vol}" >/dev/null 2>&1 || true
fi

log "clone template ${tmpl_name} → ${vol_name}"
VOL_ID="$(api POST "/v1/volumes/${TMPL_ID}/clone" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg n "$vol_name" '{name:$n, linked:false}')" | jq -r '.id // empty')"
[[ -n "$VOL_ID" ]] || die "clone failed"

if [[ -n "$DISK_GB" ]]; then
  bytes=$((DISK_GB * 1024 * 1024 * 1024))
  log "resize ${vol_name} to ${DISK_GB} GiB"
  api POST "/v1/volumes/${VOL_ID}/resize" -H 'Content-Type: application/json' \
    -d "$(jq -n --argjson n "$bytes" '{size_bytes:$n}')" >/dev/null
fi

log "create VM ${NAME} id=${VMID} cores=${CORES} memory=${MEMORY}"
api POST /v1/vms -H 'Content-Type: application/json' -d "$(jq -n \
  --argjson id "$VMID" --arg name "$NAME" --argjson cpus "$CORES" --argjson mem "$MEMORY" \
  '{id:$id, name:$name, vcpus:$cpus, memory_mib:$mem, ha:true}')" >/dev/null

api POST "/v1/vms/${VMID}/disks" -H 'Content-Type: application/json' \
  -d "$(jq -n --arg id "$VOL_ID" '{volume_id:$id}')" >/dev/null

nic_body="$(jq -n --arg id "$NET_ID" '{network_id:$id}')"
if [[ -n "$STATIC_IP" ]]; then
  ip="${STATIC_IP%%/*}"
  nic_body="$(jq -n --arg id "$NET_ID" --arg ip "$ip" '{network_id:$id, ip:$ip}')"
fi
api POST "/v1/vms/${VMID}/nics" -H 'Content-Type: application/json' -d "$nic_body" >/dev/null

if [[ "$START" == "1" ]]; then
  log "start VM ${VMID}"
  api POST "/v1/vms/${VMID}/start" >/dev/null
fi

mac="$(api GET "/v1/vms/${VMID}" | jq -r '(.spec.nets // []) | map(.mac // empty) | map(select(. != "")) | .[0] // empty')"
log "OK ${NAME} id=${VMID} mac=${mac:-unknown} volume=${VOL_ID}"
echo "OK name=${NAME} id=${VMID} mac=${mac:-} volume=${VOL_ID}"
