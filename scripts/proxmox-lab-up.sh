#!/usr/bin/env bash
# End-to-end lab: build image → create Proxmox VMs → wait for DHCP IPs (MAC→ARP)
# → bootstrap control-plane → join workers → install CNI + DNS + addons (+ optional apps).
#
# Prerequisites: ./proxmox.sh (or PROXMOX_* env), jq, curl, kubectl; helm if CNI=cilium.
# Recommended: PROXMOX_SSH=root@<pve> so MAC→IP can read the node's ARP/neigh table.
#
# Examples:
#   ./scripts/proxmox-lab-up.sh
#   ./scripts/proxmox-lab-up.sh --skip-build --cni cilium --workers 2
#   ./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.200 --workers 2 --cni cilium
#   ./scripts/proxmox-lab-up.sh --dual-stack --vip 10.1.1.210 --vip6 fd00:1::210 --cni cilium
#   ./scripts/proxmox-lab-up.sh --skip-build --cni calico
#   ./scripts/proxmox-lab-up.sh --skip-build --cni flannel
#   ./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cp-vmid 210   # reuse VMs / resume HA
#   CNI=flannel APPS="examples/apps/nginx.yaml" ./scripts/proxmox-lab-up.sh --skip-build
#   ./scripts/proxmox-lab-up.sh --skip-addons   # skip optional reflector (CoreDNS + metrics-server always)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
CREATE_VMS="${ROOT}/scripts/proxmox-create-cluster-vms.sh"
CTL="${ROOT}/out/bin/pertiskctl"
CLUSTER_OUT="${CLUSTER_OUT:-${ROOT}/out/cluster}"
DISK="${PROXMOX_DISK:-${ROOT}/out/pertisk-cloud-amd64.qcow2}"

MEMORY="${PROXMOX_MEMORY:-4096}"
CORES="${PROXMOX_CORES:-2}"
CP_MEMORY="${PROXMOX_CP_MEMORY:-}"
CP_CORES="${PROXMOX_CP_CORES:-}"
WORKER_MEMORY="${PROXMOX_WORKER_MEMORY:-}"
WORKER_CORES="${PROXMOX_WORKER_CORES:-}"
DISK_GB="${PERTISK_DISK_GB:-}"
CP_DISK_GB="${PROXMOX_CP_DISK_GB:-}"
WORKER_DISK_GB="${PROXMOX_WORKER_DISK_GB:-}"
CP_VMID="${CP_VMID:-210}"
CONTROLPLANES="${CONTROLPLANES:-1}"
VIP="${VIP:-}"
VIP6="${VIP6:-}"
DUAL_STACK="${DUAL_STACK:-0}"
WORKERS="${WORKERS:-2}"
NAME_PREFIX="${NAME_PREFIX:-pertisk}"
CLUSTER_NAME="${CLUSTER_NAME:-lab-ha}"
K8S_VER="${K8S_VER:-v1.36.3}"
CNI="${CNI:-cilium}"          # cilium | calico | flannel | none
CALICO_VERSION="${CALICO_VERSION:-v3.29.3}"
ARCH="${ARCH:-amd64}"
SKIP_BUILD=0
SKIP_VMS=0
SKIP_ADDONS=0
IP_TIMEOUT="${IP_TIMEOUT:-300}"
API_TIMEOUT="${API_TIMEOUT:-300}"
BOOTSTRAP_TIMEOUT="${BOOTSTRAP_TIMEOUT:-300}"
LAB_SUBNET="${LAB_SUBNET:-}"  # optional CIDR for ping-sweep fallback, e.g. 10.1.1.0/24
REFLECTOR_YAML="${REFLECTOR_YAML:-https://github.com/emberstack/kubernetes-reflector/releases/latest/download/reflector.yaml}"
METRICS_SERVER_YAML="${METRICS_SERVER_YAML:-${ROOT}/examples/addons/metrics-server.yaml}"

usage() {
  sed -n '2,16p' "$0"
  cat <<EOF

Flags:
  --cp-vmid N         first control-plane VMID (default ${CP_VMID})
  --controlplanes N   stacked CP count (default ${CONTROLPLANES}; >1 needs --vip)
  --vip IP            kube-vip ARP address (required when --controlplanes > 1)
  --vip6 ADDR         optional IPv6 API VIP (with --dual-stack)
  --dual-stack        opt-in IPv4+IPv6 (networkMode, Cilium ipv6, pod/service CIDRs)
  --workers N         worker count (default ${WORKERS})
  --prefix NAME       Proxmox VM name prefix (default ${NAME_PREFIX})
  --cluster NAME      Kubernetes / hostname prefix (default ${CLUSTER_NAME})
  --cni NAME          cilium|calico|flannel|none (default ${CNI})
  --k8s VER           kubernetesVersion for gen config (default ${K8S_VER})
  --disk PATH         cloud qcow2 (default ${DISK})
  --memory MB         default RAM for CP and workers (default ${MEMORY}; env PROXMOX_MEMORY)
  --cores N           default vCPUs for CP and workers (default ${CORES}; env PROXMOX_CORES)
  --cp-memory MB      control-plane RAM (env PROXMOX_CP_MEMORY)
  --cp-cores N        control-plane vCPUs (env PROXMOX_CP_CORES)
  --worker-memory MB  worker RAM (env PROXMOX_WORKER_MEMORY)
  --worker-cores N    worker vCPUs (env PROXMOX_WORKER_CORES)
  --disk-gb N         default disk GiB for both roles (env PERTISK_DISK_GB)
  --cp-disk-gb N      control-plane image + scsi0 GiB (env PROXMOX_CP_DISK_GB)
  --worker-disk-gb N  worker image + scsi0 GiB (env PROXMOX_WORKER_DISK_GB)
  --skip-build        do not run make cloud
  --skip-vms          do not create/recreate VMs (resolve IPs on existing VMIDs)
  --skip-addons       skip optional reflector (CoreDNS + metrics-server always installed)
  --subnet CIDR       ping-sweep fallback when ARP miss (e.g. 10.1.1.0/24)
  -h, --help

Env: PROXMOX_*, PROXMOX_SSH, APPS (space/comma-separated kubectl apply paths)
     CALICO_VERSION (default ${CALICO_VERSION})
     CONTROLPLANES, VIP, VIP6, DUAL_STACK=1
     PROXMOX_MEMORY / PROXMOX_CORES (defaults for both roles)
     PROXMOX_CP_MEMORY / PROXMOX_CP_CORES / PROXMOX_WORKER_MEMORY / PROXMOX_WORKER_CORES
     PROXMOX_CP_DISK_GB / PROXMOX_WORKER_DISK_GB / PERTISK_DISK_GB
EOF
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cp-vmid) CP_VMID="$2"; shift 2 ;;
    --controlplanes) CONTROLPLANES="$2"; shift 2 ;;
    --vip) VIP="$2"; shift 2 ;;
    --vip6) VIP6="$2"; shift 2 ;;
    --dual-stack) DUAL_STACK=1; shift ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --prefix) NAME_PREFIX="$2"; shift 2 ;;
    --cluster) CLUSTER_NAME="$2"; shift 2 ;;
    --cni) CNI="$2"; shift 2 ;;
    --k8s) K8S_VER="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --cp-memory) CP_MEMORY="$2"; shift 2 ;;
    --cp-cores) CP_CORES="$2"; shift 2 ;;
    --worker-memory) WORKER_MEMORY="$2"; shift 2 ;;
    --worker-cores) WORKER_CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --cp-disk-gb) CP_DISK_GB="$2"; shift 2 ;;
    --worker-disk-gb) WORKER_DISK_GB="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-vms) SKIP_VMS=1; shift ;;
    --skip-addons) SKIP_ADDONS=1; shift ;;
    --subnet) LAB_SUBNET="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

CP_MEMORY="${CP_MEMORY:-$MEMORY}"
CP_CORES="${CP_CORES:-$CORES}"
WORKER_MEMORY="${WORKER_MEMORY:-$MEMORY}"
WORKER_CORES="${WORKER_CORES:-$CORES}"
CP_DISK_GB="${CP_DISK_GB:-$DISK_GB}"
WORKER_DISK_GB="${WORKER_DISK_GB:-$DISK_GB}"

# Fast build: populate ~4G, then qemu-img resize per role. Guest grows GPT +
# EPHEMERAL on first boot. Distinct --*-disk-gb → separate sized qcow2s.
sized_qcow() {
  local gb="$1"
  echo "${ROOT}/out/pertisk-cloud-${ARCH}-${gb}g.qcow2"
}

CP_DISK="$DISK"
WORKER_DISK="$DISK"
if [[ -n "$CP_DISK_GB" ]]; then
  CP_DISK="$(sized_qcow "$CP_DISK_GB")"
fi
if [[ -n "$WORKER_DISK_GB" ]]; then
  WORKER_DISK="$(sized_qcow "$WORKER_DISK_GB")"
fi

if [[ "$CONTROLPLANES" -lt 1 ]]; then
  echo "ERROR: --controlplanes must be >= 1" >&2
  exit 1
fi
if [[ "$CONTROLPLANES" -gt 1 && -z "$VIP" ]]; then
  echo "ERROR: --vip IP is required when --controlplanes > 1" >&2
  exit 1
fi

# --- Proxmox env ---
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
: "${PROXMOX_URL:?set PROXMOX_URL (or proxmox.sh)}"
: "${PROXMOX_TOKEN_ID:?set PROXMOX_TOKEN_ID}"
: "${PROXMOX_TOKEN_SECRET:?set PROXMOX_TOKEN_SECRET}"
: "${PROXMOX_NODE:?set PROXMOX_NODE}"

# Derive PVE SSH + lab subnet from PROXMOX_URL when unset (lab default).
PVE_HOST="$(echo "${PROXMOX_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
if [[ -z "${PROXMOX_SSH:-}" && -n "${PVE_HOST}" ]]; then
  if ssh -o BatchMode=yes -o ConnectTimeout=3 -o StrictHostKeyChecking=accept-new \
    "root@${PVE_HOST}" true >/dev/null 2>&1; then
    export PROXMOX_SSH="root@${PVE_HOST}"
    echo "==> auto PROXMOX_SSH=${PROXMOX_SSH}"
  fi
fi
if [[ -z "${LAB_SUBNET}" && "${PVE_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
  LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
  echo "==> auto LAB_SUBNET=${LAB_SUBNET}"
fi

if [[ -z "${PROXMOX_SSH:-}" ]]; then
  echo "WARNING: PROXMOX_SSH unset — MAC→IP needs ARP on the PVE bridge." >&2
  echo "         export PROXMOX_SSH=root@${PVE_HOST:-<pve>}  (and/or --subnet 10.1.1.0/24)" >&2
fi

command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }

CURL=(curl -sS)
[[ "${PROXMOX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
AUTH="Authorization: PVEAPIToken=${PROXMOX_TOKEN_ID}=${PROXMOX_TOKEN_SECRET}"
BASE="${PROXMOX_URL%/}/api2/json"
NODE="${PROXMOX_NODE}"

api_get() {
  "${CURL[@]}" -H "${AUTH}" "${BASE}$1"
}

log() { printf '==> %s\n' "$*" >&2; }
die() { echo "error: $*" >&2; exit 1; }

# --- MAC / IP helpers ---
vm_mac() {
  local vmid="$1" net0 mac
  net0="$(api_get "/nodes/${NODE}/qemu/${vmid}/config" | jq -r '.data.net0 // empty')"
  [[ -n "$net0" ]] || die "VM ${vmid}: no net0"
  # net0 forms: virtio=AA:BB:...,bridge=vmbr0  OR  virtio,bridge=vmbr0 (no fixed MAC)
  if [[ "$net0" =~ ([0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}) ]]; then
    mac="${BASH_REMATCH[1]}"
  else
    die "VM ${vmid}: net0 has no MAC yet (${net0}); start the VM once so Proxmox assigns one"
  fi
  # normalize lowercase
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
  fi
  if [[ "$out" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$out"
  fi
}

# Populate PVE bridge ARP (guests often silent until nudged), then re-read neigh.
nudge_arp_subnet() {
  local cidr="$1" base
  [[ -n "$cidr" ]] || return 0
  [[ -n "${PROXMOX_SSH:-}" ]] || return 0
  base="${cidr%/*}"
  base="${base%.*}" # 10.1.1
  log "nudge ARP on ${PROXMOX_SSH} for ${base}.0/24"
  ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 -o BatchMode=yes "${PROXMOX_SSH}" \
    "base=${base}; for i in \$(seq 1 254); do ping -c1 -W1 \${base}.\$i >/dev/null 2>&1 & done; wait" \
    >/dev/null 2>&1 || true
}

ping_sweep_find() {
  local mac="$1" cidr="$2"
  [[ -n "$cidr" ]] || return 0
  nudge_arp_subnet "$cidr"
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
    if [[ -z "$ip" && -n "$LAB_SUBNET" ]]; then
      if [[ "$nudged" == "0" ]] || (( SECONDS % 30 < 3 )); then
        nudged=1
        ip="$(ping_sweep_find "$mac" "$LAB_SUBNET" || true)"
      fi
    fi
    if [[ -n "$ip" ]] && api_reachable "$ip"; then
      log "VM ${vmid} → ${ip} (API :50000 up)"
      echo "$ip"
      return 0
    fi
    if [[ -n "$ip" ]]; then
      log "VM ${vmid} ARP=${ip} but :50000 not ready yet..."
    fi
    sleep 3
  done
  die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (PROXMOX_SSH=${PROXMOX_SSH:-unset} subnet=${LAB_SUBNET:-unset})"
}

wait_api() {
  local ip="$1" deadline
  deadline=$((SECONDS + API_TIMEOUT))
  while (( SECONDS < deadline )); do
    if "$CTL" -e "${ip}:50000" version >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  die "pertiskctl API not ready at ${ip}:50000"
}

# Rewrite kubeconfig server URL (portable; works on macOS/Linux).
rewrite_kubeconfig_server() {
  local kc="$1" url="$2"
  python3 - "$kc" "$url" <<'PY'
import sys
path, url = sys.argv[1], sys.argv[2]
text = open(path).read()
out = []
for line in text.splitlines(True):
    if line.lstrip().startswith("server:"):
        pad = line[: len(line) - len(line.lstrip())]
        out.append(f"{pad}server: {url}\n")
    else:
        out.append(line)
open(path, "w").write("".join(out))
PY
}

# Wait for apiserver: always via a CP node IP first; then VIP when HA.
wait_apiserver_ready() {
  local kc="$1" cp_ip="$2" endpoint="$3"
  local deadline tmpkc

  # 1) Direct CP (kube-vip may still be pulling / electing).
  tmpkc="${kc}.direct"
  cp "$kc" "$tmpkc"
  rewrite_kubeconfig_server "$tmpkc" "https://${cp_ip}:6443"
  log "waiting for apiserver on CP ${cp_ip}:6443"
  deadline=$((SECONDS + BOOTSTRAP_TIMEOUT))
  until kubectl --kubeconfig "$tmpkc" get --raw=/readyz >/dev/null 2>&1; do
    (( SECONDS < deadline )) || { rm -f "$tmpkc"; die "apiserver not ready on ${cp_ip}:6443"; }
    sleep 3
  done
  log "apiserver ready on ${cp_ip}"

  if [[ "$endpoint" == "$cp_ip" ]]; then
    rm -f "$tmpkc"
    return 0
  fi

  # 2) VIP (kube-vip ARP). Allow extra time for image pull + lease.
  log "waiting for kube-vip ${endpoint}:6443"
  deadline=$((SECONDS + BOOTSTRAP_TIMEOUT + 180))
  until curl -sk --connect-timeout 2 "https://${endpoint}:6443/readyz" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      log "WARNING: VIP ${endpoint} not up — falling back kubeconfig to ${cp_ip} (check kube-vip static pod)"
      rewrite_kubeconfig_server "$kc" "https://${cp_ip}:6443"
      API_ENDPOINT="$cp_ip"
      rm -f "$tmpkc"
      return 0
    fi
    sleep 5
  done
  log "apiserver ready on VIP ${endpoint}"
  # Confirm kubectl via VIP kubeconfig
  deadline=$((SECONDS + 60))
  until kubectl --kubeconfig "$kc" get --raw=/readyz >/dev/null 2>&1; do
    (( SECONDS < deadline )) || die "kubectl via VIP kubeconfig failed"
    sleep 2
  done
  rm -f "$tmpkc"
}

set_hostname_yaml() {
  local src="$1" dest="$2" host="$3"
  # portable: replace first hostname: line under network
  awk -v h="$host" '
    BEGIN { done=0 }
    /^  network:/ { innet=1 }
    innet && /^    hostname:/ && !done { print "    hostname: " h; done=1; next }
    { print }
  ' "$src" >"$dest"
}

# --- steps ---
# Fast cloud build: populate ~4G, convert, qemu-img resize to role size.
# EPHEMERAL filesystem is expanded on first guest boot (pertisk-disk).
resize_qcow_to_gb() {
  local qcow="$1" gb="$2"
  [[ -f "$qcow" ]] || die "missing $qcow"
  [[ "$gb" =~ ^[0-9]+$ ]] || die "bad disk gb: $gb"
  log "qemu-img resize $(basename "$qcow") → ${gb}G"
  if command -v qemu-img >/dev/null 2>&1; then
    qemu-img resize "$qcow" "${gb}G"
  else
    local out_dir base
    out_dir="$(cd "$(dirname "$qcow")" && pwd)"
    base="$(basename "$qcow")"
    docker run --rm -v "${out_dir}:/work" alpine:3.20 \
      sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img resize /work/${base} ${gb}G"
  fi
}

build_cloud_base() {
  local role="${1:-wk}"
  export PERTISK_BUILD_DISK_GB="${PERTISK_BUILD_DISK_GB:-4}"
  # Virtual size after resize comes from PERTISK_DISK_GB; default = build size.
  export PERTISK_DISK_GB="${PERTISK_DISK_GB:-$PERTISK_BUILD_DISK_GB}"
  export PERTISK_HOSTNAME_ROLE="$role"
  unset PERTISK_HOSTNAME || true
  log "building cloud base (populate ${PERTISK_BUILD_DISK_GB}G, virtual ${PERTISK_DISK_GB}G)"
  if [[ "${CLOUD_BUILT:-0}" == "1" ]]; then
    PERTISK_ARCH="$ARCH" "$ROOT/image/build-cloud-image.sh"
  else
    make -C "$ROOT" cloud ARCH="$ARCH"
    CLOUD_BUILT=1
  fi
  [[ -f "$DISK" ]] || die "disk not produced: $DISK"
}

step_build() {
  if [[ "$SKIP_BUILD" == "1" ]]; then
    log "skip build"
    if [[ -n "$CP_DISK_GB" && ! -f "$CP_DISK" ]]; then
      [[ -f "$DISK" ]] || die "disk missing: $CP_DISK (and fallback $DISK)"
      log "warn: missing sized CP image $CP_DISK — using $DISK"
      CP_DISK="$DISK"
    fi
    if [[ -n "$WORKER_DISK_GB" && ! -f "$WORKER_DISK" ]]; then
      [[ -f "$DISK" ]] || die "disk missing: $WORKER_DISK (and fallback $DISK)"
      log "warn: missing sized worker image $WORKER_DISK — using $DISK"
      WORKER_DISK="$DISK"
    fi
    [[ -f "$CP_DISK" ]] || die "disk missing: $CP_DISK"
    [[ -f "$WORKER_DISK" ]] || die "disk missing: $WORKER_DISK"
    return 0
  fi

  CLOUD_BUILT=0
  # One populate+convert; clone and resize for CP/worker virtual sizes.
  local min_gb="${PERTISK_BUILD_DISK_GB:-4}"
  if [[ -n "$CP_DISK_GB" && "$CP_DISK_GB" -lt "$min_gb" ]]; then
    min_gb="$CP_DISK_GB"
  fi
  if [[ -n "$WORKER_DISK_GB" && "$WORKER_DISK_GB" -lt "$min_gb" ]]; then
    min_gb="$WORKER_DISK_GB"
  fi
  export PERTISK_BUILD_DISK_GB="${PERTISK_BUILD_DISK_GB:-4}"
  export PERTISK_DISK_GB="$min_gb"
  build_cloud_base wk

  if [[ -n "$CP_DISK_GB" || -n "$WORKER_DISK_GB" ]]; then
    if [[ -n "$CP_DISK_GB" ]]; then
      cp -f "$DISK" "$CP_DISK"
      resize_qcow_to_gb "$CP_DISK" "$CP_DISK_GB"
    fi
    if [[ -n "$WORKER_DISK_GB" ]]; then
      if [[ -n "$CP_DISK_GB" && "$CP_DISK_GB" == "$WORKER_DISK_GB" ]]; then
        cp -f "$CP_DISK" "$WORKER_DISK"
      else
        cp -f "$DISK" "$WORKER_DISK"
        resize_qcow_to_gb "$WORKER_DISK" "$WORKER_DISK_GB"
      fi
    fi
  fi

  [[ -f "$CP_DISK" ]] || die "CP disk missing after build: $CP_DISK"
  [[ -f "$WORKER_DISK" ]] || die "worker disk missing after build: $WORKER_DISK"
}

step_vms() {
  if [[ "$SKIP_VMS" == "1" ]]; then
    log "skip VM create (using existing VMIDs from ${CP_VMID})"
    if [[ -n "${CP_DISK_GB}${WORKER_DISK_GB}" ]]; then
      step_apply_vm_sizing
    fi
    return 0
  fi
  if [[ -n "$CP_DISK_GB" || -n "$WORKER_DISK_GB" ]]; then
    log "note: importing role-sized qcow2 (cp=${CP_DISK_GB:-default}G wk=${WORKER_DISK_GB:-default}G); EPHEMERAL grows on first boot"
  fi
  log "creating cluster VMs (cp=${CP_MEMORY}MB/${CP_CORES}c/${CP_DISK_GB:-img}G wk=${WORKER_MEMORY}MB/${WORKER_CORES}c/${WORKER_DISK_GB:-img}G)"
  CREATE_ARGS=(
    --no-lab-up
    --cp-vmid "$CP_VMID"
    --controlplanes "$CONTROLPLANES"
    --workers "$WORKERS"
    --prefix "$NAME_PREFIX"
    --disk "$DISK"
    --cp-disk "$CP_DISK"
    --worker-disk "$WORKER_DISK"
    --cp-memory "$CP_MEMORY"
    --cp-cores "$CP_CORES"
    --worker-memory "$WORKER_MEMORY"
    --worker-cores "$WORKER_CORES"
  )
  # Image already built at role size — do not pass --*-disk-gb (qm resize cannot shrink).
  if [[ "$DUAL_STACK" == "1" ]]; then
    export DUAL_STACK=1 PERTISK_DUAL_STACK=1
  fi
  PROXMOX_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
}

# Apply memory/cores/disk-gb to existing VMs (qm set + qm resize).
step_apply_vm_sizing() {
  : "${PROXMOX_NODE:?PROXMOX_NODE required for VM sizing}"
  log "applying VM sizing (cp=${CP_MEMORY}MB/${CP_CORES}c/${CP_DISK_GB:--}G wk=${WORKER_MEMORY}MB/${WORKER_CORES}c/${WORKER_DISK_GB:--}G)"
  local i vid
  for i in $(seq 1 "$CONTROLPLANES"); do
    vid=$((CP_VMID + i - 1))
    apply_one_vm_sizing "$vid" "$CP_MEMORY" "$CP_CORES" "$CP_DISK_GB"
  done
  for i in $(seq 1 "$WORKERS"); do
    vid=$((CP_VMID + CONTROLPLANES + i - 1))
    apply_one_vm_sizing "$vid" "$WORKER_MEMORY" "$WORKER_CORES" "$WORKER_DISK_GB"
  done
}

apply_one_vm_sizing() {
  local vmid="$1" mem="$2" cores="$3" disk_gb="${4:-}"
  log "size VM ${vmid}: memory=${mem} cores=${cores} disk-gb=${disk_gb:-unchanged}"
  if [[ -z "${PROXMOX_SSH:-}" ]]; then
    die "PROXMOX_SSH required to resize existing VM disks (e.g. PROXMOX_SSH=root@pve)"
  fi
  ssh -o StrictHostKeyChecking=accept-new "${PROXMOX_SSH}" \
    "qm set ${vmid} --memory ${mem} --cores ${cores}"
  if [[ -n "$disk_gb" ]]; then
    ssh -o StrictHostKeyChecking=accept-new "${PROXMOX_SSH}" bash -s <<EOF
set -euo pipefail
VMID=${vmid}
GB=${disk_gb}
qm stop "\$VMID" >/dev/null 2>&1 || true
for i in \$(seq 1 45); do
  qm status "\$VMID" 2>/dev/null | grep -q stopped && break
  sleep 1
done
# qm resize is grow-only. Shrinking is rejected; recreate VMs from a sized qcow2 instead.
if ! qm resize "\$VMID" scsi0 "\${GB}G"; then
  echo "WARN: qm resize \${VMID} scsi0 → \${GB}G failed (cannot shrink). Recreate without --skip-build/--skip-vms." >&2
fi
qm config "\$VMID" | grep -E '^(memory|cores|scsi0):' || true
qm start "\$VMID" >/dev/null 2>&1 || true
EOF
  fi
}

step_resolve_ips() {
  CP_IPS=()
  local i cvid
  for i in $(seq 1 "$CONTROLPLANES"); do
    cvid=$((CP_VMID + i - 1))
    CP_IPS+=("$(wait_ip "$cvid" "${CLUSTER_NAME}-cp-${i}")")
  done
  CP_IP="${CP_IPS[0]}"
  WORKER_IPS=()
  local wvid
  for i in $(seq 1 "$WORKERS"); do
    wvid=$((CP_VMID + CONTROLPLANES + i - 1))
    WORKER_IPS+=("$(wait_ip "$wvid" "${CLUSTER_NAME}-wk-${i}")")
  done
  if [[ "$CONTROLPLANES" -gt 1 ]]; then
    API_ENDPOINT="$VIP"
  else
    API_ENDPOINT="$CP_IP"
  fi
  log "CPs=${CP_IPS[*]} VIP=${VIP:-none} API_ENDPOINT=${API_ENDPOINT} workers=${WORKER_IPS[*]:-}"
}

step_cluster() {
  make -C "$ROOT" pertiskctl
  [[ -x "$CTL" ]] || die "pertiskctl missing"

  mkdir -p "$CLUSTER_OUT"
  log "gen config ${CLUSTER_NAME} https://${API_ENDPOINT}:6443 (controlplanes=${CONTROLPLANES} dual-stack=${DUAL_STACK})"
  local gen_args=(
    gen config "$CLUSTER_NAME" "https://${API_ENDPOINT}:6443"
    -o "$CLUSTER_OUT" -k "$K8S_VER" --controlplanes "$CONTROLPLANES"
  )
  if [[ "$DUAL_STACK" == "1" ]]; then
    gen_args+=(--dual-stack)
    [[ -n "$VIP6" ]] && gen_args+=(--vip6 "$VIP6")
  fi
  "$CTL" "${gen_args[@]}"

  # Ensure CP1 hostname matches lab convention
  set_hostname_yaml "$CLUSTER_OUT/controlplane.yaml" "$CLUSTER_OUT/controlplane.yaml.tmp" "${CLUSTER_NAME}-cp-1"
  mv "$CLUSTER_OUT/controlplane.yaml.tmp" "$CLUSTER_OUT/controlplane.yaml"

  wait_api "$CP_IP"
  log "apply controlplane → ${CP_IP}"
  "$CTL" -e "${CP_IP}:50000" apply -f "$CLUSTER_OUT/controlplane.yaml"

  log "bootstrap CP1"
  "$CTL" -e "${CP_IP}:50000" bootstrap

  # Join additional control planes (stacked etcd + kube-vip).
  # Use C-style for: macOS `seq 2 1` counts down and would iterate wrongly.
  local i ip host cpyaml etcd_ep
  etcd_ep="https://${CP_IP}:2379"
  for ((i = 2; i <= CONTROLPLANES; i++)); do
    ip="${CP_IPS[$((i - 1))]}"
    host="${CLUSTER_NAME}-cp-${i}"
    cpyaml="${CLUSTER_OUT}/controlplane-${i}.yaml"
    log "get-join-config for ${host}"
    "$CTL" -e "${CP_IP}:50000" get-join-config \
      --controlplane --controlplane-index "$i" --cluster-name "$CLUSTER_NAME" \
      -o "$cpyaml"
    set_hostname_yaml "$cpyaml" "${cpyaml}.tmp" "$host"
    mv "${cpyaml}.tmp" "$cpyaml"
    wait_api "$ip"
    log "apply + join-controlplane ${host} @ ${ip}"
    "$CTL" -e "${ip}:50000" apply -f "$cpyaml"
    # apply reloads runtime; give Machine API a moment before the long join RPC
    sleep 5
    wait_api "$ip"
    log "waiting for CP1 etcd ${etcd_ep} (join retries inside agent)"
    local join_try=0
    until "$CTL" -e "${ip}:50000" join-controlplane --etcd-endpoints "$etcd_ep"; do
      join_try=$((join_try + 1))
      (( join_try < 5 )) || die "join-controlplane failed for ${host} after ${join_try} attempts"
      log "join-controlplane retry ${join_try}/5 for ${host}..."
      sleep 10
      wait_api "$ip"
    done
  done

  log "kubeconfig (endpoint https://${API_ENDPOINT}:6443)"
  "$CTL" -e "${CP_IP}:50000" kubeconfig -f "$CLUSTER_OUT/admin.conf"

  log "join-config (fill CA)"
  "$CTL" -e "${CP_IP}:50000" join-config -f "$CLUSTER_OUT/worker.yaml"

  # Wait for apiserver on a CP node IP first (VIP needs kube-vip leader election).
  wait_apiserver_ready "$CLUSTER_OUT/admin.conf" "$CP_IP" "$API_ENDPOINT"

  local wyaml
  for i in $(seq 1 "$WORKERS"); do
    ip="${WORKER_IPS[$((i - 1))]}"
    host="${CLUSTER_NAME}-wk-${i}"
    wyaml="${CLUSTER_OUT}/worker-${i}.yaml"
    set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$host"
    wait_api "$ip"
    log "join worker ${host} @ ${ip}"
    "$CTL" -e "${ip}:50000" apply -f "$wyaml"
  done
}

step_cni() {
  local kc="$CLUSTER_OUT/admin.conf"
  case "$CNI" in
    none)
      log "CNI=none — skip"
      ;;
    flannel)
      command -v kubectl >/dev/null || die "kubectl required"
      install_kube_proxy "$kc"
      log "install Flannel"
      kubectl --kubeconfig "$kc" apply -f "${ROOT}/examples/cni/kube-flannel.yaml"
      # Reach apiserver before ClusterIP works (kube-proxy may still be syncing).
      kubectl --kubeconfig "$kc" -n kube-flannel set env ds/kube-flannel-ds \
        KUBERNETES_SERVICE_HOST="${API_ENDPOINT}" \
        KUBERNETES_SERVICE_PORT=6443
      kubectl --kubeconfig "$kc" -n kube-flannel rollout status ds/kube-flannel-ds --timeout=5m 2>/dev/null \
        || echo "WARNING: flannel DS not Ready yet; check: kubectl --kubeconfig $kc -n kube-flannel get pods" >&2
      ;;
    calico)
      command -v kubectl >/dev/null || die "kubectl required"
      command -v curl >/dev/null || die "curl required for CNI=calico"
      install_kube_proxy "$kc"
      log "install Calico ${CALICO_VERSION} (VXLAN, pod CIDR 10.244.0.0/16)"
      curl -fsSL "https://raw.githubusercontent.com/projectcalico/calico/${CALICO_VERSION}/manifests/calico.yaml" \
        | kubectl --kubeconfig "$kc" apply -f -
      # Prefer VXLAN (linux-virt module path) over default IPIP; pin Pertisk pod CIDR.
      kubectl --kubeconfig "$kc" -n kube-system set env ds/calico-node \
        CALICO_IPV4POOL_CIDR=10.244.0.0/16 \
        CALICO_IPV4POOL_IPIP=Never \
        CALICO_IPV4POOL_VXLAN=Always \
        KUBERNETES_SERVICE_HOST="${API_ENDPOINT}" \
        KUBERNETES_SERVICE_PORT=6443
      kubectl --kubeconfig "$kc" -n kube-system rollout status ds/calico-node --timeout=5m 2>/dev/null \
        || echo "WARNING: calico-node not Ready yet; check: kubectl --kubeconfig $kc -n kube-system get pods -l k8s-app=calico-node" >&2
      ;;
    cilium)
      command -v helm >/dev/null || die "helm required for CNI=cilium"
      command -v kubectl >/dev/null || die "kubectl required"
      log "install Cilium (kubernetes IPAM + kubeProxyReplacement + Hubble; dual-stack=${DUAL_STACK})"
      helm repo add cilium https://helm.cilium.io/ >/dev/null 2>&1 || true
      helm repo update cilium >/dev/null 2>&1 || true
      export KUBECONFIG="$kc"
      local cilium_extra=()
      if [[ "$DUAL_STACK" == "1" ]]; then
        # Dual-stack on top of the known-good IPv4 helm set (Node.PodCIDR from
        # controller-manager --cluster-cidr=v4,v6).
        cilium_extra+=(
          --set ipv6.enabled=true
          --set enableIPv6Masquerade=true
          --set ipam.operator.clusterPoolIPv6MaskSize=112
        )
      else
        cilium_extra+=(--set ipv6.enabled=false --set enableIPv6Masquerade=false)
      fi
      # Known-good lab values (ipam.mode=kubernetes, Hubble relay hostNetwork).
      # Pertisk-only: bpf.autoMount=false (host already mounts bpffs).
      helm upgrade --install cilium cilium/cilium \
        --kubeconfig "$kc" \
        --namespace cilium --create-namespace \
        --set ipam.mode=kubernetes \
        --set kubeProxyReplacement=true \
        --set securityContext.capabilities.ciliumAgent="{CHOWN,KILL,NET_ADMIN,NET_RAW,IPC_LOCK,SYS_ADMIN,SYS_RESOURCE,DAC_OVERRIDE,FOWNER,SETGID,SETUID}" \
        --set securityContext.capabilities.cleanCiliumState="{NET_ADMIN,SYS_ADMIN,SYS_RESOURCE}" \
        --set cgroup.autoMount.enabled=false \
        --set cgroup.hostRoot=/sys/fs/cgroup \
        --set bpf.autoMount.enabled=false \
        --set k8sServiceHost="${API_ENDPOINT}" \
        --set k8sServicePort=6443 \
        --set l2announcements.enabled=true \
        --set bpf.masquerade=true \
        --set hubble.enabled=true \
        --set hubble.relay.enabled=true \
        --set hubble.ui.enabled=true \
        --set prometheus.enabled=true \
        --set ipam.operator.clusterPoolIPv4MaskSize=24 \
        --set hubble.relay.hostNetwork=true \
        --set hubble.relay.dnsPolicy=ClusterFirstWithHostNet \
        "${cilium_extra[@]}" \
        --timeout 10m || {
          echo "WARNING: helm install reported failure; continuing to netns / iptables patches" >&2
        }
      # Cilium defaults to hostPath /var/run/netns. EPHEMERAL /var is not shared
      # until pertiskd binds /run over /var/run — use /run/netns (already rshared).
      patch_cilium_netns_to_run "$kc"
      # Cilium image defaults `iptables` → nft; Pertisk host uses iptables-legacy.
      # Force legacy binaries before cilium-agent starts (L7/Hubble still needs iptables).
      patch_cilium_iptables_legacy "$kc"
      # Best-effort wait after patch (pods recreate).
      kubectl --kubeconfig "$kc" -n cilium rollout status ds/cilium --timeout=5m 2>/dev/null \
        || echo "WARNING: cilium DS not Ready yet; check: kubectl --kubeconfig $kc -n cilium get pods" >&2
      ;;
    *)
      die "unknown CNI=$CNI (use cilium|calico|flannel|none)"
      ;;
  esac
}

# kube-proxy for Flannel/Calico (Cilium uses kubeProxyReplacement instead).
install_kube_proxy() {
  local kc="$1"
  local src="${ROOT}/examples/cni/kube-proxy.yaml"
  [[ -f "$src" ]] || die "missing $src"
  log "install kube-proxy (apiserver ${API_ENDPOINT}:6443)"
  # Drop any leftover Cilium agent-not-ready taints if switching CNIs.
  kubectl --kubeconfig "$kc" taint nodes --all node.cilium.io/agent-not-ready- 2>/dev/null || true
  sed "s/__KUBERNETES_SERVICE_HOST__/${API_ENDPOINT}/g" "$src" \
    | sed "s|registry.k8s.io/kube-proxy:v1.36.3|registry.k8s.io/kube-proxy:${K8S_VER}|g" \
    | kubectl --kubeconfig "$kc" apply -f -
  kubectl --kubeconfig "$kc" -n kube-system rollout status ds/kube-proxy --timeout=3m 2>/dev/null \
    || echo "WARNING: kube-proxy not Ready yet" >&2
}

# Point Cilium's cilium-netns volume at /run/netns (shared) instead of /var/run/netns.
patch_cilium_netns_to_run() {
  local kc="$1"
  log "patch Cilium cilium-netns hostPath → /run/netns"
  if ! python3 - "$kc" <<'PY'
import json, subprocess, sys, time
kc = sys.argv[1]
d = None
for _ in range(30):
    try:
        d = json.loads(subprocess.check_output(
            ["kubectl", "--kubeconfig", kc, "-n", "cilium", "get", "ds", "cilium", "-o", "json"],
            stderr=subprocess.DEVNULL,
        ))
        break
    except Exception:
        time.sleep(2)
if not d:
    raise SystemExit("cilium DaemonSet not found")
vols = d["spec"]["template"]["spec"]["volumes"]
idx = next(i for i, v in enumerate(vols) if v.get("name") == "cilium-netns")
cur = vols[idx].get("hostPath", {}).get("path")
if cur == "/run/netns":
    print("cilium-netns already /run/netns", file=sys.stderr)
    raise SystemExit(0)
patch = [{"op": "replace", "path": f"/spec/template/spec/volumes/{idx}/hostPath/path", "value": "/run/netns"}]
subprocess.check_call(
    ["kubectl", "--kubeconfig", kc, "-n", "cilium", "patch", "ds", "cilium", "--type=json", "-p", json.dumps(patch)]
)
print(f"patched cilium-netns {cur} → /run/netns", file=sys.stderr)
PY
  then
    echo "WARNING: netns patch failed — expect CreateContainerError on /var/run/netns" >&2
  fi
}

# Cilium's image links `iptables` → nft; Pertisk hosts use iptables-legacy tables.
# Wrap cilium-agent so each start retargets iptables* to xtables-legacy-multi.
patch_cilium_iptables_legacy() {
  local kc="$1"
  log "patch Cilium agent entrypoint → iptables-legacy"
  if ! python3 - "$kc" <<'PY'
import json, subprocess, sys, time
kc = sys.argv[1]
d = None
for _ in range(30):
    try:
        d = json.loads(subprocess.check_output(
            ["kubectl", "--kubeconfig", kc, "-n", "cilium", "get", "ds", "cilium", "-o", "json"],
            stderr=subprocess.DEVNULL,
        ))
        break
    except Exception:
        time.sleep(2)
if not d:
    raise SystemExit("cilium DaemonSet not found")
idx = next(i for i, c in enumerate(d["spec"]["template"]["spec"]["containers"]) if c["name"] == "cilium-agent")
c = d["spec"]["template"]["spec"]["containers"][idx]
args = c.get("args") or ["--config-dir=/tmp/cilium/config-map"]
# Drop a previous wrap so re-runs stay idempotent.
if args and "xtables-legacy-multi" in args[0]:
    # unwrap: ["wrap script", "--", ...real args]
    if len(args) >= 2 and args[1] == "--":
        args = args[2:]
    else:
        args = ["--config-dir=/tmp/cilium/config-map"]
wrap = (
    'set -e; for p in /usr/sbin /sbin; do '
    'ln -sfn xtables-legacy-multi $p/iptables; '
    'ln -sfn xtables-legacy-multi $p/iptables-restore; '
    'ln -sfn xtables-legacy-multi $p/iptables-save; '
    'ln -sfn xtables-legacy-multi $p/ip6tables; '
    'ln -sfn xtables-legacy-multi $p/ip6tables-restore; '
    'ln -sfn xtables-legacy-multi $p/ip6tables-save; '
    'done; exec cilium-agent "$@"'
)
new_args = [wrap, "--", *args]
ops = [
    {"op": "add" if "command" not in c else "replace",
     "path": f"/spec/template/spec/containers/{idx}/command",
     "value": ["/bin/sh", "-c"]},
    {"op": "replace",
     "path": f"/spec/template/spec/containers/{idx}/args",
     "value": new_args},
]
if "command" not in c:
    ops[0]["op"] = "add"
subprocess.check_call(
    ["kubectl", "--kubeconfig", kc, "-n", "cilium", "patch", "ds", "cilium", "--type=json", "-p", json.dumps(ops)]
)
# Quiet ipv6 masquerade noise when ipv6 is disabled (chart default can leave it true).
subprocess.run(
    ["kubectl", "--kubeconfig", kc, "-n", "cilium", "patch", "cm", "cilium-config", "--type=merge",
     "-p", '{"data":{"enable-ipv6-masquerade":"false"}}'],
    check=False,
)
print("patched cilium-agent iptables-legacy wrapper", file=sys.stderr)
PY
  then
    echo "WARNING: iptables-legacy patch failed — expect nft POSTROUTING / Extension errors" >&2
  fi
}

step_dns() {
  local kc="$CLUSTER_OUT/admin.conf"
  command -v kubectl >/dev/null || die "kubectl required"
  log "ensure CoreDNS (kube-dns 10.96.0.10) — also applied by bootstrap finalize"
  kubectl --kubeconfig "$kc" apply -f "${ROOT}/examples/dns/coredns.yaml"
}

step_addons() {
  local kc="$CLUSTER_OUT/admin.conf"
  command -v kubectl >/dev/null || die "kubectl required"
  # Always ensure basic addons (bootstrap finalize also embeds these).
  log "ensure metrics-server (basic addon)"
  kubectl --kubeconfig "$kc" apply -f "$METRICS_SERVER_YAML"
  if [[ "$SKIP_ADDONS" == "1" ]]; then
    log "skip-addons — skip optional reflector"
    return 0
  fi
  log "install kubernetes-reflector (optional lab addon)"
  kubectl --kubeconfig "$kc" apply -f "$REFLECTOR_YAML"
}

step_apps() {
  local kc="$CLUSTER_OUT/admin.conf" app
  [[ -n "${APPS:-}" ]] || { log "no APPS set — skip"; return 0; }
  command -v kubectl >/dev/null || die "kubectl required"
  # shellcheck disable=SC2086
  for app in ${APPS//,/ }; do
    [[ -z "$app" ]] && continue
    if [[ -f "$app" ]]; then
      log "kubectl apply -f ${app}"
      kubectl --kubeconfig "$kc" apply -f "$app"
    elif [[ -f "${ROOT}/${app}" ]]; then
      log "kubectl apply -f ${ROOT}/${app}"
      kubectl --kubeconfig "$kc" apply -f "${ROOT}/${app}"
    else
      die "app manifest not found: ${app}"
    fi
  done
}

step_summary() {
  local kc="$CLUSTER_OUT/admin.conf"
  cat <<EOF

======== lab-up complete ========
CPs:     ${CP_IPS[*]:-$CP_IP}  (Machine API :50000)
API:     https://${API_ENDPOINT}:6443${VIP:+ (kube-vip ${VIP})}
Workers: ${WORKER_IPS[*]:-}
kubeconfig: ${kc}
CNI: ${CNI}
dual-stack: ${DUAL_STACK}
controlplanes: ${CONTROLPLANES}

  kubectl --kubeconfig ${kc} get nodes -o wide
  kubectl --kubeconfig ${kc} get pods -A
EOF
  if command -v kubectl >/dev/null; then
    kubectl --kubeconfig "$kc" get nodes -o wide || true
  fi
}

# --- run ---
log "lab-up cluster=${CLUSTER_NAME} cp-vmid=${CP_VMID} controlplanes=${CONTROLPLANES} workers=${WORKERS} cni=${CNI} vip=${VIP:-none}"
step_build
step_vms
step_resolve_ips
step_cluster
step_cni
step_dns
step_addons
step_apps
step_summary
