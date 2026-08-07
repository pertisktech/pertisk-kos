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

ROOT="${PERTISK_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
PROVIDER_KIND="${PROVIDER_KIND:-proxmox}"
if [[ "$PROVIDER_KIND" == "vsphere" ]]; then
  CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/vsphere-create-cluster-vms.sh}"
else
  CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/proxmox-create-cluster-vms.sh}"
fi
# Prefer explicit binary from mgmt (RPM: /usr/bin/pertiskctl).
if [[ -n "${PERTISKCTL:-}" && -x "${PERTISKCTL}" ]]; then
  CTL="${PERTISKCTL}"
elif [[ -x "${ROOT}/out/bin/pertiskctl" ]]; then
  CTL="${ROOT}/out/bin/pertiskctl"
elif command -v pertiskctl >/dev/null 2>&1; then
  CTL="$(command -v pertiskctl)"
else
  CTL="${ROOT}/out/bin/pertiskctl"
fi
CLUSTER_OUT="${CLUSTER_OUT:-${ROOT}/out/cluster}"

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
if [[ -n "${NAME_PREFIX:-}" ]]; then
  PREFIX_SET=1
else
  PREFIX_SET=0
  NAME_PREFIX=pertisk
fi
CLUSTER_NAME="${CLUSTER_NAME:-lab-ha}"
MAX_PODS="${MAX_PODS:-}"
K8S_VER="${K8S_VER:-v1.36.3}"
CNI="${CNI:-cilium}"          # cilium | calico | flannel | none
CALICO_VERSION="${CALICO_VERSION:-v3.29.3}"
# Guest arch: ARCH / PERTISK_ARCH (amd64|arm64). Default amd64.
ARCH="${PERTISK_ARCH:-${ARCH:-amd64}}"
case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported ARCH=${ARCH} (use amd64|arm64)" >&2; exit 1 ;;
esac
export PERTISK_ARCH="$ARCH"
# Cloud images: RPM puts qcow2 under /var/lib/pertisk-mgmt/images; local builds use $ROOT/out.
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
DISK="${PROXMOX_DISK:-}"
DISK_FROM_CLI=0
if [[ -z "$DISK" ]]; then
  for _cand in \
    "${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2" \
    "${ROOT}/out/pertisk-cloud-${ARCH}.qcow2"; do
    if [[ -f "$_cand" ]]; then
      DISK="$_cand"
      break
    fi
  done
  DISK="${DISK:-${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2}"
fi
unset _img_d _cand
SKIP_BUILD=0
SKIP_VMS=0
SKIP_ADDONS=0
IP_TIMEOUT="${IP_TIMEOUT:-300}"
# Extra budget after DHCP/ARP for first-boot Machine API (disk expand can be slow).
API_AFTER_IP_TIMEOUT="${API_AFTER_IP_TIMEOUT:-900}"
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
  --arch amd64|arm64  guest arch (default ${ARCH}; env ARCH/PERTISK_ARCH)
  --cni NAME          cilium|calico|flannel|none (default ${CNI})
  --k8s VER           kubernetesVersion for gen config (default ${K8S_VER})
  --max-pods N        kubelet maxPods (machine.kubelet.extraConfig.maxPods)
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
     ARCH / PERTISK_ARCH (amd64|arm64; arm64 → machine=virt + AAVMF)
     PROXMOX_MEMORY / PROXMOX_CORES (defaults for both roles)
     PROXMOX_CP_MEMORY / PROXMOX_CP_CORES / PROXMOX_WORKER_MEMORY / PROXMOX_WORKER_CORES
     PROXMOX_CP_DISK_GB / PROXMOX_WORKER_DISK_GB / PERTISK_DISK_GB
     PERTISK_IMAGES_DIR / PROXMOX_IMAGES_DIR (default: /var/lib/pertisk-mgmt/images or \$ROOT/out)
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
    --prefix) NAME_PREFIX="$2"; PREFIX_SET=1; shift 2 ;;
    --cluster) CLUSTER_NAME="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --cni) CNI="$2"; shift 2 ;;
    --k8s) K8S_VER="$2"; shift 2 ;;
    --max-pods) MAX_PODS="$2"; shift 2 ;;
    --disk) DISK="$2"; DISK_FROM_CLI=1; shift 2 ;;
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

case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported --arch=${ARCH} (use amd64|arm64)" >&2; exit 1 ;;
esac
export PERTISK_ARCH="$ARCH" ARCH="$ARCH"

# Re-resolve default disk if --arch changed and --disk / PROXMOX_DISK were not set.
if [[ "$DISK_FROM_CLI" != "1" && -z "${PROXMOX_DISK:-}" ]]; then
  DISK=""
  for _cand in \
    "${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2" \
    "${ROOT}/out/pertisk-cloud-${ARCH}.qcow2"; do
    if [[ -f "$_cand" ]]; then
      DISK="$_cand"
      break
    fi
  done
  DISK="${DISK:-${IMAGES_DIR}/pertisk-cloud-${ARCH}.qcow2}"
  unset _cand
fi

# Proxmox VM names follow cluster name unless --prefix was set explicitly.
if [[ "$PREFIX_SET" -eq 0 ]]; then
  NAME_PREFIX="$CLUSTER_NAME"
fi

CP_MEMORY="${CP_MEMORY:-$MEMORY}"
CP_CORES="${CP_CORES:-$CORES}"
WORKER_MEMORY="${WORKER_MEMORY:-$MEMORY}"
WORKER_CORES="${WORKER_CORES:-$CORES}"
CP_DISK_GB="${CP_DISK_GB:-$DISK_GB}"
WORKER_DISK_GB="${WORKER_DISK_GB:-$DISK_GB}"

# Fast build: populate ~4G, then qemu-img resize per role. Guest grows GPT +
# EPHEMERAL on first boot. Distinct --*-disk-gb → separate sized qcow2s.
sized_qcow() {
  local gb="$1" cand
  for cand in \
    "${IMAGES_DIR}/pertisk-cloud-${ARCH}-${gb}g.qcow2" \
    "${ROOT}/out/pertisk-cloud-${ARCH}-${gb}g.qcow2"; do
    if [[ -f "$cand" ]]; then
      echo "$cand"
      return 0
    fi
  done
  # Preferred write/read path when building or when image is not present yet.
  echo "${IMAGES_DIR}/pertisk-cloud-${ARCH}-${gb}g.qcow2"
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
if [[ "$CONTROLPLANES" -gt 1 && -z "$VIP" && -z "$VIP6" ]]; then
  echo "ERROR: --vip and/or --vip6 is required when --controlplanes > 1" >&2
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

if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
  : "${VSPHERE_URL:?set VSPHERE_URL}"
  : "${VSPHERE_USER:?set VSPHERE_USER}"
  : "${VSPHERE_PASSWORD:?set VSPHERE_PASSWORD}"
  : "${VSPHERE_DATASTORE:?set VSPHERE_DATASTORE}"
  VSPHERE_NETWORK="${VSPHERE_NETWORK:-VM Network}"
  export VSPHERE_INSECURE="${VSPHERE_INSECURE:-1}"
  ESXI_HOST="$(echo "${VSPHERE_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
  if [[ -z "${LAB_SUBNET}" && "${ESXI_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
    LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
    echo "==> auto LAB_SUBNET=${LAB_SUBNET}"
  fi
  # Reuse DISK from VSPHERE_DISK if set.
  if [[ -n "${VSPHERE_DISK:-}" ]]; then
    DISK="${VSPHERE_DISK}"
  fi
  echo "==> provider=vsphere url=${VSPHERE_URL} datastore=${VSPHERE_DATASTORE} network=${VSPHERE_NETWORK}"
  echo "==> images dir=${IMAGES_DIR} disk=${DISK}"
  # Stub Proxmox vars so shared helpers that reference them don't explode.
  PROXMOX_URL="${PROXMOX_URL:-${VSPHERE_URL}}"
  PROXMOX_NODE="${PROXMOX_NODE:-esxi}"
  unset PROXMOX_SSH || true
else
if [[ -z "${PROXMOX_URL:-}" ]]; then
  echo "==> loading Proxmox env from ${ROOT}/proxmox.sh"
  load_proxmox_sh "${ROOT}/proxmox.sh"
fi
: "${PROXMOX_URL:?set PROXMOX_URL (or proxmox.sh)}"
: "${PROXMOX_TOKEN_ID:?set PROXMOX_TOKEN_ID}"
: "${PROXMOX_TOKEN_SECRET:?set PROXMOX_TOKEN_SECRET}"
: "${PROXMOX_NODE:?set PROXMOX_NODE}"

# Derive lab subnet from PROXMOX_URL. Disk import defaults to Proxmox API
# (Omni-style — provider token only). Set PROXMOX_SSH=root@host to use scp+qm.
# PROXMOX_NO_SSH=1 clears SSH; PROXMOX_SSH_AUTO=1 restores old auto root@<ip>.
PVE_HOST="$(echo "${PROXMOX_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
if [[ "${PROXMOX_NO_SSH:-0}" == "1" ]]; then
  unset PROXMOX_SSH || true
elif [[ -z "${PROXMOX_SSH:-}" && "${PROXMOX_SSH_AUTO:-0}" == "1" && "${PVE_HOST}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  export PROXMOX_SSH="root@${PVE_HOST}"
  echo "==> auto PROXMOX_SSH=${PROXMOX_SSH} (PROXMOX_SSH_AUTO=1)"
elif [[ -z "${PROXMOX_SSH:-}" && "${PROXMOX_SSH_AUTO:-0}" == "1" && -n "${PVE_HOST}" ]]; then
  if ssh -o BatchMode=yes -o ConnectTimeout=3 -o StrictHostKeyChecking=accept-new \
    "root@${PVE_HOST}" true >/dev/null 2>&1; then
    export PROXMOX_SSH="root@${PVE_HOST}"
    echo "==> auto PROXMOX_SSH=${PROXMOX_SSH} (PROXMOX_SSH_AUTO=1)"
  fi
fi
if [[ -z "${LAB_SUBNET}" && "${PVE_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
  LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
  echo "==> auto LAB_SUBNET=${LAB_SUBNET}"
fi

if [[ -z "${PROXMOX_SSH:-}" ]]; then
  # ZFS/LVM cannot hold content=import; upload via directory storage then import-from.
  if [[ -z "${PROXMOX_UPLOAD_STORAGE:-}" ]]; then
    case "${PROXMOX_STORAGE:-}" in
      *zfs*|*lvm*|local-lvm) export PROXMOX_UPLOAD_STORAGE=local ;;
      "") export PROXMOX_UPLOAD_STORAGE=local ;;
      *) export PROXMOX_UPLOAD_STORAGE="${PROXMOX_STORAGE}" ;;
    esac
  fi
  echo "==> disk import via Proxmox API (upload→${PROXMOX_UPLOAD_STORAGE}; no SSH)"
  echo "    set PROXMOX_SSH=root@${PVE_HOST:-<pve>} for scp+qm instead"
else
  echo "==> disk import via SSH ${PROXMOX_SSH}"
fi
echo "==> images dir=${IMAGES_DIR} disk=${DISK}"
fi # PROVIDER_KIND != vsphere

command -v curl >/dev/null || { echo "curl required" >&2; exit 1; }
if [[ "${PROVIDER_KIND}" != "vsphere" ]]; then
  command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
fi
command -v python3 >/dev/null || { echo "python3 required" >&2; exit 1; }

CURL=(curl -sS)
if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
  [[ "${VSPHERE_INSECURE:-0}" == "1" ]] && CURL+=(-k)
  AUTH=""
  BASE=""
  NODE="esxi"
else
  [[ "${PROXMOX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
  AUTH="Authorization: PVEAPIToken=${PROXMOX_TOKEN_ID}=${PROXMOX_TOKEN_SECRET}"
  BASE="${PROXMOX_URL%/}/api2/json"
  NODE="${PROXMOX_NODE}"
fi

api_get() {
  "${CURL[@]}" -H "${AUTH}" "${BASE}$1"
}

log() { printf '==> %s\n' "$*" >&2; }
die() { echo "error: $*" >&2; exit 1; }

# --- MAC / IP helpers ---
# $1 = vmid, $2 = optional guest/VM name (e.g. lab-cp-1) for vsphere inventory lookup
vm_mac() {
  local vmid="$1" name="${2:-}" mac=""
  if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
    if [[ -n "$name" ]]; then
      mac="$(vsphere_vm_mac "$name" 2>/dev/null || true)"
    fi
    # Legacy create used {prefix}-{vmid}; keep lookup for older VMs.
    if [[ -z "$mac" ]]; then
      mac="$(vsphere_vm_mac "${NAME_PREFIX}-${vmid}" 2>/dev/null || true)"
    fi
    [[ -n "$mac" ]] || die "VM ${name:-$vmid}: no MAC yet; power on once so ESXi assigns one"
    echo "$mac" | tr 'A-F' 'a-f'
    return 0
  fi
  local net0
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

vsphere_vm_mac() {
  local name="$1" jar sdk base resp
  base="${VSPHERE_URL%/}"
  sdk="${base}/sdk"
  jar="$(mktemp)"
  local curl_args=(curl -sS -k -b "$jar" -c "$jar")
  "${curl_args[@]}" -X POST "$sdk" \
    -H 'Content-Type: text/xml; charset=UTF-8' \
    -H 'SOAPAction: urn:vim25/8.0.3.0' \
    --data-binary @- >/dev/null <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body>
    <Login xmlns="urn:vim25">
      <_this type="SessionManager">ha-sessionmgr</_this>
      <userName>${VSPHERE_USER}</userName>
      <password>${VSPHERE_PASSWORD}</password>
    </Login>
  </soapenv:Body>
</soapenv:Envelope>
EOF
  resp="$("${curl_args[@]}" -X POST "$sdk" \
    -H 'Content-Type: text/xml; charset=UTF-8' \
    -H 'SOAPAction: urn:vim25/8.0.3.0' \
    --data-binary @- <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
  <soapenv:Body>
    <RetrievePropertiesEx xmlns="urn:vim25">
      <_this type="PropertyCollector">ha-property-collector</_this>
      <specSet>
        <propSet>
          <type>VirtualMachine</type>
          <all>false</all>
          <pathSet>name</pathSet>
          <pathSet>config.hardware.device</pathSet>
        </propSet>
        <objectSet>
          <obj type="Folder">ha-folder-vm</obj>
          <skip>false</skip>
          <selectSet xsi:type="TraversalSpec" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
            <name>visitFolders</name>
            <type>Folder</type>
            <path>childEntity</path>
            <skip>false</skip>
            <selectSet><name>visitFolders</name></selectSet>
          </selectSet>
        </objectSet>
      </specSet>
      <options></options>
    </RetrievePropertiesEx>
  </soapenv:Body>
</soapenv:Envelope>
EOF
)"
  rm -f "$jar"
  python3 -c "
import sys,re
xml=sys.stdin.read()
want=$(printf '%s' "$name" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')
for m in re.finditer(r'<obj[^>]*type=\"VirtualMachine\">([^<]+)</obj>(.*?)</objects>', xml, re.S):
    block=m.group(2)
    nm=re.search(r'<name>name</name>\s*<val[^>]*>([^<]*)</val>', block)
    if not nm or nm.group(1)!=want: continue
    mac=re.search(r'<macAddress>([^<]+)</macAddress>', block)
    if mac:
        print(mac.group(1).lower())
        break
" <<<"$resp"
}

arp_ip_for_mac() {
  local mac="$1" out="" mac_cmp
  mac="$(echo "$mac" | tr 'A-F' 'a-f')"
  mac_cmp="$(echo "$mac" | tr -d ':.-')"
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    out="$(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 -o BatchMode=yes "${PROXMOX_SSH}" \
      "ip -4 neigh show | awk 'BEGIN{IGNORECASE=1} \$0 ~ /${mac}/ {print \$1; exit}'" \
      2>/dev/null || true)"
  else
    # Prefer entries that have an lladdr (skip INCOMPLETE/FAILED).
    out="$(ip -4 neigh show 2>/dev/null | awk -v m="$mac" -v c="$mac_cmp" '
      BEGIN { IGNORECASE=1 }
      $0 ~ /lladdr/ {
        line=tolower($0)
        gsub(/:/, "", line); gsub(/-/, "", line); gsub(/\./, "", line)
        if (index(line, c) || tolower($0) ~ m) { print $1; exit }
      }' || true)"
    if [[ -z "$out" ]]; then
      out="$(ip -4 neigh show 2>/dev/null | awk -v m="$mac" 'BEGIN{IGNORECASE=1} $0 ~ m {print $1; exit}' || true)"
    fi
    if [[ -z "$out" ]] && command -v arp >/dev/null 2>&1; then
      out="$(arp -an 2>/dev/null | tr '[:upper:]' '[:lower:]' | grep -F "$mac" \
        | sed -n 's/.*(\([0-9.]*\)).*/\1/p' | head -1 || true)"
    fi
  fi
  if [[ "$out" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$out"
  fi
}

# Populate ARP for LAB_SUBNET so MAC→IP works without PROXMOX_SSH (mgmt on same L2).
nudge_arp_subnet() {
  local cidr="$1" base
  [[ -n "$cidr" ]] || return 0
  base="${cidr%/*}"
  base="${base%.*}" # 10.1.1
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    log "nudge ARP on ${PROXMOX_SSH} for ${base}.0/24"
    ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 -o BatchMode=yes "${PROXMOX_SSH}" \
      "base=${base}; for i in \$(seq 1 254); do ping -c1 -W1 \${base}.\$i >/dev/null 2>&1 & done; wait" \
      >/dev/null 2>&1 || true
  else
    log "nudge ARP locally for ${base}.0/24"
    (
      local i
      for i in $(seq 1 254); do
        ping -c1 -W1 "${base}.${i}" >/dev/null 2>&1 &
        if (( i % 80 == 0 )); then wait || true; fi
      done
      wait || true
    ) >/dev/null 2>&1 || true
  fi
}

# Parallel :50000 probe (≈few seconds). TCP connects populate ARP, then match MAC.
# Avoids the old sequential 254×2s scan that looked like a hang on the last VM.
scan_api_subnet_for_mac() {
  local mac="$1" cidr="$2" base i
  [[ -n "$cidr" ]] || return 0
  mac="$(echo "$mac" | tr 'A-F' 'a-f')"
  base="${cidr%/*}"
  base="${base%.*}"
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

ping_sweep_find() {
  local mac="$1" cidr="$2" ip
  [[ -n "$cidr" ]] || return 0
  nudge_arp_subnet "$cidr"
  ip="$(arp_ip_for_mac "$mac" || true)"
  if [[ -n "$ip" ]]; then
    echo "$ip"
    return 0
  fi
  scan_api_subnet_for_mac "$mac" "$cidr" || true
}

api_reachable() {
  local ip="$1"
  # Machine API is gRPC — must probe TCP, not HTTP (curl always fails).
  if command -v nc >/dev/null 2>&1; then
    nc -z -w 2 "$ip" 50000 >/dev/null 2>&1
  else
    timeout 2 bash -c "echo >/dev/tcp/${ip}/50000" 2>/dev/null
  fi
}

wait_ip() {
  local vmid="$1" label="$2" mac ip="" nudged=0 saw_ip=0 last_log=0
  local ip_deadline api_deadline=0 deadline
  mac="$(vm_mac "$vmid" "$label")"
  log "VM ${vmid} (${label}) MAC=${mac} — waiting for DHCP IP (timeout ${IP_TIMEOUT}s; +${API_AFTER_IP_TIMEOUT}s after ARP for :50000)"
  ip_deadline=$((SECONDS + IP_TIMEOUT))
  while true; do
    if (( saw_ip )); then
      deadline=$api_deadline
    else
      deadline=$ip_deadline
    fi
    (( SECONDS < deadline )) || break

    ip="$(arp_ip_for_mac "$mac" || true)"
    # Only sweep when we still have no IP — never re-sweep while waiting on :50000.
    if [[ -z "$ip" && -n "$LAB_SUBNET" ]]; then
      if [[ "$nudged" == "0" ]] || (( SECONDS % 60 < 3 )); then
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
      if (( !saw_ip )); then
        saw_ip=1
        api_deadline=$((SECONDS + API_AFTER_IP_TIMEOUT))
        last_log=$SECONDS
        log "VM ${vmid} ARP=${ip} — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s)"
      elif (( SECONDS - last_log >= 20 )); then
        last_log=$SECONDS
        local left=$((api_deadline - SECONDS))
        (( left < 0 )) && left=0
        log "VM ${vmid} ARP=${ip} but :50000 not ready yet... (${left}s left)"
      fi
    else
      if (( SECONDS - last_log >= 15 )); then
        last_log=$SECONDS
        log "VM ${vmid} no ARP yet for ${mac}…"
      fi
    fi
    sleep 3
  done
  if (( saw_ip )); then
    die "timed out waiting for Machine API :50000 on ${ip:-?} (VM ${vmid} MAC=${mac}; ARP was up but guest services slow — try IP_TIMEOUT/API_AFTER_IP_TIMEOUT)"
  fi
  die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (PROXMOX_SSH=${PROXMOX_SSH:-unset} subnet=${LAB_SUBNET:-unset})
hint: without PROXMOX_SSH, mgmt must share L2 with guests (LAB_SUBNET ping-sweep).
      check: ip -4 neigh | grep -i ${mac}
      resume: lab-up --skip-build --skip-vms --cp-vmid ${CP_VMID} --controlplanes ${CONTROLPLANES} --workers ${WORKERS}"
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

# Ensure the bootstrap-token Secret exists (worker TLS bootstrap). CP1 finalize
# can race HA joins and leave workers as system:anonymous / Unauthorized.
ensure_bootstrap_token_secret() {
  local kc="$1" worker_yaml="$2"
  local token id secret
  token="$(awk '/^[[:space:]]*token:/{print $2; exit}' "$worker_yaml")"
  [[ -n "$token" && "$token" == *.* ]] || die "no cluster.token in ${worker_yaml}"
  id="${token%%.*}"
  secret="${token#*.}"
  if kubectl --kubeconfig "$kc" -n kube-system get "secret/bootstrap-token-${id}" >/dev/null 2>&1; then
    log "bootstrap-token Secret bootstrap-token-${id} present"
    return 0
  fi
  log "WARNING: bootstrap-token Secret missing — creating bootstrap-token-${id}"
  kubectl --kubeconfig "$kc" apply -f - <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: bootstrap-token-${id}
  namespace: kube-system
type: bootstrap.kubernetes.io/token
stringData:
  description: pertisk bootstrap token
  token-id: "${id}"
  token-secret: "${secret}"
  usage-bootstrap-authentication: "true"
  usage-bootstrap-signing: "true"
  auth-extra-groups: system:bootstrappers:kubeadm:default-node-token
EOF
}

# Ensure every control-plane has the role label (join finalize can miss CP3+).
ensure_control_plane_roles() {
  local kc="$1" n="$2" i node
  for ((i = 1; i <= n; i++)); do
    node="${CLUSTER_NAME}-cp-${i}"
    if ! kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; then
      log "WARNING: node ${node} not found yet — skip role ensure"
      continue
    fi
    if kubectl --kubeconfig "$kc" get node "$node" -o jsonpath='{.metadata.labels.node-role\.kubernetes\.io/control-plane}' 2>/dev/null | grep -q .; then
      continue
    fi
    log "WARNING: ${node} missing control-plane role — labeling + tainting"
    kubectl --kubeconfig "$kc" label node "$node" 'node-role.kubernetes.io/control-plane=' --overwrite
    kubectl --kubeconfig "$kc" taint node "$node" 'node-role.kubernetes.io/control-plane=:NoSchedule' --overwrite || true
  done
}

ensure_worker_roles() {
  local kc="$1" n="$2" i node
  for ((i = 1; i <= n; i++)); do
    node="${CLUSTER_NAME}-wk-${i}"
    kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1 || continue
    kubectl --kubeconfig "$kc" label node "$node" 'node-role.kubernetes.io/worker=' --overwrite >/dev/null
  done
}

wait_nodes_ready() {
  local kc="$1"
  shift
  local node deadline
  for node in "$@"; do
    log "waiting for node ${node} Ready"
    deadline=$((SECONDS + BOOTSTRAP_TIMEOUT))
    until kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1 \
      && [[ "$(kubectl --kubeconfig "$kc" get node "$node" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null)" == "True" ]]; do
      (( SECONDS < deadline )) || die "node ${node} not Ready within timeout (check bootstrap-token Secret / kubelet logs)"
      sleep 5
    done
    log "node ${node} Ready"
  done
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

# Ensure machine.dashboard.mgmt_url (Public URL) is set in generated YAML.
set_mgmt_url_yaml() {
  local src="$1" dest="$2" url="$3"
  python3 - "$src" "$dest" "$url" <<'PY'
import sys
src, dest, url = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    import yaml
except ImportError:
    # Minimal fallback: inject under machine.dashboard without PyYAML.
    text = open(src).read()
    if "mgmt_url:" in text or "mgmtUrl:" in text:
        import re
        text2, n = re.subn(
            r"(?m)^([ \t]*mgmt_url:[ \t]*).*$",
            r"\1" + url,
            text,
            count=1,
        )
        if n == 0:
            text2, n = re.subn(
                r"(?m)^([ \t]*mgmtUrl:[ \t]*).*$",
                r"\1" + url,
                text,
                count=1,
            )
        if n:
            open(dest, "w").write(text2)
            raise SystemExit(0)
        open(dest, "w").write(text)
        raise SystemExit(0)
    lines = text.splitlines(True)
    out = []
    i = 0
    injected = False
    while i < len(lines):
        line = lines[i]
        out.append(line)
        if (not injected) and line.startswith("  dashboard:"):
            # Insert mgmt_url as first child of dashboard.
            out.append(f"    mgmt_url: {url}\n")
            injected = True
        i += 1
    if not injected:
        # Insert dashboard block after machine:
        out2 = []
        for line in out:
            out2.append(line)
            if line.startswith("machine:"):
                out2.append("  dashboard:\n")
                out2.append(f"    mgmt_url: {url}\n")
                injected = True
        out = out2
    open(dest, "w").write("".join(out))
    raise SystemExit(0)

with open(src) as f:
    doc = yaml.safe_load(f)
machine = doc.setdefault("machine", {})
dash = machine.setdefault("dashboard", {})
if not isinstance(dash, dict):
    dash = {}
    machine["dashboard"] = dash
dash["mgmt_url"] = url
with open(dest, "w") as f:
    yaml.safe_dump(doc, f, default_flow_style=False, sort_keys=False)
PY
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
      [[ -f "$DISK" ]] || die "disk missing: $CP_DISK (and fallback $DISK)
Copy cloud qcow2 into ${IMAGES_DIR}/ (e.g. pertisk-cloud-${ARCH}.qcow2 or *-Ng.qcow2),
or set PROXMOX_DISK / PERTISK_IMAGES_DIR in /etc/pertisk-mgmt/pertisk-mgmt.env"
      log "warn: missing sized CP image $CP_DISK — using $DISK (will qm-resize scsi0 → ${CP_DISK_GB}G after import)"
      CP_DISK="$DISK"
    fi
    if [[ -n "$WORKER_DISK_GB" && ! -f "$WORKER_DISK" ]]; then
      [[ -f "$DISK" ]] || die "disk missing: $WORKER_DISK (and fallback $DISK)
Copy cloud qcow2 into ${IMAGES_DIR}/ (e.g. pertisk-cloud-${ARCH}.qcow2 or *-Ng.qcow2),
or set PROXMOX_DISK / PERTISK_IMAGES_DIR in /etc/pertisk-mgmt/pertisk-mgmt.env"
      log "warn: missing sized worker image $WORKER_DISK — using $DISK (will qm-resize scsi0 → ${WORKER_DISK_GB}G after import)"
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
    log "note: target disks cp=${CP_DISK_GB:-default}G wk=${WORKER_DISK_GB:-default}G (grow scsi0 after import when image is smaller)"
  fi
  log "creating cluster VMs (cp=${CP_MEMORY}MB/${CP_CORES}c/${CP_DISK_GB:-img}G wk=${WORKER_MEMORY}MB/${WORKER_CORES}c/${WORKER_DISK_GB:-img}G)"
  CREATE_ARGS=(
    --no-lab-up
    --cp-vmid "$CP_VMID"
    --controlplanes "$CONTROLPLANES"
    --workers "$WORKERS"
    --prefix "$NAME_PREFIX"
    --arch "$ARCH"
    --disk "$DISK"
    --cp-disk "$CP_DISK"
    --worker-disk "$WORKER_DISK"
    --cp-memory "$CP_MEMORY"
    --cp-cores "$CP_CORES"
    --worker-memory "$WORKER_MEMORY"
    --worker-cores "$WORKER_CORES"
  )
  # Always pass role disk sizes so upload-vm can `qm resize` when the qcow2 is
  # smaller (common with --skip-build + missing *-Ng.qcow2 falling back to base).
  # upload-vm skips resize when the image virtual size already matches.
  [[ -n "$CP_DISK_GB" ]] && CREATE_ARGS+=(--cp-disk-gb "$CP_DISK_GB")
  [[ -n "$WORKER_DISK_GB" ]] && CREATE_ARGS+=(--worker-disk-gb "$WORKER_DISK_GB")
  if [[ "$DUAL_STACK" == "1" ]]; then
    export DUAL_STACK=1 PERTISK_DUAL_STACK=1
  fi
  if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
    VSPHERE_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  else
    PROXMOX_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  fi
}

# Apply memory/cores/disk-gb to existing VMs (qm set + qm resize).
step_apply_vm_sizing() {
  if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
    log "skip Proxmox qm sizing on vsphere (recreate VMs with --cp-memory/--disk-gb instead)"
    return 0
  fi
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
    API_ENDPOINT="${VIP:-$VIP6}"
  else
    API_ENDPOINT="$CP_IP"
  fi
  log "CPs=${CP_IPS[*]} VIP=${VIP:-none} VIP6=${VIP6:-none} API_ENDPOINT=${API_ENDPOINT} workers=${WORKER_IPS[*]:-}"
}

ensure_pertiskctl() {
  # RPM / packaged: PERTISKCTL=/usr/bin/pertiskctl (set by pertisk-mgmt).
  # Dev tree: build with make when missing.
  if [[ -x "$CTL" ]]; then
    return 0
  fi
  if command -v pertiskctl >/dev/null 2>&1; then
    CTL="$(command -v pertiskctl)"
    return 0
  fi
  if [[ -x /usr/bin/pertiskctl ]]; then
    CTL=/usr/bin/pertiskctl
    return 0
  fi
  if command -v make >/dev/null 2>&1 && [[ -f "${ROOT}/Makefile" ]]; then
    log "build pertiskctl"
    make -C "$ROOT" pertiskctl
  fi
  [[ -x "$CTL" ]] || die "pertiskctl missing (set PERTISKCTL=/usr/bin/pertiskctl or install the RPM)"
}

step_cluster() {
  ensure_pertiskctl
  log "using pertiskctl=${CTL}"

  mkdir -p "$CLUSTER_OUT"
  log "gen config ${CLUSTER_NAME} https://${API_ENDPOINT}:6443 (controlplanes=${CONTROLPLANES} dual-stack=${DUAL_STACK})"
  local gen_args=(
    gen config "$CLUSTER_NAME" "https://${API_ENDPOINT}:6443"
    -o "$CLUSTER_OUT" -k "$K8S_VER" --controlplanes "$CONTROLPLANES"
  )
  if [[ -n "$MAX_PODS" ]]; then
    gen_args+=(--max-pods "$MAX_PODS")
  fi
  if [[ -n "${MGMT_PUBLIC_URL:-}" ]]; then
    if "$CTL" gen config --help 2>&1 | grep -q -- '--mgmt-url'; then
      gen_args+=(--mgmt-url "$MGMT_PUBLIC_URL")
    fi
    log "dashboard mgmt_url=${MGMT_PUBLIC_URL}"
  fi
  if [[ "$DUAL_STACK" == "1" ]]; then
    gen_args+=(--dual-stack)
    [[ -n "$VIP6" ]] && gen_args+=(--vip6 "$VIP6")
  fi
  "$CTL" "${gen_args[@]}"

  # Ensure CP1 hostname matches lab convention
  set_hostname_yaml "$CLUSTER_OUT/controlplane.yaml" "$CLUSTER_OUT/controlplane.yaml.tmp" "${CLUSTER_NAME}-cp-1"
  mv "$CLUSTER_OUT/controlplane.yaml.tmp" "$CLUSTER_OUT/controlplane.yaml"
  # Belt-and-suspenders for older pertiskctl without --mgmt-url.
  if [[ -n "${MGMT_PUBLIC_URL:-}" ]]; then
    for f in "$CLUSTER_OUT"/controlplane.yaml "$CLUSTER_OUT"/worker.yaml "$CLUSTER_OUT"/controlplane-*.yaml; do
      [[ -f "$f" ]] || continue
      set_mgmt_url_yaml "$f" "$f.tmp" "$MGMT_PUBLIC_URL"
      mv "$f.tmp" "$f"
    done
  fi

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
    if [[ -n "${MGMT_PUBLIC_URL:-}" ]]; then
      set_mgmt_url_yaml "$cpyaml" "${cpyaml}.tmp" "$MGMT_PUBLIC_URL"
      mv "${cpyaml}.tmp" "$cpyaml"
    fi
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
  if [[ -n "${MGMT_PUBLIC_URL:-}" ]]; then
    set_mgmt_url_yaml "$CLUSTER_OUT/worker.yaml" "$CLUSTER_OUT/worker.yaml.tmp" "$MGMT_PUBLIC_URL"
    mv "$CLUSTER_OUT/worker.yaml.tmp" "$CLUSTER_OUT/worker.yaml"
  fi

  # Wait for apiserver on a CP node IP first (VIP needs kube-vip leader election).
  wait_apiserver_ready "$CLUSTER_OUT/admin.conf" "$CP_IP" "$API_ENDPOINT"
  ensure_bootstrap_token_secret "$CLUSTER_OUT/admin.conf" "$CLUSTER_OUT/worker.yaml"
  ensure_control_plane_roles "$CLUSTER_OUT/admin.conf" "$CONTROLPLANES"

  local wyaml worker_hosts=()
  for i in $(seq 1 "$WORKERS"); do
    ip="${WORKER_IPS[$((i - 1))]}"
    host="${CLUSTER_NAME}-wk-${i}"
    wyaml="${CLUSTER_OUT}/worker-${i}.yaml"
    set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$host"
    wait_api "$ip"
    log "join worker ${host} @ ${ip}"
    "$CTL" -e "${ip}:50000" apply -f "$wyaml"
    worker_hosts+=("$host")
  done
  if ((${#worker_hosts[@]} > 0)); then
    wait_nodes_ready "$CLUSTER_OUT/admin.conf" "${worker_hosts[@]}"
    ensure_worker_roles "$CLUSTER_OUT/admin.conf" "$WORKERS"
  fi
  # Re-check after workers (late CP node registration can miss join-time label).
  ensure_control_plane_roles "$CLUSTER_OUT/admin.conf" "$CONTROLPLANES"
}

# If the kube-vip ARP VIP became unreachable (busy IP, missing af_packet, …),
# rewrite kubeconfig + API_ENDPOINT to a live CP so CNI/DNS still install.
ensure_api_endpoint_reachable() {
  local kc="$CLUSTER_OUT/admin.conf"
  local cp_ip="${CP_IP:-}"
  [[ -n "$cp_ip" ]] || return 0
  if curl -sk --connect-timeout 2 "https://${API_ENDPOINT}:6443/readyz" >/dev/null 2>&1; then
    return 0
  fi
  if [[ "$API_ENDPOINT" == "$cp_ip" ]]; then
    die "apiserver unreachable at ${API_ENDPOINT}:6443"
  fi
  log "WARNING: API endpoint ${API_ENDPOINT}:6443 unreachable — falling back kubeconfig to CP ${cp_ip}"
  log "         (pick a free --vip; ensure guest image has af_packet for kube-vip ARP)"
  rewrite_kubeconfig_server "$kc" "https://${cp_ip}:6443"
  API_ENDPOINT="$cp_ip"
  curl -sk --connect-timeout 2 "https://${API_ENDPOINT}:6443/readyz" >/dev/null 2>&1 \
    || die "apiserver still unreachable after fallback to ${cp_ip}:6443"
}

# Before HA create: refuse a VIP that already answers ICMP / :6443.
require_vip_free() {
  local vip="$1" label="${2:-VIP}"
  [[ -n "$vip" ]] || return 0
  log "checking ${label} ${vip} is free on the LAN"
  if ping -c 1 -W 1 "$vip" >/dev/null 2>&1 \
    || ping -c 1 -W 1 -6 "$vip" >/dev/null 2>&1; then
    die "${label} ${vip} already responds to ping — kube-vip cannot use a busy address. Pick a free VIP."
  fi
  # Bracket IPv6 for URL.
  local host="$vip"
  if [[ "$vip" == *:* ]]; then
    host="[${vip}]"
  fi
  if curl -sk --connect-timeout 1 "https://${host}:6443/readyz" >/dev/null 2>&1; then
    die "${label} ${vip}:6443 already serves an apiserver — pick a free VIP."
  fi
  log "${label} ${vip} looks free"
}

warn_if_vip_busy() {
  # Back-compat alias — create path uses require_vip_free (hard fail).
  require_vip_free "$@"
}

step_cni() {
  local kc="$CLUSTER_OUT/admin.conf"
  ensure_api_endpoint_reachable
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
  ensure_api_endpoint_reachable
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
# kubectl node VERSION is the kubelet baked into the cloud image. Machine-config
# kubernetesVersion alone cannot change it — rebuild when the pin differs.
image_k8s_ver() {
  local f="${ROOT}/out/runtime/versions.txt"
  [[ -f "$f" ]] || { echo ""; return 0; }
  sed -n 's/^K8S_VER=//p' "$f" | head -1 | tr -d '[:space:]'
}

requested_k8s="$K8S_VER"
embedded_k8s="$(image_k8s_ver)"
if [[ "$SKIP_BUILD" == "1" && -n "$embedded_k8s" && "$embedded_k8s" != "$requested_k8s" ]]; then
  log "kubelet in image is ${embedded_k8s} but --k8s=${requested_k8s} — rebuilding cloud image (fetch-runtime + make cloud)"
  SKIP_BUILD=0
elif [[ "$SKIP_BUILD" == "1" && -n "$embedded_k8s" ]]; then
  log "kubelet in image matches --k8s=${requested_k8s} (skip build)"
fi

# fetch-runtime / make cloud read K8S_VER from the environment.
export K8S_VER

log "lab-up cluster=${CLUSTER_NAME} cp-vmid=${CP_VMID} controlplanes=${CONTROLPLANES} workers=${WORKERS} cni=${CNI} k8s=${K8S_VER} vip=${VIP:-none}"
if [[ "$CONTROLPLANES" -gt 1 ]]; then
  require_vip_free "${VIP:-}" "IPv4 VIP"
  require_vip_free "${VIP6:-}" "IPv6 VIP"
fi
step_build
step_vms
step_resolve_ips
step_cluster
step_cni
step_dns
step_addons
step_apps
step_summary
