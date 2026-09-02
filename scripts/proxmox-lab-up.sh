#!/usr/bin/env bash
# End-to-end lab: build image → create Proxmox VMs → wait for DHCP IPs (MAC→ARP)
# → bootstrap control-plane → join workers → install CNI → workers Ready → DNS + addons (+ optional apps).
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
#     (skips soft-reset when CP1 :6443/readyz is already up — do not wipe a live join)
#   CNI=flannel APPS="examples/apps/nginx.yaml" ./scripts/proxmox-lab-up.sh --skip-build
#   ./scripts/proxmox-lab-up.sh --skip-addons   # skip optional reflector (CoreDNS + metrics-server always)
set -euo pipefail

ROOT="${PERTISK_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
PROVIDER_KIND="${PROVIDER_KIND:-proxmox}"
case "$PROVIDER_KIND" in
  vsphere)
    CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/vsphere-create-cluster-vms.sh}"
    ;;
  nutanix)
    CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/nutanix-create-cluster-vms.sh}"
    ;;
  pertisk-vms)
    CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/pertisk-vms-create-cluster-vms.sh}"
    ;;
  *)
    CREATE_VMS="${CREATE_VMS:-${ROOT}/scripts/proxmox-create-cluster-vms.sh}"
    ;;
esac
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
CP_IPS=()
WORKER_IPS=()
DUAL_STACK="${DUAL_STACK:-0}"
WORKERS="${WORKERS:-2}"
# Static IPs (Proxmox only; no DHCP, stable across reboot/shutdown).
STATIC_BASE="${STATIC_BASE:-${PROXMOX_STATIC_BASE:-}}"
STATIC_SUBNET="${STATIC_SUBNET:-${PROXMOX_STATIC_SUBNET:-}}"
STATIC_GATEWAY="${STATIC_GATEWAY:-${PROXMOX_STATIC_GATEWAY:-}}"
STATIC_NAMESERVER="${STATIC_NAMESERVER:-${PROXMOX_STATIC_NAMESERVER:-}}"
STATIC_EXCLUDE="${STATIC_EXCLUDE:-${PROXMOX_STATIC_EXCLUDE:-}}"
# Auto-detected static IPs from mgmt (space-separated list): build VMID→IP map.
STATIC_IPS_ENV="${PROXMOX_STATIC_IPS:-}"
declare -A STATIC_IPS_MAP=()
if [[ -n "${NAME_PREFIX:-}" ]]; then
  PREFIX_SET=1
else
  PREFIX_SET=0
  NAME_PREFIX=pertisk
fi
CLUSTER_NAME="${CLUSTER_NAME:-lab-ha}"
MAX_PODS="${MAX_PODS:-}"
POD_SUBNET="${POD_SUBNET:-10.244.0.0/16}"
SERVICE_SUBNET="${SERVICE_SUBNET:-10.96.0.0/12}"
POD_SUBNET_IPV6="${POD_SUBNET_IPV6:-2001:db8:10:0::/56}"
SERVICE_SUBNET_IPV6="${SERVICE_SUBNET_IPV6:-2001:db8:96:1::/112}"
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
BOOTSTRAP_TIMEOUT="${BOOTSTRAP_TIMEOUT:-600}"
# Join finalize talks to local apiserver; TCP :6443 can RST HTTP for minutes after etcd member add.
JOIN_TRIES="${JOIN_TRIES:-15}"
JOIN_READYZ_WAIT="${JOIN_READYZ_WAIT:-720}"
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
  --pod-subnet CIDR   IPv4 pod CIDR (default ${POD_SUBNET})
  --service-subnet CIDR  IPv4 service CIDR (default ${SERVICE_SUBNET})
  --pod-subnet-ipv6 CIDR IPv6 pod CIDR when --dual-stack (default ${POD_SUBNET_IPV6})
  --service-subnet-ipv6 CIDR IPv6 service CIDR when --dual-stack (default ${SERVICE_SUBNET_IPV6})
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

Static IPs (Proxmox only; no DHCP, stable across reboot/shutdown):
  --static-base IP/PREFIX  cp-1 address (e.g. 10.1.1.120/24); each later node
                           gets +1 (env STATIC_BASE / PROXMOX_STATIC_BASE)
  --static-subnet CIDR    scan this subnet (e.g. 10.1.1.0/24) for free addresses
                           instead of a manual base (env STATIC_SUBNET / PROXMOX_STATIC_SUBNET)
  --static-gateway IP      required with --static-base/--static-subnet
  --static-nameserver IP   default: gateway
  --static-exclude IP[,IP...]  always skip these IPs (e.g. a Nutanix CVM),
                           even if they don't answer ICMP (env STATIC_EXCLUDE)

Env: PROXMOX_*, PROXMOX_SSH, APPS (space/comma-separated kubectl apply paths)
     CALICO_VERSION (default ${CALICO_VERSION})
     CONTROLPLANES, VIP, VIP6, DUAL_STACK=1
     ARCH / PERTISK_ARCH (amd64|arm64; arm64 → machine=virt + AAVMF)
     PROXMOX_MEMORY / PROXMOX_CORES (defaults for both roles)
     PROXMOX_CP_MEMORY / PROXMOX_CP_CORES / PROXMOX_WORKER_MEMORY / PROXMOX_WORKER_CORES
     PROXMOX_CP_DISK_GB / PROXMOX_WORKER_DISK_GB / PERTISK_DISK_GB
     PERTISK_IMAGES_DIR / PROXMOX_IMAGES_DIR (default: /var/lib/pertisk-mgmt/images or \$ROOT/out)
     JOIN_TRIES (default 15) JOIN_READYZ_WAIT (seconds to wait for joining CP :6443/readyz)
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
    --pod-subnet) POD_SUBNET="$2"; shift 2 ;;
    --service-subnet) SERVICE_SUBNET="$2"; shift 2 ;;
    --pod-subnet-ipv6) POD_SUBNET_IPV6="$2"; shift 2 ;;
    --service-subnet-ipv6) SERVICE_SUBNET_IPV6="$2"; shift 2 ;;
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
    --static-base) STATIC_BASE="$2"; shift 2 ;;
    --static-subnet) STATIC_SUBNET="$2"; shift 2 ;;
    --static-gateway) STATIC_GATEWAY="$2"; shift 2 ;;
    --static-nameserver) STATIC_NAMESERVER="$2"; shift 2 ;;
    --static-exclude) STATIC_EXCLUDE="$2"; shift 2 ;;
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

# Re-resolve default disk when --disk was not set, or when inherited PROXMOX_DISK
# points at a different arch (mgmt often has PROXMOX_DISK=…-amd64.qcow2).
_disk_arch_ok=0
[[ "$DISK" == *"pertisk-cloud-${ARCH}"* ]] && _disk_arch_ok=1
if [[ "$DISK_FROM_CLI" != "1" && ( -z "${PROXMOX_DISK:-}" || "$_disk_arch_ok" -eq 0 ) ]]; then
  if [[ -n "${PROXMOX_DISK:-}" && "$_disk_arch_ok" -eq 0 ]]; then
    echo "==> note: PROXMOX_DISK=${DISK} does not match ARCH=${ARCH} — re-resolving" >&2
  fi
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
unset _disk_arch_ok

# Proxmox VM names follow cluster name unless --prefix was set explicitly.
if [[ "$PREFIX_SET" -eq 0 ]]; then
  NAME_PREFIX="$CLUSTER_NAME"
fi
# `{prefix}-cp-1` is the Proxmox VM name and Kubernetes node hostname (RFC 1123).
if [[ ! "$NAME_PREFIX" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,48}[A-Za-z0-9])?$ ]]; then
  echo "error: cluster/prefix '${NAME_PREFIX}' is not a valid DNS name (Proxmox rejects '+')." >&2
  echo "  Use lab-ha-orion (letters, digits, hyphen), not lab-ha+orion." >&2
  exit 1
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
elif [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
  : "${NUTANIX_URL:?set NUTANIX_URL}"
  : "${NUTANIX_USER:?set NUTANIX_USER}"
  : "${NUTANIX_PASSWORD:?set NUTANIX_PASSWORD}"
  : "${NUTANIX_STORAGE:?set NUTANIX_STORAGE}"
  : "${NUTANIX_NETWORK:?set NUTANIX_NETWORK}"
  export NUTANIX_INSECURE="${NUTANIX_INSECURE:-1}"
  NX_HOST="$(echo "${NUTANIX_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
  if [[ -z "${LAB_SUBNET}" && "${NX_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
    LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
    echo "==> auto LAB_SUBNET=${LAB_SUBNET}"
  fi
  if [[ -n "${NUTANIX_DISK:-}" ]]; then
    DISK="${NUTANIX_DISK}"
  fi
  echo "==> provider=nutanix url=${NUTANIX_URL} storage=${NUTANIX_STORAGE} network=${NUTANIX_NETWORK}"
  echo "==> images dir=${IMAGES_DIR} disk=${DISK}"
  PROXMOX_URL="${PROXMOX_URL:-${NUTANIX_URL}}"
  PROXMOX_NODE="${PROXMOX_NODE:-${NUTANIX_CLUSTER:-ahv}}"
  unset PROXMOX_SSH || true
elif [[ "${PROVIDER_KIND}" == "pertisk-vms" ]]; then
  : "${PERTISK_VMS_URL:?set PERTISK_VMS_URL}"
  : "${PERTISK_VMS_USER:?set PERTISK_VMS_USER}"
  : "${PERTISK_VMS_PASSWORD:?set PERTISK_VMS_PASSWORD}"
  PERTISK_VMS_STORAGE="${PERTISK_VMS_STORAGE:-replica}"
  PERTISK_VMS_NETWORK="${PERTISK_VMS_NETWORK:-vmbr0}"
  export PERTISK_VMS_INSECURE="${PERTISK_VMS_INSECURE:-1}"
  PVMS_HOST="$(echo "${PERTISK_VMS_URL}" | sed -E 's|https?://([^/:]+).*|\1|')"
  if [[ -z "${LAB_SUBNET}" && "${PVMS_HOST}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\.[0-9]+$ ]]; then
    LAB_SUBNET="${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}.0/24"
    echo "==> auto LAB_SUBNET=${LAB_SUBNET}"
  fi
  if [[ -n "${PERTISK_VMS_DISK:-}" ]]; then
    DISK="${PERTISK_VMS_DISK}"
  fi
  if [[ -n "${PERTISK_VMS_STATIC_IPS:-}" ]]; then
    STATIC_IPS_ENV="${PERTISK_VMS_STATIC_IPS}"
  fi
  [[ -n "${PERTISK_VMS_STATIC_GATEWAY:-}" ]] && STATIC_GATEWAY="${PERTISK_VMS_STATIC_GATEWAY}"
  echo "==> provider=pertisk-vms url=${PERTISK_VMS_URL} storage=${PERTISK_VMS_STORAGE} network=${PERTISK_VMS_NETWORK}"
  echo "==> images dir=${IMAGES_DIR} disk=${DISK}"
  PROXMOX_URL="${PROXMOX_URL:-${PERTISK_VMS_URL}}"
  PROXMOX_NODE="${PROXMOX_NODE:-${PERTISK_VMS_NODE:-n1}}"
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
# (Omni-style — provider token only). Set PROXMOX_SSH=root@anything to use scp+qm;
# the host is rewritten to this provider's API host (multi-Proxmox).
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
# Global PROXMOX_SSH is user + mode; the host is always this provider's API host
# (one env cannot pin a single IP when mgmt has several Proxmox providers).
if [[ -n "${PROXMOX_SSH:-}" && -n "${PVE_HOST}" ]]; then
  _ssh_user="${PROXMOX_SSH%%@*}"
  [[ -z "${_ssh_user}" || "${_ssh_user}" == "${PROXMOX_SSH}" ]] && _ssh_user=root
  _ssh_h="${PROXMOX_SSH#*@}"
  _ssh_h="${_ssh_h%%:*}"
  if [[ "${_ssh_h}" != "${PVE_HOST}" ]]; then
    echo "==> PROXMOX_SSH=${PROXMOX_SSH} → ${_ssh_user}@${PVE_HOST} (this provider)"
    export PROXMOX_SSH="${_ssh_user}@${PVE_HOST}"
  fi
  unset _ssh_user _ssh_h
fi
# Keys are per-PVE. If this provider rejects SSH, use the API for the whole lab
# (import already fell back; resize must not still call qm over a dead session).
if [[ -n "${PROXMOX_SSH:-}" ]]; then
  if ! ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=8 \
      "${PROXMOX_SSH}" true >/dev/null 2>&1; then
    echo "==> SSH ${PROXMOX_SSH} not usable (no key auth) — Proxmox API for this provider"
    unset PROXMOX_SSH || true
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
echo "==> images dir=${IMAGES_DIR} arch=${ARCH} disk=${DISK}"
if [[ "${CP_DISK:-$DISK}" != "$DISK" || "${WORKER_DISK:-$DISK}" != "$DISK" ]]; then
  echo "==> sized disks cp=${CP_DISK:-$DISK} wk=${WORKER_DISK:-$DISK}"
fi
fi # PROVIDER_KIND != vsphere|nutanix

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
elif [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
  [[ "${NUTANIX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
  AUTH=""
  BASE=""
  NODE="${NUTANIX_CLUSTER:-ahv}"
elif [[ "${PROVIDER_KIND}" == "pertisk-vms" ]]; then
  [[ "${PERTISK_VMS_INSECURE:-0}" == "1" ]] && CURL+=(-k)
  AUTH=""
  BASE=""
  NODE="${PERTISK_VMS_NODE:-n1}"
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
# $1 = vmid, $2 = optional guest/VM name (e.g. lab-cp-1) for vsphere/nutanix inventory lookup
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
  if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
    if [[ -n "$name" ]]; then
      mac="$(nutanix_vm_mac "$name" 2>/dev/null || true)"
    fi
    if [[ -z "$mac" ]]; then
      mac="$(nutanix_vm_mac "${NAME_PREFIX}-${vmid}" 2>/dev/null || true)"
    fi
    [[ -n "$mac" ]] || die "VM ${name:-$vmid}: no MAC yet; power on once so AHV assigns one"
    echo "$mac" | tr 'A-F' 'a-f'
    return 0
  fi
  if [[ "${PROVIDER_KIND}" == "pertisk-vms" ]]; then
    if [[ -n "$name" ]]; then
      mac="$(pertisk_vms_vm_mac "$name" 2>/dev/null || true)"
    fi
    if [[ -z "$mac" ]]; then
      mac="$(pertisk_vms_vm_mac "${NAME_PREFIX}-${vmid}" 2>/dev/null || true)"
    fi
    [[ -n "$mac" ]] || die "VM ${name:-$vmid}: no MAC yet; power on once so pertisk-vms assigns one"
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

# IPv4 from QEMU guest agent (Proxmox virtio serial — no L2 ARP needed).
# Guest reports DHCP as soon as qemu-ga is up (pertiskd starts it after early net).
qemu_agent_ipv4() {
  local vmid="$1" json ip
  [[ "${PROVIDER_KIND}" == "vsphere" || "${PROVIDER_KIND}" == "nutanix" || "${PROVIDER_KIND}" == "pertisk-vms" ]] && return 0
  [[ -n "${BASE:-}" && -n "${NODE:-}" ]] || return 0
  json="$(api_get "/nodes/${NODE}/qemu/${vmid}/agent/network-get-interfaces" 2>/dev/null || true)"
  [[ -n "$json" ]] || return 0
  ip="$(printf '%s' "$json" | python3 -c '
import json,sys
raw=sys.stdin.read()
try:
    data=json.loads(raw)
except Exception:
    sys.exit(0)
blob=data.get("data") or data
result=blob.get("result") if isinstance(blob, dict) else blob
if not isinstance(result, list):
    sys.exit(0)
for iface in result:
    if not isinstance(iface, dict):
        continue
    name=(iface.get("name") or "").lower()
    if name in ("lo", "lo0"):
        continue
    for a in iface.get("ip-addresses") or []:
        if not isinstance(a, dict):
            continue
        if (a.get("ip-address-type") or "").lower() != "ipv4":
            continue
        ip=a.get("ip-address") or ""
        if ip and not ip.startswith("127."):
            print(ip)
            sys.exit(0)
' 2>/dev/null || true)"
  if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    if [[ -n "${LAB_SUBNET:-}" ]]; then
      local base="${LAB_SUBNET%/*}"
      base="${base%.*}."
      [[ "$ip" == ${base}* ]] || return 0
    fi
    echo "$ip"
  fi
}

# Global/ULA IPv6 from QGA (skip link-local fe80::).
qemu_agent_ipv6() {
  local vmid="$1" json ip
  [[ "${PROVIDER_KIND}" == "vsphere" || "${PROVIDER_KIND}" == "nutanix" || "${PROVIDER_KIND}" == "pertisk-vms" ]] && return 0
  [[ -n "${BASE:-}" && -n "${NODE:-}" ]] || return 0
  json="$(api_get "/nodes/${NODE}/qemu/${vmid}/agent/network-get-interfaces" 2>/dev/null || true)"
  [[ -n "$json" ]] || return 0
  ip="$(printf '%s' "$json" | python3 -c '
import json,sys
raw=sys.stdin.read()
try:
    data=json.loads(raw)
except Exception:
    sys.exit(0)
blob=data.get("data") or data
result=blob.get("result") if isinstance(blob, dict) else blob
if not isinstance(result, list):
    sys.exit(0)
for iface in result:
    if not isinstance(iface, dict):
        continue
    name=(iface.get("name") or "").lower()
    if name in ("lo", "lo0"):
        continue
    addrs=[]
    for a in iface.get("ip-addresses") or []:
        if not isinstance(a, dict):
            continue
        if (a.get("ip-address-type") or "").lower() != "ipv6":
            continue
        ip=(a.get("ip-address") or "").split("%")[0]
        if not ip or ip.lower().startswith("fe80:"):
            continue
        addrs.append(ip)
    # Prefer GUA (not fd/fc ULA) when both exist.
    for ip in addrs:
        if not ip.lower().startswith(("fd", "fc")):
            print(ip)
            sys.exit(0)
    if addrs:
        print(addrs[0])
        sys.exit(0)
' 2>/dev/null || true)"
  [[ -n "$ip" ]] && echo "$ip"
}

wait_guest_ipv6() {
  local vmid="$1" label="$2" deadline ip6="" last=0
  [[ "${DUAL_STACK}" == "1" ]] || return 0
  case "${PROVIDER_KIND:-proxmox}" in
    nutanix | ahv | prism | vsphere | esxi | pertisk-vms)
      log "skip QGA IPv6 wait on ${PROVIDER_KIND} (no Proxmox qemu-guest-agent); IPv4-only guests are OK"
      return 0
      ;;
  esac
  deadline=$((SECONDS + 90))
  log "VM ${vmid} (${label}) waiting for IPv6 (dual-stack, QGA)"
  while (( SECONDS < deadline )); do
    ip6="$(qemu_agent_ipv6 "$vmid" || true)"
    if [[ -n "$ip6" ]]; then
      log "VM ${vmid} IPv6=${ip6}"
      return 0
    fi
    if (( SECONDS - last >= 15 )); then
      last=$SECONDS
      log "VM ${vmid} no global/ULA IPv6 yet…"
    fi
    sleep 3
  done
  log "WARNING: VM ${vmid} (${label}) still has IPv4 only after dual-stack apply (no GUA/ULA via QGA)"
}

nutanix_vm_mac() {
  local name="$1" base api resp uuid detail mac nics
  base="${NUTANIX_URL%/}"
  api="${base}/api/nutanix/v2.0"
  local curl_args=(curl -sS)
  [[ "${NUTANIX_INSECURE:-0}" == "1" ]] && curl_args+=(-k)
  curl_args+=(-u "${NUTANIX_USER}:${NUTANIX_PASSWORD}" -H 'Accept: application/json')
  resp="$("${curl_args[@]}" "${api}/vms?include_vm_nic_config=true")"
  # Prefer MAC from list (with nic config); else uuid → detail / nics endpoint.
  mac="$(VMS_JSON="$resp" python3 -c '
import json,os,sys
want=sys.argv[1].lower()
data=json.loads(os.environ["VMS_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [])
for e in ents:
    if (e.get("name") or "").lower()!=want:
        continue
    for nic in (e.get("vm_nics") or e.get("nic_list") or []):
        m=nic.get("mac_address") or nic.get("mac_addr")
        if m:
            print(m)
            raise SystemExit
    print("UUID:"+ (e.get("uuid") or ""))
    raise SystemExit
' "$name" 2>/dev/null || true)"
  if [[ "$mac" == UUID:* ]]; then
    uuid="${mac#UUID:}"
    mac=""
  elif [[ -n "$mac" ]]; then
    echo "$mac"
    return 0
  else
    return 0
  fi
  [[ -n "$uuid" ]] || return 0
  detail="$("${curl_args[@]}" "${api}/vms/${uuid}?include_vm_nic_config=true")"
  mac="$(echo "$detail" | jq -r '
    (.vm_nics // .nic_list // [])
    | map(.mac_address // .mac_addr // empty)
    | map(select(. != null and . != ""))
    | .[0] // empty
  ')"
  if [[ -z "$mac" ]]; then
    nics="$("${curl_args[@]}" "${api}/vms/${uuid}/nics" 2>/dev/null || true)"
    mac="$(echo "${nics:-}" | jq -r '
      (.entities // . // [])
      | if type=="array" then . else [.] end
      | map(.mac_address // .mac_addr // empty)
      | map(select(. != null and . != ""))
      | .[0] // empty
    ' 2>/dev/null || true)"
  fi
  [[ -n "$mac" ]] && echo "$mac"
}

# IPs AHV learned on the guest NIC (works even when mgmt is not on the same L2).
nutanix_vm_ips() {
  local name="$1" base api resp
  base="${NUTANIX_URL%/}"
  api="${base}/api/nutanix/v2.0"
  local curl_args=(curl -sS)
  [[ "${NUTANIX_INSECURE:-0}" == "1" ]] && curl_args+=(-k)
  curl_args+=(-u "${NUTANIX_USER}:${NUTANIX_PASSWORD}" -H 'Accept: application/json')
  resp="$("${curl_args[@]}" "${api}/vms?include_vm_nic_config=true")"
  VMS_JSON="$resp" python3 -c '
import json,os,sys
want=sys.argv[1].lower()
data=json.loads(os.environ["VMS_JSON"])
ents=data.get("entities") or (data if isinstance(data,list) else [])
for e in ents:
    if (e.get("name") or "").lower()!=want:
        continue
    ips=[]
    for nic in (e.get("vm_nics") or e.get("nic_list") or []):
        for key in ("ip_addresses","ip_address","assigned_ips"):
            v=nic.get(key)
            if isinstance(v,list):
                ips.extend([x for x in v if isinstance(x,str) and "." in x])
            elif isinstance(v,str) and "." in v:
                ips.append(v)
        ea=nic.get("endpoint_address") or nic.get("requested_ip_address")
        if isinstance(ea,str) and "." in ea:
            ips.append(ea)
    for ip in ips:
        print(ip)
    raise SystemExit
' "$name" 2>/dev/null || true
}

pertisk_vms_token() {
  if [[ -n "${PERTISK_VMS_TOKEN:-}" ]]; then
    echo "$PERTISK_VMS_TOKEN"
    return 0
  fi
  local curl_args=(curl -sS)
  [[ "${PERTISK_VMS_INSECURE:-0}" == "1" ]] && curl_args+=(-k)
  PERTISK_VMS_TOKEN="$("${curl_args[@]}" -X POST "${PERTISK_VMS_URL%/}/v1/login" \
    -H 'Content-Type: application/json' \
    -d "{\"username\":\"${PERTISK_VMS_USER}\",\"password\":\"${PERTISK_VMS_PASSWORD}\"}" \
    | jq -r '.token // empty')"
  export PERTISK_VMS_TOKEN
  echo "${PERTISK_VMS_TOKEN:-}"
}

pertisk_vms_vm_json() {
  local name="$1" token
  token="$(pertisk_vms_token)"
  [[ -n "$token" ]] || return 0
  local curl_args=(curl -sS)
  [[ "${PERTISK_VMS_INSECURE:-0}" == "1" ]] && curl_args+=(-k)
  curl_args+=(-H "Authorization: Bearer ${token}" -H 'Accept: application/json')
  "${curl_args[@]}" "${PERTISK_VMS_URL%/}/v1/vms" | jq -c --arg n "$name" --arg id "$name" '
    (if type=="array" then . else (.vms // []) end)
    | map(select((.spec.name == $n) or (.id|tostring) == $id or ((.spec.name // "") | endswith("-"+$id))))
    | .[0] // empty
  ' 2>/dev/null || true
}

pertisk_vms_vm_mac() {
  local name="$1" json
  json="$(pertisk_vms_vm_json "$name")"
  [[ -n "$json" && "$json" != "null" ]] || return 0
  echo "$json" | jq -r '(.spec.nets // []) | map(.mac // empty) | map(select(. != "")) | .[0] // empty'
}

pertisk_vms_vm_ips() {
  local name="$1" json
  json="$(pertisk_vms_vm_json "$name")"
  [[ -n "$json" && "$json" != "null" ]] || return 0
  echo "$json" | jq -r '(.spec.nets // []) | map(.ip // empty) | map(select(. != null and . != "")) | .[]'
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
  # Empty MAC matches everything in awk/grep — never allow that (returns Docker
  # bridge IPs like 172.18.0.2 and looks like a false DHCP hit).
  if [[ -z "$mac" || -z "$mac_cmp" || ${#mac_cmp} -lt 8 ]]; then
    return 0
  fi
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
    # Prefer LAB_SUBNET hits when set (ignore Docker 172.18/16 etc.).
    if [[ -n "${LAB_SUBNET:-}" ]]; then
      local base="${LAB_SUBNET%/*}"
      base="${base%.*}."
      if [[ "$out" != ${base}* ]]; then
        return 0
      fi
    fi
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

# Check if an IP is in the exclude list. E.g., is_ip_excluded 10.1.1.111
# STATIC_EXCLUDE="10.1.1.111,10.1.1.194" → true for .111 and .194.
is_ip_excluded() {
  local ip="$1" exclude_list="${STATIC_EXCLUDE:-}"
  [[ -z "$exclude_list" ]] && return 1
  [[ ",${exclude_list}," == *",${ip},"* ]] && return 0
  return 1
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

# ICMP is a liveness hint. Nutanix IPAM issues an address at NIC create; AHV may
# ARP that MAC→IP before the guest answers ping. :50000 is the live signal.
guest_icmp_alive() {
  local ip="$1"
  ping -c1 -W2 "$ip" >/dev/null 2>&1
}

# Offer the Prism IPAM address over DHCP from mgmt so an already-running guest
# (dashboard "no ip") can bind it without recreating the VM.
# Skip when not root — UDP/67 will fail and the log is noise on unmanaged vlan.0
# (LAN DHCP already works). Set NUTANIX_IPAM_DHCP=1 to force.
nutanix_start_ipam_dhcp() {
  local mac="$1" ip="$2" helper gw prefix
  if [[ "${NUTANIX_IPAM_DHCP:-}" != "1" && "$(id -u 2>/dev/null || echo 1)" != "0" ]]; then
    return 0
  fi
  helper="${ROOT}/scripts/nutanix-ipam-dhcp.sh"
  [[ -x "$helper" ]] || helper="/usr/share/pertisk-mgmt/scripts/nutanix-ipam-dhcp.sh"
  [[ -x "$helper" ]] || return 0
  gw="${NUTANIX_GATEWAY:-${LAB_GATEWAY:-}}"
  if [[ -z "$gw" ]]; then
    gw="$(ip -4 route show default 2>/dev/null | awk '{
      for (i = 1; i < NF; i++) if ($i == "via") { print $(i+1); exit }
    }')"
  fi
  prefix="${LAB_SUBNET##*/}"
  [[ "$prefix" =~ ^[0-9]+$ ]] || prefix=24
  log "IPAM DHCP helper ${mac} → ${ip} (guest has no address yet; AHV reservation is not a lease)"
  "$helper" "$mac" "$ip" "${gw:-}" "$prefix" || log "warn: IPAM DHCP helper failed (need root for UDP/67?)"
}

# Build VMID→IP map from auto-detected static IPs (space-separated list).
# Call early so wait_ip() can use them directly.
build_static_ips_map() {
  [[ -z "$STATIC_IPS_ENV" ]] && return 0
  local ip_idx=0 vmid cp_idx wk_idx
  for ip in $STATIC_IPS_ENV; do
    # Skip CIDR suffix (e.g., 10.1.1.13/24 → 10.1.1.13)
    ip="${ip%/*}"
    if (( ip_idx < CONTROLPLANES )); then
      vmid=$((CP_VMID + ip_idx))
      STATIC_IPS_MAP[$vmid]="$ip"
      ip_idx=$((ip_idx + 1))
    elif (( ip_idx < CONTROLPLANES + WORKERS )); then
      vmid=$((CP_VMID + ip_idx))
      STATIC_IPS_MAP[$vmid]="$ip"
      ip_idx=$((ip_idx + 1))
    fi
  done
  if [[ ${#STATIC_IPS_MAP[@]} -gt 0 ]]; then
    local map_str=""
    for vmid in "${!STATIC_IPS_MAP[@]}"; do
      map_str+="vmid=$vmid:${STATIC_IPS_MAP[$vmid]} "
    done
    log "using pre-assigned static IPs: $map_str"
  fi
}

wait_ip() {
  local vmid="$1" label="$2" mac ip="" static_ip="" nxip="" alt="" nudged=0 saw_ip=0 last_log=0 live=0 issued=0 dhcp_helper=0
  local ip_deadline=$((SECONDS + IP_TIMEOUT)) api_deadline=0 deadline left
  mac="$(vm_mac "$vmid" "$label")"
  
  # If static IP is pre-assigned for this VMID, save it (don't lose it in the loop).
  if [[ -n "${STATIC_IPS_MAP[$vmid]:-}" ]]; then
    static_ip="${STATIC_IPS_MAP[$vmid]}"
    log "VM ${vmid} (${label}) MAC=${mac} — using pre-assigned static IP ${static_ip}"
    if api_reachable "$static_ip"; then
      log "VM ${vmid} → ${static_ip} (API :50000 up, pre-assigned static IP)"
      echo "$static_ip"
      return 0
    else
      log "VM ${vmid} static IP ${static_ip} not yet reachable; waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s)"
      saw_ip=1
      api_deadline=$((SECONDS + API_AFTER_IP_TIMEOUT))
      ip="$static_ip"
    fi
  else
    if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
      log "VM ${vmid} (${label}) MAC=${mac} — waiting for IPAM/DHCP IP (timeout ${IP_TIMEOUT}s; +${API_AFTER_IP_TIMEOUT}s after issued IP for :50000)"
    else
      log "VM ${vmid} (${label}) MAC=${mac} — waiting for DHCP IP (timeout ${IP_TIMEOUT}s; +${API_AFTER_IP_TIMEOUT}s after live IP for :50000)"
    fi
    ip_deadline=$((SECONDS + IP_TIMEOUT))
  fi
  while true; do
    if (( saw_ip )); then
      deadline=$api_deadline
    else
      deadline=$ip_deadline
    fi
    (( SECONDS < deadline )) || break

    # If we have a static IP, keep using it; only discover via ARP/QGA if not set.
    if [[ -z "$ip" ]]; then
      ip="$(arp_ip_for_mac "$mac" || true)"
      # Proxmox QGA: guest IPv4 without sharing L2 / ARP with mgmt.
      if [[ -z "$ip" ]]; then
        ip="$(qemu_agent_ipv4 "$vmid" || true)"
      fi
    fi
    issued=0
    # Static netcfg: the address is already pinned. ICMP is optional (same as
    # Nutanix IPAM); :50000 is the live signal.
    if [[ -n "$static_ip" ]]; then
      issued=1
      ip="$static_ip"
    fi
    if [[ "${PROVIDER_KIND}" == "nutanix" && -n "$label" ]]; then
      nxip="$(nutanix_vm_ips "$label" 2>/dev/null | head -1 || true)"
      if [[ -z "$nxip" ]]; then
        nxip="$(nutanix_vm_ips "${NAME_PREFIX}-${vmid}" 2>/dev/null | head -1 || true)"
      fi
      if [[ -n "$nxip" ]]; then
        ip="$nxip"
        issued=1
      fi
    fi
    if [[ "${PROVIDER_KIND}" == "pertisk-vms" && -n "$label" ]]; then
      pvip="$(pertisk_vms_vm_ips "$label" 2>/dev/null | head -1 || true)"
      if [[ -z "$pvip" ]]; then
        pvip="$(pertisk_vms_vm_ips "${NAME_PREFIX}-${vmid}" 2>/dev/null | head -1 || true)"
      fi
      if [[ -n "$pvip" ]]; then
        ip="$pvip"
        issued=1
      fi
    fi
    # Only sweep when we still have no IP — never re-sweep while waiting on :50000.
    if [[ -z "$ip" && -n "$LAB_SUBNET" ]]; then
      if [[ "$nudged" == "0" ]] || (( SECONDS % 60 < 3 )); then
        nudged=1
        ip="$(ping_sweep_find "$mac" "$LAB_SUBNET" || true)"
      fi
    fi
    # Skip excluded IPs (provider hosts, existing nodes, etc). Never drop a
    # pre-assigned static address — that is the address we pinned on the guest.
    if [[ -n "$ip" && -z "$static_ip" ]] && is_ip_excluded "$ip"; then
      log "VM ${vmid} found IP=${ip} for ${mac} but it's in STATIC_EXCLUDE (provider/existing node) — ignoring, still waiting…"
      ip=""
    fi
    # Ghost IPAM reservation: guest DHCP may land on a different address.
    # BUT: don't scan for alternatives if we have a pre-assigned static IP — stick with it.
    if [[ -n "$ip" && -n "$LAB_SUBNET" && -z "$static_ip" ]] && ! api_reachable "$ip"; then
      if (( SECONDS % 60 < 5 )); then
        alt="$(scan_api_subnet_for_mac "$mac" "$LAB_SUBNET" || true)"
        if [[ -n "$alt" ]] && api_reachable "$alt"; then
          ip="$alt"
          issued=1
        fi
      fi
    fi
    # Guest may have fallen back to DHCP (netcfg disk late). QGA is the live address.
    qga="$(qemu_agent_ipv4 "$vmid" || true)"
    if [[ -n "$qga" ]] && api_reachable "$qga"; then
      if [[ -n "$static_ip" && "$qga" != "$static_ip" ]]; then
        log "VM ${vmid} → ${qga} (API :50000 up; QGA DHCP, netcfg wanted ${static_ip})"
      else
        log "VM ${vmid} → ${qga} (API :50000 up${static_ip:+, pre-assigned static IP})"
      fi
      echo "$qga"
      return 0
    fi
    if [[ -n "$ip" ]] && api_reachable "$ip"; then
      log "VM ${vmid} → ${ip} (API :50000 up)"
      echo "$ip"
      return 0
    fi
    if [[ -n "$ip" ]]; then
      live=0
      if guest_icmp_alive "$ip"; then
        live=1
      fi
      # Nutanix IPAM issued the address — wait for :50000 even without ICMP.
      # AHV answers ARP for the reservation before the guest stack is up.
      if (( live || issued )); then
        if (( !saw_ip )); then
          saw_ip=1
          api_deadline=$((SECONDS + API_AFTER_IP_TIMEOUT))
          last_log=$SECONDS
          if (( live )); then
            log "VM ${vmid} live=${ip} (ICMP ok) — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s)"
          elif [[ -n "$static_ip" ]]; then
            log "VM ${vmid} static=${ip} — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s; ICMP optional)"
          else
            log "VM ${vmid} IPAM issued ${ip} — waiting for Machine API :50000 (timeout ${API_AFTER_IP_TIMEOUT}s; ICMP optional on AHV)"
            if [[ "${PROVIDER_KIND}" == "nutanix" && "$dhcp_helper" == "0" ]]; then
              dhcp_helper=1
              nutanix_start_ipam_dhcp "$mac" "$ip"
            fi
          fi
        elif (( SECONDS - last_log >= 20 )); then
          last_log=$SECONDS
          left=$((api_deadline - SECONDS))
          (( left < 0 )) && left=0
          qga="$(qemu_agent_ipv4 "$vmid" || true)"
          if (( live )); then
            log "VM ${vmid} live=${ip} but :50000 not ready yet... (${left}s left)"
          elif [[ -n "$qga" && "$qga" != "$ip" ]]; then
            log "VM ${vmid} expected ${ip} but QGA reports ${qga} — :50000 not up (${left}s left)"
          elif [[ -n "$qga" ]]; then
            log "VM ${vmid} QGA has ${ip} but :50000 not reachable from mgmt (${left}s left; duplicate IP or firewall?)"
          elif [[ -n "$static_ip" ]]; then
            log "VM ${vmid} static=${ip} — waiting for guest :50000 (${left}s left; QGA not up yet)"
          else
            log "VM ${vmid} issued=${ip} but :50000 not ready yet... (${left}s left)"
            if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
              log "hint: Prism Serial Console on ${label}. VGA EFI stub is normal; Serial should show pertiskd."
            fi
          fi
        fi
      else
        # ARP/QGA candidate without ICMP: do not start the long API timer.
        # Pre-assigned static IPs are issued=1 above (never land here).
        if [[ -z "$static_ip" ]]; then
          saw_ip=0
        fi
        if (( SECONDS - last_log >= 15 )); then
          last_log=$SECONDS
          log "VM ${vmid} candidate IP=${ip} for ${mac} but no ICMP — still waiting for live guest…"
        fi
      fi
    else
      if (( SECONDS - last_log >= 15 )); then
        last_log=$SECONDS
        log "VM ${vmid} no ARP yet for ${mac}…"
        if [[ "${PROVIDER_KIND}" == "proxmox" || -z "${PROVIDER_KIND:-}" ]]; then
          log "hint: Proxmox Summary IP / QGA for VM ${vmid}; mgmt L2 ARP optional when qemu-ga is up."
        fi
        if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
          log "hint: open Prism → ${label} → Serial Console (kServer). If still on EFI stub only, guest never reached userspace."
          log "hint: confirm AHV network '${NUTANIX_NETWORK:-?}' is same L2/DHCP as LAB_SUBNET=${LAB_SUBNET:-?} (mgmt ${NUTANIX_HTTP_ADDR:-auto})."
        fi
      fi
    fi
    sleep 3
  done
  if (( saw_ip )); then
    if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
      die "timed out waiting for Machine API :50000 on ${ip:-?} (VM ${vmid} MAC=${mac}; AHV IPAM issued this address but the guest never bound :50000)
hint: Prism → ${label} → Serial Console for pertiskd logs (VGA EFI stub is normal).
      --skip-vms reuses this guest and will time out again. Recreate the VMs (omit --skip-vms), or:
      sudo /usr/share/pertisk-mgmt/scripts/nutanix-ipam-dhcp.sh ${mac} ${ip:-?} ${LAB_GATEWAY:-${NUTANIX_GATEWAY:-}}
      then Prism power-cycle ${label} so it DISCOVERs again.
      from mgmt: nc -zv ${ip:-?} 50000"
    fi
    die "timed out waiting for Machine API :50000 on ${ip:-?} (VM ${vmid} MAC=${mac}; guest address was known but :50000 never became reachable from mgmt)
hint: Proxmox → VM ${vmid} → Console / QGA Summary IP. Duplicate IP and firewall both look like this.
      ping -c2 ${ip:-?}; nc -zv ${ip:-?} 50000"
  fi
  if [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
    die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (subnet=${LAB_SUBNET:-unset})
hint: Prism IPAM did not report a NIC IP. Check network '${NUTANIX_NETWORK:-?}' and Serial Console on ${label}.
      resume: lab-up --skip-build --skip-vms --cp-vmid ${CP_VMID} --controlplanes ${CONTROLPLANES} --workers ${WORKERS}"
  fi
  die "timed out waiting for IP/API for VM ${vmid} MAC=${mac} (PROXMOX_SSH=${PROXMOX_SSH:-unset} subnet=${LAB_SUBNET:-unset})
hint: without PROXMOX_SSH, mgmt uses QGA then LAB_SUBNET ping-sweep.
      check: Proxmox → VM ${vmid} → Summary (QEMU Guest Agent IPs);
             ip -4 neigh | grep -i ${mac}; ping -c2 <ip>; nc -zv <ip> 50000
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

# After `reset --force` (reboot_scheduled), :50000 stays up until the guest
# actually reboots. wait_ip's static-IP fast path would return immediately
# and bootstrap then hits connection reset.
wait_api_down() {
  local ip="$1" timeout_s="${2:-90}"
  local deadline=$((SECONDS + timeout_s))
  log "waiting for Machine API ${ip}:50000 to drop after reset…"
  while (( SECONDS < deadline )); do
    if ! api_reachable "$ip"; then
      log "Machine API ${ip}:50000 is down"
      return 0
    fi
    sleep 1
  done
  log "WARNING: ${ip}:50000 still up after ${timeout_s}s — continuing"
}

wait_after_reset() {
  local vmid="$1" label="$2" ip="$3"
  wait_api_down "$ip" 90
  sleep 3
  wait_ip "$vmid" "$label"
}

# HTTP /readyz on a node IP (not TCP). Join finalize used to fail with
# Connection reset by peer while :6443 accepted sockets.
https_readyz() {
  local ip="$1"
  local host="$ip"
  [[ "$ip" == *:* && "$ip" != \[* ]] && host="[${ip}]"
  curl -sk --connect-timeout 3 --max-time 8 "https://${host}:6443/readyz" 2>/dev/null | grep -qi ok
}

wait_https_readyz() {
  local ip="$1" timeout_s="${2:-180}"
  local deadline=$((SECONDS + timeout_s))
  if https_readyz "$ip"; then
    return 0
  fi
  log "waiting for https://${ip}:6443/readyz (up to ${timeout_s}s; TCP :6443 can RST HTTP after etcd join)"
  while (( SECONDS < deadline )); do
    https_readyz "$ip" && return 0
    sleep 5
  done
  return 1
}

# True when this guest already wrote admin.conf (bootstrapped or joined).
guest_has_admin_kubeconfig() {
  local ip="$1" tmp
  tmp="$(mktemp "${TMPDIR:-/tmp}/pertisk-kc.XXXXXX")"
  if "$CTL" -e "${ip}:50000" kubeconfig -f "$tmp" >/dev/null 2>&1 \
    && grep -q 'certificate-authority-data:' "$tmp"; then
    rm -f "$tmp"
    return 0
  fi
  rm -f "$tmp"
  return 1
}

# Apply after :50000 is up. The API listens *before* the STATE partition is
# mounted; an early write lands on initramfs `/system/state` and disappears
# when prepare_state mounts the disk over that path (bootstrap then sees
# "config.yaml: No such file"). Re-apply after a short wait so the second
# write hits the real volume. New images also block apply until STATE is bound.
apply_machine_yaml() {
  local ip="$1" yaml="$2" vmid="${3:-}" i
  [[ -f "$yaml" ]] || die "apply: missing ${yaml}"
  log "apply ${yaml##*/} → ${ip} (wait for STATE mount)"
  for i in $(seq 1 24); do
    if "$CTL" -e "${ip}:50000" apply -f "$yaml"; then
      sleep 8
      if "$CTL" -e "${ip}:50000" apply -f "$yaml"; then
        [[ -n "$vmid" ]] && wait_guest_ipv6 "$vmid" "${yaml##*/}"
        return 0
      fi
    fi
    log "apply not ready yet (try ${i}/24) — STATE may still be mounting"
    sleep 5
  done
  die "apply failed for ${ip} (${yaml})"
}

# Refuse soft-reset/apply only when :50000 already belongs to a *different*
# Pertisk cluster node (hostname like other-*-cp-N / *-wk-N). Leftover short
# names on reused disks (e.g. 51fad4-wk) must still be soft-resettable.
assert_guest_identity() {
  local ip="$1" expected_host="${2:-}" out host prefix
  out="$("$CTL" -e "${ip}:50000" version 2>/dev/null || true)"
  host="$(printf '%s\n' "$out" | sed -n 's/.*hostname=\([^ ]*\).*/\1/p' | head -1)"
  [[ -n "$host" && -n "$expected_host" ]] || return 0
  case "$host" in
    pertisk|localhost|"$expected_host") return 0 ;;
  esac
  # Expected is always {cluster}-cp-N or {cluster}-wk-N.
  prefix="${expected_host%-cp-*}"
  prefix="${prefix%-wk-*}"
  if [[ "$host" == "$prefix"-cp-* || "$host" == "$prefix"-wk-* ]]; then
    # Same cluster, different role/index — still OK to reset before re-apply.
    log "note: guest ${ip} hostname='${host}' (will become ${expected_host})"
    return 0
  fi
  if [[ "$host" =~ ^.+-cp-[0-9]+$ || "$host" =~ ^.+-wk-[0-9]+$ ]]; then
    die "refusing to touch ${ip}: guest hostname is '${host}', expected '${expected_host}'.
Likely DHCP/MAC collision with another cluster on this LAN (same base VMID on two Proxmox hosts).
Pick a free --cp-vmid (wizard suggests one) and redeploy MAC-salted upload scripts before recreate."
  fi
  log "note: guest ${ip} hostname='${host}' (leftover; soft-reset will clear → ${expected_host})"
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

# Dump why :6443/readyz never answered (CNI is unrelated — it installs later).
diagnose_apiserver_wait() {
  local cp_ip="$1" kc="${2:-}"
  local host="$cp_ip"
  [[ "$cp_ip" == *:* ]] && host="[${cp_ip}]"
  log "diagnose apiserver ${cp_ip}:6443 (CNI is not installed yet — expected)"
  echo "--- curl https://${host}:6443/readyz ---" >&2
  curl -sk --connect-timeout 3 -o /tmp/pertisk-readyz.out -w "http=%{http_code}\n" \
    "https://${host}:6443/readyz" 2>&1 || true
  [[ -s /tmp/pertisk-readyz.out ]] && cat /tmp/pertisk-readyz.out >&2
  rm -f /tmp/pertisk-readyz.out
  if [[ -n "$kc" && -f "$kc" ]]; then
    echo "--- kubectl get --raw=/readyz ---" >&2
    kubectl --kubeconfig "$kc" get --raw=/readyz 2>&1 | tail -n 20 >&2 || true
  fi
  if [[ -n "${CTL:-}" ]]; then
    echo "--- pertiskctl health ---" >&2
    "$CTL" -e "${cp_ip}:50000" health 2>&1 || true
    echo "--- containerd log (tail) ---" >&2
    "$CTL" -e "${cp_ip}:50000" logs containerd -n 40 2>&1 || true
    echo "--- kubelet log (tail) ---" >&2
    "$CTL" -e "${cp_ip}:50000" logs kubelet -n 40 2>&1 || true
  fi
}

# Wait for apiserver: always via a CP node IP first; then VIP when HA.
wait_apiserver_ready() {
  local kc="$1" cp_ip="$2" endpoint="$3"
  local deadline tmpkc last_err="" tick=0

  # 1) Direct CP (kube-vip may still be pulling / electing).
  tmpkc="${kc}.direct"
  cp "$kc" "$tmpkc"
  rewrite_kubeconfig_server "$tmpkc" "https://${cp_ip}:6443"
  log "waiting for apiserver on CP ${cp_ip}:6443 (timeout ${BOOTSTRAP_TIMEOUT}s; CNI comes after this)"
  deadline=$((SECONDS + BOOTSTRAP_TIMEOUT))
  until last_err="$(kubectl --kubeconfig "$tmpkc" get --raw=/readyz 2>&1)"; do
    if (( SECONDS >= deadline )); then
      diagnose_apiserver_wait "$cp_ip" "$tmpkc"
      rm -f "$tmpkc"
      die "apiserver not ready on ${cp_ip}:6443 after ${BOOTSTRAP_TIMEOUT}s
Last kubectl: ${last_err}
This is before CNI install — fix image pulls / static pods on the CP guest first."
    fi
    tick=$((tick + 1))
    if (( tick % 10 == 0 )); then
      log "still waiting for ${cp_ip}:6443/readyz (${SECONDS}s elapsed)…"
    fi
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
      # Workers still had cluster.endpoint=VIP; rewrite so TLS bootstrap can reach the API.
      rewrite_cluster_out_endpoints "https://${cp_ip}:6443"
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
# Empty-string label values are valid — do not use `grep -q .` on jsonpath.
ensure_control_plane_roles() {
  local kc="$1" n="$2" i node deadline
  for ((i = 1; i <= n; i++)); do
    node="${CLUSTER_NAME}-cp-${i}"
    deadline=$((SECONDS + 180))
    until kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; do
      (( SECONDS < deadline )) || die "node ${node} not registered for control-plane role ensure"
      sleep 3
    done
    if kubectl --kubeconfig "$kc" get node "$node" -o json 2>/dev/null \
      | grep -Fq '"node-role.kubernetes.io/control-plane"'; then
      continue
    fi
    log "WARNING: ${node} missing control-plane role — labeling + tainting"
    kubectl --kubeconfig "$kc" label node "$node" 'node-role.kubernetes.io/control-plane=' --overwrite \
      || die "failed to label ${node} control-plane"
    kubectl --kubeconfig "$kc" taint node "$node" 'node-role.kubernetes.io/control-plane=:NoSchedule' --overwrite || true
    kubectl --kubeconfig "$kc" get node "$node" -o json 2>/dev/null \
      | grep -Fq '"node-role.kubernetes.io/control-plane"' \
      || die "node ${node} still missing control-plane role after label"
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

# Core Kubernetes deliberately leaves kubelet-serving CSRs Pending. During this
# trusted provisioning flow, approve Pending requests from registered nodes so
# :10250 gets a serving cert. Long-term rotation needs an external CSR approver.
ensure_kubelet_serving_certs() {
  local kc="$1"
  [[ -f "$kc" ]] || return 0
  command -v kubectl >/dev/null 2>&1 || return 0
  log "approve Pending kubelet-serving CSRs from registered nodes"
  local names
  names="$(kubectl --kubeconfig "$kc" get csr -o json 2>/dev/null | python3 -c '
import json,sys
try:
    data=json.load(sys.stdin)
except Exception:
    sys.exit(0)
for i in data.get("items") or []:
    spec=i.get("spec") or {}
    if spec.get("signerName") != "kubernetes.io/kubelet-serving":
        continue
    username=spec.get("username") or ""
    groups=spec.get("groups") or []
    if not username.startswith("system:node:") or "system:nodes" not in groups:
        continue
    types=[(c or {}).get("type") for c in (i.get("status") or {}).get("conditions") or []]
    if "Approved" in types or "Denied" in types:
        continue
    print((i.get("metadata", {}).get("name") or "") + ":" + username.removeprefix("system:node:"))
' 2>/dev/null || true)"
  local item csr node
  for item in $names; do
    csr="${item%%:*}"
    node="${item#*:}"
    [[ -n "$csr" && -n "$node" ]] || continue
    if ! kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; then
      log "WARNING: skip kubelet-serving CSR ${csr}; requester node ${node} is not registered"
      continue
    fi
    log "approve kubelet-serving CSR ${csr}"
    kubectl --kubeconfig "$kc" certificate approve "$csr" >/dev/null || true
  done
}

# Dump node / CSR / Ready condition hints when a wait fails.
diagnose_node_wait() {
  local kc="$1" node="$2"
  echo "---- diagnose node ${node} ----" >&2
  kubectl --kubeconfig "$kc" get nodes -o wide 2>&1 | sed 's/^/  /' >&2 || true
  if kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; then
    kubectl --kubeconfig "$kc" get node "$node" -o jsonpath='Ready={.status.conditions[?(@.type=="Ready")].status} reason={.status.conditions[?(@.type=="Ready")].reason} msg={.status.conditions[?(@.type=="Ready")].message}{"\n"}' 2>&1 \
      | sed 's/^/  /' >&2 || true
    kubectl --kubeconfig "$kc" describe node "$node" 2>&1 | grep -E 'Ready|NetworkUnavailable|Taints|InternalIP|Kubelet' | sed 's/^/  /' >&2 || true
  else
    echo "  node object missing (TLS bootstrap / CSR / endpoint?)" >&2
  fi
  kubectl --kubeconfig "$kc" get csr 2>&1 | sed 's/^/  /' >&2 || true
  echo "---- end diagnose ----" >&2
}

# Wait until the Node object exists (TLS bootstrap succeeded). Ready may need CNI.
wait_nodes_registered() {
  local kc="$1"
  shift
  local node deadline
  for node in "$@"; do
    log "waiting for node ${node} registered"
    deadline=$((SECONDS + BOOTSTRAP_TIMEOUT))
    until kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; do
      if (( SECONDS >= deadline )); then
        diagnose_node_wait "$kc" "$node"
        die "node ${node} not registered within timeout (check bootstrap-token Secret / kubelet logs / CSR)"
      fi
      sleep 5
    done
    log "node ${node} registered"
  done
}

wait_nodes_ready() {
  local kc="$1"
  shift
  local node deadline ready
  for node in "$@"; do
    log "waiting for node ${node} Ready"
    deadline=$((SECONDS + BOOTSTRAP_TIMEOUT))
    until true; do
      if kubectl --kubeconfig "$kc" get node "$node" >/dev/null 2>&1; then
        ready="$(kubectl --kubeconfig "$kc" get node "$node" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
        [[ "$ready" == "True" ]] && break
      fi
      if (( SECONDS >= deadline )); then
        diagnose_node_wait "$kc" "$node"
        die "node ${node} not Ready within timeout (check CNI / bootstrap-token / kubelet logs)"
      fi
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

# Rewrite cluster.endpoint in machine config YAML (in-place).
rewrite_machine_endpoint() {
  local file="$1" url="$2"
  [[ -f "$file" ]] || return 0
  python3 - "$file" "$url" <<'PY'
import sys
path, url = sys.argv[1], sys.argv[2]
text = open(path).read()
out = []
in_cluster = False
done = False
for line in text.splitlines(True):
    stripped = line.lstrip()
    indent = len(line) - len(stripped)
    if stripped.startswith("cluster:") and indent == 0:
        in_cluster = True
        out.append(line)
        continue
    if in_cluster and indent == 0 and stripped and not stripped.startswith("#"):
        in_cluster = False
    if in_cluster and not done and stripped.startswith("endpoint:"):
        pad = line[:indent]
        out.append(f"{pad}endpoint: {url}\n")
        done = True
        continue
    out.append(line)
open(path, "w").write("".join(out))
PY
}

# After VIP fallback, point all generated machine configs at a live CP API.
rewrite_cluster_out_endpoints() {
  local url="$1"
  local f
  for f in "$CLUSTER_OUT"/controlplane.yaml \
           "$CLUSTER_OUT"/controlplane-*.yaml \
           "$CLUSTER_OUT"/worker.yaml \
           "$CLUSTER_OUT"/worker-*.yaml; do
    [[ -f "$f" ]] || continue
    rewrite_machine_endpoint "$f" "$url"
  done
  log "rewrote cluster.endpoint → ${url} in ${CLUSTER_OUT} machine configs"
}

# Valid IPv4 (reject octets >255 like 10.1.1.270) or IPv6.
require_valid_ip() {
  local addr="$1" label="${2:-address}"
  [[ -n "$addr" ]] || return 0
  python3 - "$addr" "$label" <<'PY'
import ipaddress, sys
addr, label = sys.argv[1], sys.argv[2]
try:
    ipaddress.ip_address(addr)
except ValueError as e:
    raise SystemExit(f"ERROR: {label} {addr!r} is not a valid IP ({e})")
PY
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
    docker run --rm -v "${out_dir}:/work" alpine:3.22 \
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
  if [[ "${PROVIDER_KIND}" != "vsphere" && "${PROVIDER_KIND}" != "nutanix" && "${PROVIDER_KIND}" != "pertisk-vms" && ( -n "$STATIC_BASE" || -n "$STATIC_SUBNET" ) ]]; then
    [[ -n "$STATIC_GATEWAY" ]] || die "--static-base/--static-subnet requires --static-gateway"
    if [[ -n "$STATIC_SUBNET" ]]; then
      CREATE_ARGS+=(--static-subnet "$STATIC_SUBNET" --static-gateway "$STATIC_GATEWAY")
    else
      CREATE_ARGS+=(--static-base "$STATIC_BASE" --static-gateway "$STATIC_GATEWAY")
    fi
    [[ -n "$STATIC_NAMESERVER" ]] && CREATE_ARGS+=(--static-nameserver "$STATIC_NAMESERVER")
    [[ -n "$STATIC_EXCLUDE" ]] && CREATE_ARGS+=(--static-exclude "$STATIC_EXCLUDE")
  fi
  if [[ "$DUAL_STACK" == "1" ]]; then
    export DUAL_STACK=1 PERTISK_DUAL_STACK=1
  fi
  # Export auto-detected static IPs and gateway if set (from mgmt jobs.rs TCP scan).
  [[ -n "${PROXMOX_STATIC_IPS:-}" ]] && export PROXMOX_STATIC_IPS
  [[ -n "${PROXMOX_STATIC_GATEWAY:-}" ]] && export PROXMOX_STATIC_GATEWAY
  [[ -n "${PROXMOX_STATIC_NAMESERVER:-}" ]] && export PROXMOX_STATIC_NAMESERVER
  if [[ "${PROVIDER_KIND}" == "vsphere" ]]; then
    VSPHERE_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  elif [[ "${PROVIDER_KIND}" == "nutanix" ]]; then
    NUTANIX_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  elif [[ "${PROVIDER_KIND}" == "pertisk-vms" ]]; then
    PERTISK_VMS_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  else
    PROXMOX_DISK="$DISK" "$CREATE_VMS" "${CREATE_ARGS[@]}"
  fi
}

# Apply memory/cores/disk-gb to existing VMs (qm set + qm resize).
step_apply_vm_sizing() {
  if [[ "${PROVIDER_KIND}" == "vsphere" || "${PROVIDER_KIND}" == "nutanix" || "${PROVIDER_KIND}" == "pertisk-vms" ]]; then
    log "skip Proxmox qm sizing on ${PROVIDER_KIND} (recreate VMs with --cp-memory/--disk-gb instead)"
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
  # Build static IP map from pre-assigned IPs (if any).
  build_static_ips_map
  
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
  ensure_vip_usable "${VIP:-}" "IPv4"
  ensure_vip_usable "${VIP6:-}" "IPv6"
  log "CPs=${CP_IPS[*]} VIP=${VIP:-none} VIP6=${VIP6:-none} API_ENDPOINT=${API_ENDPOINT} workers=${WORKER_IPS[*]:-}"
  WORKER_HOSTS=()
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

# Fail before soft-reset/bootstrap when CNI tooling is missing (RHEL sudo PATH
# often hides /usr/local/bin — install into /usr/bin or fix PATH).
require_cni_tools() {
  case "$CNI" in
    cilium)
      command -v kubectl >/dev/null 2>&1 || die "kubectl required for CNI=cilium (install on mgmt before create)"
      if ! command -v helm >/dev/null 2>&1; then
        die "helm required for CNI=cilium (default). Install on mgmt before create:
  curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
  # RHEL: sudo ln -sf /usr/local/bin/helm /usr/bin/helm
Or recreate with cni=flannel."
      fi
      ;;
    flannel|calico)
      command -v kubectl >/dev/null 2>&1 || die "kubectl required for CNI=${CNI}"
      ;;
  esac
}

step_cluster() {
  ensure_pertiskctl
  require_cni_tools
  log "using pertiskctl=${CTL}"

  mkdir -p "$CLUSTER_OUT"
  log "gen config ${CLUSTER_NAME} https://${API_ENDPOINT}:6443 (controlplanes=${CONTROLPLANES} dual-stack=${DUAL_STACK})"
  local gen_args=(
    gen config "$CLUSTER_NAME" "https://${API_ENDPOINT}:6443"
    -o "$CLUSTER_OUT" -k "$K8S_VER" --controlplanes "$CONTROLPLANES"
    --pod-subnet "$POD_SUBNET" --service-subnet "$SERVICE_SUBNET"
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
    gen_args+=(--pod-cidr-ipv6 "$POD_SUBNET_IPV6" --service-cidr-ipv6 "$SERVICE_SUBNET_IPV6")
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
  # Reused disks / failed creates leave BOOTSTRAPPED with a stale advertise IP.
  assert_guest_identity "$CP_IP" "${CLUSTER_NAME}-cp-1"
  if guest_has_admin_kubeconfig "$CP_IP" && https_readyz "$CP_IP"; then
    log "CP1 already bootstrapped and https://${CP_IP}:6443/readyz ok — skip soft-reset/apply/bootstrap (resume)"
  else
    log "soft-reset CP1 @ ${CP_IP} before apply (clear leftover STATE)"
    if "$CTL" -e "${CP_IP}:50000" reset --force 2>&1; then
      CP_IP="$(wait_after_reset "$CP_VMID" "${CLUSTER_NAME}-cp-1" "$CP_IP")"
      CP_IPS[0]="$CP_IP"
      wait_api "$CP_IP"
    else
      log "WARNING: reset CP1 failed — continuing (fresh guests are fine)"
    fi
    log "apply controlplane → ${CP_IP}"
    apply_machine_yaml "$CP_IP" "$CLUSTER_OUT/controlplane.yaml" "$CP_VMID"
    # Apply only writes STATE + flags reload; give pertiskd a moment to start
    # containerd/kubelet as controlplane before the long bootstrap RPC.
    sleep 8
    wait_api "$CP_IP"

    log "bootstrap CP1 (advertise=${CP_IP}; waits for registry.k8s.io pulls + :6443, up to ~10m)"
    local boot_out boot_try=0
    while true; do
      boot_out="$("$CTL" -e "${CP_IP}:50000" bootstrap --advertise-address "$CP_IP" 2>&1)" && break
      echo "$boot_out" >&2
      boot_try=$((boot_try + 1))
      if echo "$boot_out" | grep -q 'No such file or directory'; then
        (( boot_try < 6 )) || die "bootstrap CP1 failed"
        log "config.yaml missing after apply (STATE race); re-apply and retry ${boot_try}/5"
        apply_machine_yaml "$CP_IP" "$CLUSTER_OUT/controlplane.yaml" "$CP_VMID"
        wait_api "$CP_IP"
        continue
      fi
      if echo "$boot_out" | grep -qiE 'transport error|connection reset|connection error|Unavailable|Broken pipe'; then
        (( boot_try < 8 )) || die "bootstrap CP1 failed"
        log "bootstrap RPC dropped (guest reboot/reload?); wait for API and retry ${boot_try}/7"
        if api_reachable "$CP_IP"; then
          wait_api_down "$CP_IP" 60
          sleep 3
        fi
        CP_IP="$(wait_ip "$CP_VMID" "${CLUSTER_NAME}-cp-1")"
        CP_IPS[0]="$CP_IP"
        wait_api "$CP_IP"
        continue
      fi
      die "bootstrap CP1 failed"
    done
    echo "$boot_out"
    if echo "$boot_out" | grep -q 'already=true'; then
      log "CP1 already bootstrapped — verifying apiserver on ${CP_IP}"
      if ! https_readyz "$CP_IP"; then
        die "CP1 already bootstrapped but https://${CP_IP}:6443/readyz failed.
STATE is likely leftover from a previous create (guest IP may have changed).
Destroy the VMs (or: pertiskctl -e ${CP_IP}:50000 reset --force) and recreate the cluster."
      fi
    fi
  fi
  log "bootstrap CP1 done"

  # Join additional control planes (stacked etcd + kube-vip).
  # Use C-style for: macOS `seq 2 1` counts down and would iterate wrongly.
  local i ip host cpyaml etcd_ep cvid
  etcd_ep="https://${CP_IP}:2379"
  for ((i = 2; i <= CONTROLPLANES; i++)); do
    ip="${CP_IPS[$((i - 1))]}"
    host="${CLUSTER_NAME}-cp-${i}"
    cvid=$((CP_VMID + i - 1))
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
    # Clear leftover BOOTSTRAPPED from a previous failed join on reused disks.
    # If this guest already joined (admin kubeconfig), skip reset — wiping it
    # leaves a zombie etcd member on CP1 and lab-up would lose quorum.
    assert_guest_identity "$ip" "$host"
    if guest_has_admin_kubeconfig "$ip"; then
      log "${host} already joined (admin kubeconfig) — skip soft-reset/apply; wait for /readyz then finalize"
      wait_https_readyz "$ip" "$JOIN_READYZ_WAIT" || true
    else
      log "soft-reset ${host} @ ${ip} before join (clear leftover STATE)"
      if "$CTL" -e "${ip}:50000" reset --force 2>&1; then
        ip="$(wait_after_reset "$cvid" "$host" "$ip")"
        CP_IPS[$((i - 1))]="$ip"
        wait_api "$ip"
      else
        log "WARNING: reset ${host} failed — continuing (fresh guests are fine)"
      fi
      log "apply + join-controlplane ${host} @ ${ip}"
      apply_machine_yaml "$ip" "$cpyaml" "$cvid"
      # apply reloads runtime; give Machine API a moment before the long join RPC
      sleep 5
      wait_api "$ip"
    fi
    log "waiting for CP1 etcd ${etcd_ep} (join RPC up to ~30m after soft-reset image pulls)"
    local join_try=0 join_err=""
    while true; do
      if join_err="$("$CTL" -e "${ip}:50000" join-controlplane --advertise-address "$ip" --etcd-endpoints "$etcd_ep" 2>&1)"; then
        printf '%s\n' "$join_err"
        break
      fi
      printf '%s\n' "$join_err" >&2
      join_try=$((join_try + 1))
      # Membership + kubeconfig are written before finalize. A 600s /readyz miss
      # is image-pull lag, not a failed join — wait for :6443 instead of re-RPC.
      if echo "$join_err" | grep -qiE 'finalize|readyz'; then
        log "join membership on disk for ${host}; waiting for :6443/readyz (up to ${JOIN_READYZ_WAIT}s)"
        if wait_https_readyz "$ip" "$JOIN_READYZ_WAIT"; then
          log "readyz ok after finalize timeout — continue (labels via kubectl later)"
          break
        fi
      fi
      if echo "$join_err" | grep -qiE 'too many learner'; then
        log "etcd still has an unpromoted learner (previous CP) — wait then retry MemberAdd"
        wait_https_readyz "$ip" 30 || true
        # Previous joiner may still be catching up; give it time before retry.
        sleep 15
      fi
      if (( join_try >= JOIN_TRIES )); then
        if guest_has_admin_kubeconfig "$ip" && https_readyz "$ip"; then
          log "WARNING: join-controlplane RPC still failing but ${ip}:6443/readyz is ok — continue (label via kubectl later)"
          break
        fi
        die "join-controlplane failed for ${host} after ${join_try} attempts"
      fi
      log "join-controlplane retry ${join_try}/${JOIN_TRIES} for ${host}..."
      # Transport / HTTP/2 drops (guest panic, NIC blip) — retry the RPC quickly.
      # Do not burn JOIN_READYZ_WAIT on :6443; this node has not joined yet.
      if echo "$join_err" | grep -qiE 'h2 protocol|http2 error|transport error|stream no longer needed'; then
        sleep 3
        wait_api "$ip"
        continue
      fi
      wait_https_readyz "$ip" "$JOIN_READYZ_WAIT" || true
      wait_api "$ip"
    done
    # etcd allows only one learner. Do not MemberAdd the next CP until this
    # joiner is a voting member (local apiserver /readyz).
    log "waiting for ${host} :6443/readyz before next control-plane (one etcd learner at a time)"
    if ! wait_https_readyz "$ip" "$JOIN_READYZ_WAIT"; then
      die "${host} joined but :6443/readyz never came up — etcd learner was not promoted.
Joining another control-plane would fail with 'too many learner members'.
Check registry.k8s.io pulls / static pods on ${ip}."
    fi
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
  ensure_kubelet_serving_certs "$CLUSTER_OUT/admin.conf"

  # Install cluster CNI (cilium default) BEFORE waiting for Node Ready or joining
  # workers — machine config uses cni:none, so kubelet stays NotReady until CNI
  # writes /etc/cni/net.d.
  step_cni

  # Wait for all CP nodes Ready before role ensure (CP3 registers late).
  local cp_hosts=()
  for ((i = 1; i <= CONTROLPLANES; i++)); do
    cp_hosts+=("${CLUSTER_NAME}-cp-${i}")
  done
  wait_nodes_ready "$CLUSTER_OUT/admin.conf" "${cp_hosts[@]}"
  ensure_control_plane_roles "$CLUSTER_OUT/admin.conf" "$CONTROLPLANES"
  ensure_kubelet_serving_certs "$CLUSTER_OUT/admin.conf"

  WORKER_HOSTS=()
  local wyaml wvid i ip host
  for i in $(seq 1 "$WORKERS"); do
    ip="${WORKER_IPS[$((i - 1))]}"
    host="${CLUSTER_NAME}-wk-${i}"
    wvid=$((CP_VMID + CONTROLPLANES + i - 1))
    wyaml="${CLUSTER_OUT}/worker-${i}.yaml"
    set_hostname_yaml "$CLUSTER_OUT/worker.yaml" "$wyaml" "$host"
    wait_api "$ip"
    # Clear leftover kubelet/bootstrap STATE on reused disks (same as joining CPs).
    assert_guest_identity "$ip" "$host"
    log "soft-reset ${host} @ ${ip} before join (clear leftover STATE)"
    if "$CTL" -e "${ip}:50000" reset --force 2>&1; then
      ip="$(wait_after_reset "$wvid" "$host" "$ip")"
      WORKER_IPS[$((i - 1))]="$ip"
      wait_api "$ip"
    else
      log "WARNING: reset ${host} failed — continuing (fresh guests are fine)"
    fi
    log "join worker ${host} @ ${ip}"
    apply_machine_yaml "$ip" "$wyaml" "$wvid"
    # apply reloads kubelet; give TLS bootstrap a moment before the wait loop
    sleep 5
    wait_api "$ip"
    WORKER_HOSTS+=("$host")
  done
  if ((${#WORKER_HOSTS[@]} > 0)); then
    # Node object = TLS bootstrap OK. Ready often needs cluster CNI (cni:none).
    wait_nodes_registered "$CLUSTER_OUT/admin.conf" "${WORKER_HOSTS[@]}"
    ensure_worker_roles "$CLUSTER_OUT/admin.conf" "$WORKERS"
    ensure_kubelet_serving_certs "$CLUSTER_OUT/admin.conf"
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
  rewrite_cluster_out_endpoints "https://${cp_ip}:6443"
  curl -sk --connect-timeout 2 "https://${API_ENDPOINT}:6443/readyz" >/dev/null 2>&1 \
    || die "apiserver still unreachable after fallback to ${cp_ip}:6443"
}

# Before HA create: VIP must not be a guest DHCP lease or another host on the LAN.
# If it is, pick a free address in the same /24 (IPv4) or /64 (IPv6) and continue
# so --skip-vms resume does not require destroy/recreate.
require_vip_free() {
  ensure_vip_usable "$1" "${2:-VIP}"
}

# $1 = address (may be empty). $2 = "IPv4" | "IPv6" | label.
ensure_vip_usable() {
  local vip="$1" family="${2:-VIP}" reason="" cand="" used="" ip
  [[ -n "$vip" ]] || return 0
  require_valid_ip "$vip" "$family VIP"
  for ip in "${CP_IPS[@]:-}" "${WORKER_IPS[@]:-}"; do
    [[ -n "${ip:-}" && "$ip" == "$vip" ]] && reason="guest DHCP ${ip}" && break
  done
  # Keep the operator VIP even if leftover kube-vip still answers ICMP/:6443.
  # Only move it when a cluster node already owns that address.
  if [[ -z "$reason" ]]; then
    if ping -c 1 -W 1 "$vip" >/dev/null 2>&1 \
      || ping -c 1 -W 1 -6 "$vip" >/dev/null 2>&1; then
      log "${family} VIP ${vip} answers ICMP (keeping operator VIP)"
    else
      log "${family} VIP ${vip} looks free (still keep it outside the DHCP pool)"
    fi
    return 0
  fi
  used=""
  for ip in "${CP_IPS[@]:-}" "${WORKER_IPS[@]:-}" "$vip"; do
    [[ -n "${ip:-}" ]] && used+="${used:+,}${ip}"
  done
  cand="$(pick_free_vip "$vip" "$used" || true)"
  [[ -n "$cand" ]] || die "${family} VIP ${vip} is busy (${reason}) and no free replacement was found.
Guest IPs: CPs=${CP_IPS[*]:-none} workers=${WORKER_IPS[*]:-none}
Resume with --vip <free-ip> --skip-build --skip-vms --cp-vmid ${CP_VMID} --controlplanes ${CONTROLPLANES} --workers ${WORKERS}"
  if [[ "$vip" == *:* ]]; then
    log "VIP reassigned IPv6 ${vip} -> ${cand} (${reason})"
    VIP6="$cand"
  else
    log "VIP reassigned IPv4 ${vip} -> ${cand} (${reason})"
    VIP="$cand"
  fi
  if [[ "$CONTROLPLANES" -gt 1 ]]; then
    API_ENDPOINT="${VIP:-$VIP6}"
  fi
}

# Print a free address in the same IPv4 /24 or IPv6 /64 as $1, skipping $2 (csv) and ICMP-live hosts.
pick_free_vip() {
  local vip="$1" used_csv="${2:-}"
  python3 - "$vip" "$used_csv" <<'PY'
import ipaddress, subprocess, sys

vip = ipaddress.ip_address(sys.argv[1])
used = set()
for raw in (sys.argv[2] or "").split(","):
    raw = raw.strip()
    if not raw:
        continue
    try:
        used.add(ipaddress.ip_address(raw))
    except ValueError:
        pass

def ping(ip):
    cmd = ["ping", "-c", "1", "-W", "1", str(ip)]
    if ip.version == 6:
        cmd = ["ping", "-c", "1", "-W", "1", "-6", str(ip)]
    return subprocess.call(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0

def locals_on_box():
    found = set()
    try:
        out = subprocess.check_output(["ip", "-o", "addr", "show"], text=True, stderr=subprocess.DEVNULL)
    except (OSError, subprocess.CalledProcessError):
        return found
    for line in out.splitlines():
        parts = line.split()
        for i, p in enumerate(parts):
            if p in ("inet", "inet6") and i + 1 < len(parts):
                try:
                    found.add(ipaddress.ip_interface(parts[i + 1].split("%")[0]).ip)
                except ValueError:
                    pass
    return found

used |= locals_on_box()
if vip.version == 4:
    net = ipaddress.ip_interface(f"{vip}/24").network
    hosts = [h for h in net.hosts() if int(h) & 0xFF != 1]
    candidates = list(reversed(hosts))
else:
    net = ipaddress.ip_interface(f"{vip}/64").network
    base = int(net.network_address)
    candidates = [ipaddress.ip_address(base + i) for i in range(0xFE, 1, -1)]

for cand in candidates:
    if cand in used or cand == vip:
        continue
    if ping(cand):
        continue
    print(cand)
    raise SystemExit(0)
raise SystemExit(1)
PY
}

warn_if_vip_busy() {
  require_vip_free "$@"
}

step_cni() {
  local kc="$CLUSTER_OUT/admin.conf"
  local cni_dir=""
  ensure_api_endpoint_reachable
  case "$CNI" in
    cilium)
      command -v kubectl >/dev/null || die "kubectl required for CNI=cilium"
      if ! command -v helm >/dev/null 2>&1; then
        die "helm required for CNI=cilium (default). Install on mgmt, or use cni=flannel.
  curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash"
      fi
      log "install Cilium first (kubernetes IPAM + kubeProxyReplacement + Hubble; dual-stack=${DUAL_STACK})"
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
    flannel)
      command -v kubectl >/dev/null || die "kubectl required for CNI=flannel"
      cni_dir="$(resolve_cni_examples_dir)" || die_missing_cni_examples
      [[ -f "${cni_dir}/kube-flannel.yaml" ]] || die "missing ${cni_dir}/kube-flannel.yaml"
      [[ -f "${cni_dir}/kube-proxy.yaml" ]] || die "missing CNI config template: ${cni_dir}/kube-proxy.yaml"
      install_kube_proxy "$kc" "$cni_dir"
      log "install Flannel from ${cni_dir}/kube-flannel.yaml"
      kubectl --kubeconfig "$kc" apply -f "${cni_dir}/kube-flannel.yaml"
      # Reach apiserver before ClusterIP works (kube-proxy may still be syncing).
      kubectl --kubeconfig "$kc" -n kube-flannel set env ds/kube-flannel-ds \
        KUBERNETES_SERVICE_HOST="${API_ENDPOINT}" \
        KUBERNETES_SERVICE_PORT=6443
      kubectl --kubeconfig "$kc" -n kube-flannel rollout status ds/kube-flannel-ds --timeout=5m 2>/dev/null \
        || echo "WARNING: flannel DS not Ready yet; check: kubectl --kubeconfig $kc -n kube-flannel get pods" >&2
      ;;
    calico)
      command -v kubectl >/dev/null || die "kubectl required for CNI=calico"
      command -v curl >/dev/null || die "curl required for CNI=calico"
      cni_dir="$(resolve_cni_examples_dir)" || die_missing_cni_examples
      [[ -f "${cni_dir}/kube-proxy.yaml" ]] || die "missing CNI config template: ${cni_dir}/kube-proxy.yaml"
      install_kube_proxy "$kc" "$cni_dir"
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
    none)
      log "CNI=none — skip (nodes stay NotReady until you install a cluster CNI)"
      ;;
    *)
      die "unknown CNI=$CNI (use cilium|calico|flannel|none)"
      ;;
  esac
}

resolve_cni_examples_dir() {
  local d
  for d in \
    "${ROOT}/examples/cni" \
    "/usr/share/pertisk-mgmt/examples/cni" \
    "${PERTISK_EXAMPLES_DIR:-}/cni"; do
    [[ -n "$d" && -d "$d" ]] || continue
    if [[ -f "${d}/kube-proxy.yaml" || -f "${d}/kube-flannel.yaml" ]]; then
      echo "$d"
      return 0
    fi
  done
  return 1
}

die_missing_cni_examples() {
  die "no CNI config templates under ${ROOT}/examples/cni (PERTISK_ROOT=${ROOT}).
  Expected /usr/share/pertisk-mgmt/examples/cni — redeploy mgmt RPM, or:
    scp -r examples/cni root@mgmt:/usr/share/pertisk-mgmt/examples/"
}

# After cluster CNI is installed, workers (cni:none) can become Ready.
step_workers_ready() {
  [[ ${#WORKER_HOSTS[@]} -gt 0 ]] || return 0
  local kc="$CLUSTER_OUT/admin.conf"
  ensure_api_endpoint_reachable
  log "waiting for workers Ready after CNI=${CNI}"
  wait_nodes_ready "$kc" "${WORKER_HOSTS[@]}"
  ensure_worker_roles "$kc" "$WORKERS"
  ensure_kubelet_serving_certs "$kc"
}

# kube-proxy for Flannel/Calico (Cilium uses kubeProxyReplacement instead).
install_kube_proxy() {
  local kc="$1"
  local cni_dir="${2:-}"
  [[ -n "$cni_dir" ]] || cni_dir="$(resolve_cni_examples_dir)" || die_missing_cni_examples
  local src="${cni_dir}/kube-proxy.yaml"
  [[ -f "$src" ]] || die "missing CNI config template: $src"
  log "install kube-proxy (apiserver ${API_ENDPOINT}:6443) from $src"
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
  log "ensure CoreDNS (kube-dns 10.96.0.10) — after CNI (cni:none defers this past bootstrap)"
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
# CNI (cilium first) runs inside step_cluster right after apiserver is ready.
step_workers_ready
step_dns
step_addons
step_apps
step_summary
