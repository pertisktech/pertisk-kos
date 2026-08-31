#!/usr/bin/env bash
# Create N control-plane + M worker VMs on Proxmox from a Pertisk cloud qcow2,
# then by default continue into lab-up (DHCP IPs → bootstrap → join → CNI).
#
# Auth: same as proxmox-upload-vm.sh (PROXMOX_URL, PROXMOX_TOKEN_*, …).
# If PROXMOX_URL is unset, loads assignments from ./proxmox.sh (skips its `exec`).
#
# Examples:
#   ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
#   ./scripts/proxmox-create-cluster-vms.sh --arch arm64 --cp-vmid 210 --workers 2
#   ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --controlplanes 3 --workers 2 --no-lab-up
#   CNI=cilium ./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UPLOAD="${ROOT}/scripts/proxmox-upload-vm.sh"
LAB_UP_SH="${ROOT}/scripts/proxmox-lab-up.sh"
# shellcheck source=pertisk-parallel.sh
. "$(cd "$(dirname "$0")" && pwd)/pertisk-parallel.sh"

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

# Defaults: shared --memory/--cores/--disk-gb apply to both roles; role-specific flags override.
MEMORY="${PROXMOX_MEMORY:-4096}"
CORES="${PROXMOX_CORES:-2}"
CP_MEMORY="${PROXMOX_CP_MEMORY:-}"
CP_CORES="${PROXMOX_CP_CORES:-}"
WORKER_MEMORY="${PROXMOX_WORKER_MEMORY:-}"
WORKER_CORES="${PROXMOX_WORKER_CORES:-}"
DISK_GB="${PROXMOX_DISK_GB:-}"
CP_DISK_GB="${PROXMOX_CP_DISK_GB:-}"
WORKER_DISK_GB="${PROXMOX_WORKER_DISK_GB:-}"
CP_VMID="${CP_VMID:-210}"
CONTROLPLANES="${CONTROLPLANES:-1}"
WORKERS="${WORKERS:-2}"
NAME_PREFIX="${NAME_PREFIX:-pertisk}"
# Static IPs (Talos-style, no DHCP): --static-base is cp-1's address (e.g.
# 10.1.1.111/24); each following node (cp-2.. then wk-1..) gets base+1, +2, ….
# Requires --static-gateway. Skips addresses that answer ICMP (already used).
# --static-subnet scans a CIDR (e.g. 10.1.1.0/24) for free addresses instead
# of requiring a manually-picked base — use this when you don't know a safe one.
STATIC_BASE="${PROXMOX_STATIC_BASE:-}"
STATIC_SUBNET="${PROXMOX_STATIC_SUBNET:-}"
STATIC_GATEWAY="${PROXMOX_STATIC_GATEWAY:-}"
STATIC_NAMESERVER="${PROXMOX_STATIC_NAMESERVER:-}"
# Comma-separated IPs to always skip (e.g. a Nutanix CVM sharing this LAN) even
# if it does not answer ICMP.
STATIC_EXCLUDE="${PROXMOX_STATIC_EXCLUDE:-}"
# Space-separated static IPs (e.g. "10.1.1.2/24 10.1.1.3/24 …") from auto-detection or operator override.
STATIC_IPS_ENV="${PROXMOX_STATIC_IPS:-}"
# Guest arch: ARCH / PERTISK_ARCH (amd64|arm64). Default amd64.
ARCH="${PERTISK_ARCH:-${ARCH:-amd64}}"
case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported ARCH=${ARCH} (use amd64|arm64)" >&2; exit 1 ;;
esac
export PERTISK_ARCH="$ARCH"
DISK="${PROXMOX_DISK:-${ROOT}/out/pertisk-cloud-${ARCH}.qcow2}"
CP_DISK="${PROXMOX_CP_DISK:-}"
WORKER_DISK="${PROXMOX_WORKER_DISK:-}"
# Chain into bootstrap/join/CNI after VMs exist (disable with --no-lab-up).
DO_LAB_UP=1
LAB_UP_EXTRA=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cp-vmid) CP_VMID="$2"; shift 2 ;;
    --controlplanes) CONTROLPLANES="$2"; shift 2 ;;
    --workers) WORKERS="$2"; shift 2 ;;
    --prefix) NAME_PREFIX="$2"; shift 2 ;;
    --arch) ARCH="$2"; PERTISK_ARCH="$2"; shift 2 ;;
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
    --lab-up) DO_LAB_UP=1; shift ;;
    --no-lab-up) DO_LAB_UP=0; shift ;;
    --cni)
      LAB_UP_EXTRA+=(--cni "$2")
      shift 2
      ;;
    --vip)
      LAB_UP_EXTRA+=(--vip "$2")
      shift 2
      ;;
    --subnet)
      LAB_UP_EXTRA+=(--subnet "$2")
      shift 2
      ;;
    --static-base) STATIC_BASE="$2"; shift 2 ;;
    --static-subnet) STATIC_SUBNET="$2"; shift 2 ;;
    --static-gateway) STATIC_GATEWAY="$2"; shift 2 ;;
    --static-nameserver) STATIC_NAMESERVER="$2"; shift 2 ;;
    --static-exclude) STATIC_EXCLUDE="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,16p' "$0"
      cat <<EOF

Sizing (prefer role-sized qcow2 via --cp-disk/--worker-disk; --*-disk-gb only grows):
  --arch amd64|arm64                   guest arch (default ${ARCH}; env ARCH/PERTISK_ARCH)
  --memory MB / --cores N              defaults for both roles
  --cp-memory / --cp-cores             control-plane
  --worker-memory / --worker-cores     workers
  --disk PATH                          default qcow2 for both roles
  --cp-disk / --worker-disk PATH       per-role qcow2 (lab-up builds *-Ng.qcow2)
  --disk-gb N                          grow scsi0 after import (env PROXMOX_DISK_GB)
  --cp-disk-gb N                       control-plane grow GiB (env PROXMOX_CP_DISK_GB)
  --worker-disk-gb N                   worker grow GiB (env PROXMOX_WORKER_DISK_GB)

Static IPs (no DHCP; stable across reboot/shutdown):
  --static-base IP/PREFIX               cp-1 address (e.g. 10.1.1.111/24); each
                                         later node gets +1 (env PROXMOX_STATIC_BASE)
  --static-subnet CIDR                  scan this subnet (e.g. 10.1.1.0/24) for free
                                         addresses instead of a manual base
                                         (env PROXMOX_STATIC_SUBNET)
  --static-gateway IP                    required with --static-base/--static-subnet
  --static-nameserver IP                 default: gateway
  --static-exclude IP[,IP...]            always skip these IPs (e.g. a Nutanix
                                         CVM), even if they don't answer ICMP
                                         (env PROXMOX_STATIC_EXCLUDE)

arm64: uses machine=virt + arch=aarch64 (AAVMF). Build: make cloud ARCH=arm64
EOF
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64 ;;
  arm64|aarch64) ARCH=arm64 ;;
  *) echo "unsupported ARCH=${ARCH}" >&2; exit 1 ;;
esac
export PERTISK_ARCH="$ARCH"
# If --disk was not set and still points at wrong-arch default, re-resolve.
if [[ -z "${PROXMOX_DISK:-}" && "$DISK" == *pertisk-cloud-* && "$DISK" != *"pertisk-cloud-${ARCH}"* ]]; then
  DISK="${ROOT}/out/pertisk-cloud-${ARCH}.qcow2"
fi

CP_MEMORY="${CP_MEMORY:-$MEMORY}"
CP_CORES="${CP_CORES:-$CORES}"
WORKER_MEMORY="${WORKER_MEMORY:-$MEMORY}"
WORKER_CORES="${WORKER_CORES:-$CORES}"
CP_DISK_GB="${CP_DISK_GB:-$DISK_GB}"
WORKER_DISK_GB="${WORKER_DISK_GB:-$DISK_GB}"
CP_DISK="${CP_DISK:-$DISK}"
WORKER_DISK="${WORKER_DISK:-$DISK}"

if [[ -z "${PROXMOX_URL:-}" ]]; then
  echo "PROXMOX_URL unset. Copy proxmox.sh.example → proxmox.sh and fill token, or export PROXMOX_*." >&2
  exit 1
fi

if [[ ! -f "$CP_DISK" ]]; then
  echo "CP disk not found: $CP_DISK (build with: make cloud ARCH=${ARCH} / lab-up without --skip-build)" >&2
  exit 1
fi
if [[ ! -f "$WORKER_DISK" ]]; then
  echo "worker disk not found: $WORKER_DISK (build with: make cloud ARCH=${ARCH} / lab-up without --skip-build)" >&2
  exit 1
fi
if [[ ! -x "$UPLOAD" ]]; then
  chmod +x "$UPLOAD" || true
fi

if [[ "$CONTROLPLANES" -lt 1 ]]; then
  echo "ERROR: --controlplanes must be >= 1" >&2
  exit 1
fi
if [[ -n "$STATIC_BASE" && -z "$STATIC_GATEWAY" ]]; then
  echo "ERROR: --static-base requires --static-gateway" >&2
  exit 1
fi
if [[ -n "$STATIC_SUBNET" && -z "$STATIC_GATEWAY" ]]; then
  echo "ERROR: --static-subnet requires --static-gateway" >&2
  exit 1
fi
if [[ -n "$STATIC_BASE" && -n "$STATIC_SUBNET" ]]; then
  echo "ERROR: use --static-base or --static-subnet, not both" >&2
  exit 1
fi

# Print base_ip + offset within its /prefix, skipping ICMP-live and excluded addresses.
static_ip_at() {
  local base="$1" offset="$2"
  python3 - "$base" "$offset" "$STATIC_EXCLUDE" <<'PY'
import ipaddress, subprocess, sys
iface = ipaddress.ip_interface(sys.argv[1])
offset = int(sys.argv[2])
excluded = {ipaddress.ip_address(s.strip()) for s in sys.argv[3].split(",") if s.strip()}
net = iface.network
cand = ipaddress.ip_address(int(iface.ip) + offset)
if cand not in net:
    print(f"static IP offset out of subnet: {cand}", file=sys.stderr)
    raise SystemExit(1)
if cand in excluded:
    print(f"static IP is in --static-exclude: {cand}", file=sys.stderr)
    raise SystemExit(1)
if subprocess.call(["ping", "-c", "1", "-W", "1", str(cand)],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0:
    print(f"static IP already in use (answers ICMP): {cand}", file=sys.stderr)
    raise SystemExit(1)
print(f"{cand}/{net.prefixlen}")
PY
}

# Scan a CIDR for $2 free addresses (skip network/gateway/broadcast, excluded
# IPs, and any host that answers ICMP), print one "ip/prefix" per line.
scan_free_ips() {
  local cidr="$1" count="$2"
  python3 - "$cidr" "$count" "$STATIC_EXCLUDE" <<'PY'
import ipaddress, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

net = ipaddress.ip_network(sys.argv[1], strict=False)
count = int(sys.argv[2])
excluded = {ipaddress.ip_address(s.strip()) for s in sys.argv[3].split(",") if s.strip()}
gateway = net.network_address + 1

def alive(ip):
    return subprocess.call(
        ["ping", "-c", "1", "-W", "1", str(ip)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ) == 0

candidates = [h for h in net.hosts() if h != gateway and h not in excluded]
with ThreadPoolExecutor(max_workers=32) as ex:
    results = list(zip(candidates, ex.map(alive, candidates)))

free = [str(ip) for ip, used in results if not used]
if len(free) < count:
    print(f"only {len(free)} free address(es) in {net} (need {count}, excluded={sorted(str(e) for e in excluded)})", file=sys.stderr)
    raise SystemExit(1)
for ip in free[:count]:
    print(f"{ip}/{net.prefixlen}")
PY
}

echo "==> Proxmox create-cluster arch=${ARCH} prefix=${NAME_PREFIX}"

pertisk_parallel_init

# Parse static IPs from env (space-separated list for all nodes) or CLI flags.
# Priority: PROXMOX_STATIC_IPS (auto-detected or operator-set) > CLI --static-subnet/--static-base
STATIC_IPS_ARRAY=()
if [[ -n "$STATIC_IPS_ENV" ]]; then
  read -ra STATIC_IPS_ARRAY <<<"$STATIC_IPS_ENV"
  echo "==> using auto-detected static IPs: ${STATIC_IPS_ARRAY[*]} gateway=${STATIC_GATEWAY:-<unset>}"
  if [[ -z "$STATIC_GATEWAY" ]]; then
    echo "ERROR: PROXMOX_STATIC_IPS requires PROXMOX_STATIC_GATEWAY to be set" >&2
    exit 1
  fi
elif [[ -n "$STATIC_SUBNET" ]]; then
  total=$((CONTROLPLANES + WORKERS))
  echo "==> scanning ${STATIC_SUBNET} for ${total} free static IP(s)"
  mapfile -t STATIC_IPS_ARRAY < <(scan_free_ips "$STATIC_SUBNET" "$total") || exit 1
  echo "    found: ${STATIC_IPS_ARRAY[*]}"
elif [[ -n "$STATIC_BASE" ]]; then
  # Will be computed per-VM below
  :
fi
STATIC_IP_IDX=0

for i in $(seq 1 "$CONTROLPLANES"); do
  cvid=$((CP_VMID + i - 1))
  echo "==> control-plane VMID=${cvid} name=${NAME_PREFIX}-cp-${i} disk=${CP_DISK} mem=${CP_MEMORY} cores=${CP_CORES} disk-gb=${CP_DISK_GB:-image}"
  UPLOAD_ARGS=(--vmid "$cvid" --name "${NAME_PREFIX}-cp-${i}" --disk "$CP_DISK" --arch "$ARCH" --memory "$CP_MEMORY" --cores "$CP_CORES")
  [[ -n "$CP_DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$CP_DISK_GB")
  # Prefer env PROXMOX_STATIC_IPS; else use CLI static-subnet/static-base flags
  if [[ $STATIC_IP_IDX -lt ${#STATIC_IPS_ARRAY[@]} ]]; then
    ip="${STATIC_IPS_ARRAY[$STATIC_IP_IDX]}"
    UPLOAD_ARGS+=(--ip "$ip" --gateway "$STATIC_GATEWAY")
    [[ -n "$STATIC_NAMESERVER" ]] && UPLOAD_ARGS+=(--nameserver "$STATIC_NAMESERVER")
    echo "    static ip=${ip}"
    STATIC_IP_IDX=$((STATIC_IP_IDX + 1))
  elif [[ -n "$STATIC_BASE" ]]; then
    ip="$(static_ip_at "$STATIC_BASE" $((i - 1)))" || exit 1
    UPLOAD_ARGS+=(--ip "$ip" --gateway "$STATIC_GATEWAY")
    [[ -n "$STATIC_NAMESERVER" ]] && UPLOAD_ARGS+=(--nameserver "$STATIC_NAMESERVER")
    echo "    static ip=${ip}"
  fi
  pertisk_parallel_add "${NAME_PREFIX}-cp-${i}" "$UPLOAD" "${UPLOAD_ARGS[@]}"
done

for i in $(seq 1 "$WORKERS"); do
  wvid=$((CP_VMID + CONTROLPLANES + i - 1))
  echo "==> worker VMID=${wvid} name=${NAME_PREFIX}-wk-${i} disk=${WORKER_DISK} mem=${WORKER_MEMORY} cores=${WORKER_CORES} disk-gb=${WORKER_DISK_GB:-image}"
  UPLOAD_ARGS=(--vmid "$wvid" --name "${NAME_PREFIX}-wk-${i}" --disk "$WORKER_DISK" --arch "$ARCH" --memory "$WORKER_MEMORY" --cores "$WORKER_CORES")
  [[ -n "$WORKER_DISK_GB" ]] && UPLOAD_ARGS+=(--disk-gb "$WORKER_DISK_GB")
  # Prefer env PROXMOX_STATIC_IPS; else use CLI static-subnet/static-base flags
  if [[ $STATIC_IP_IDX -lt ${#STATIC_IPS_ARRAY[@]} ]]; then
    ip="${STATIC_IPS_ARRAY[$STATIC_IP_IDX]}"
    UPLOAD_ARGS+=(--ip "$ip" --gateway "$STATIC_GATEWAY")
    [[ -n "$STATIC_NAMESERVER" ]] && UPLOAD_ARGS+=(--nameserver "$STATIC_NAMESERVER")
    echo "    static ip=${ip}"
    STATIC_IP_IDX=$((STATIC_IP_IDX + 1))
  elif [[ -n "$STATIC_BASE" ]]; then
    ip="$(static_ip_at "$STATIC_BASE" $((CONTROLPLANES + i - 1)))" || exit 1
    UPLOAD_ARGS+=(--ip "$ip" --gateway "$STATIC_GATEWAY")
    [[ -n "$STATIC_NAMESERVER" ]] && UPLOAD_ARGS+=(--nameserver "$STATIC_NAMESERVER")
    echo "    static ip=${ip}"
  fi
  pertisk_parallel_add "${NAME_PREFIX}-wk-${i}" "$UPLOAD" "${UPLOAD_ARGS[@]}"
done
pertisk_parallel_wait

echo "==> VMs created (CP=${CP_VMID}..$((CP_VMID + CONTROLPLANES - 1)), workers=${WORKERS})"

if [[ "$DO_LAB_UP" != "1" ]]; then
  cat <<EOF

Stopped after VM create (--no-lab-up). Continue with:
  ./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cp-vmid ${CP_VMID} \\
    --controlplanes ${CONTROLPLANES} --workers ${WORKERS}
EOF
  exit 0
fi

echo "==> continuing → lab-up (IPs → cluster → CNI)"
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
