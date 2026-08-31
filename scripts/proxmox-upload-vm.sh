#!/usr/bin/env bash
# Upload a Pertisk cloud qcow2 to Proxmox and create a UEFI VM.
#
# Arch (amd64 | arm64):
#   --arch / PERTISK_ARCH / ARCH, or inferred from disk name (*-arm64* / *-amd64*).
#   amd64 → machine=q35, bios=ovmf (OVMF)
#   arm64 → arch=aarch64, machine=virt, bios=ovmf (AAVMF; PVE arm64 or aarch64 guest)
#
# Auth — API token only (never put root passwords here):
#   export PROXMOX_URL="https://proxmox.example:8006"
#   export PROXMOX_TOKEN_ID="root@pam!pertisk"
#   export PROXMOX_TOKEN_SECRET="…"
#   export PROXMOX_NODE="pve"
#   export PROXMOX_STORAGE="local"          # directory storage recommended for upload
#   export PROXMOX_INSECURE=1              # lab self-signed TLS
#
# Optional SSH helper (most reliable disk import on the node):
#   export PROXMOX_SSH="root@proxmox.example"   # key-based SSH
#
#   ./scripts/proxmox-upload-vm.sh --vmid 9100 --name pertisk-worker-1 \
#     --disk out/pertisk-cloud-amd64.qcow2
#   ARCH=arm64 ./scripts/proxmox-upload-vm.sh --vmid 9200 --name lab-cp-1 \
#     --disk out/pertisk-cloud-arm64.qcow2
set -euo pipefail

VMID=""
NAME="pertisk-worker"
DISK=""
MEMORY="${PROXMOX_MEMORY:-4096}"
CORES="${PROXMOX_CORES:-2}"
DISK_GB="${PROXMOX_DISK_GB:-}"
BRIDGE="vmbr0"
START=1
STORAGE="${PROXMOX_STORAGE:-local}"
NODE="${PROXMOX_NODE:-}"
ARCH="${PERTISK_ARCH:-${ARCH:-}}"
STATIC_IP="${PROXMOX_STATIC_IP:-}"
STATIC_GATEWAY="${PROXMOX_STATIC_GATEWAY:-}"
STATIC_NAMESERVER="${PROXMOX_STATIC_NAMESERVER:-}"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/proxmox-upload-vm.sh --vmid ID --disk PATH [options]

Options:
  --vmid N          VM id (required)
  --disk PATH       qcow2 path (required)
  --name NAME       VM name (default pertisk-worker)
  --arch ARCH       amd64|arm64 (default: from env / disk name / amd64)
  --memory MB       RAM (default 4096; env PROXMOX_MEMORY)
  --cores N         vCPUs (default 2; env PROXMOX_CORES)
  --disk-gb N       grow scsi0 to N GiB after import (env PROXMOX_DISK_GB)
  --bridge NAME     bridge (default vmbr0)
  --storage NAME    datastore (default $PROXMOX_STORAGE or local)
  --node NAME       node (default $PROXMOX_NODE)
  --no-start        do not start after create
  --ip CIDR         static IPv4 (e.g. 10.1.1.111/24); requires --gateway.
                    Written to a small netcfg disk (PERTISK-NET) so the guest
                    never DHCPs and the address survives reboot/shutdown
                    (env PROXMOX_STATIC_IP)
  --gateway IP      static gateway for --ip (env PROXMOX_STATIC_GATEWAY)
  --nameserver IP   static DNS for --ip (default: gateway; env PROXMOX_STATIC_NAMESERVER)
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vmid) VMID="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --bridge) BRIDGE="$2"; shift 2 ;;
    --storage) STORAGE="$2"; shift 2 ;;
    --node) NODE="$2"; shift 2 ;;
    --no-start) START=0; shift ;;
    --ip) STATIC_IP="$2"; shift 2 ;;
    --gateway) STATIC_GATEWAY="$2"; shift 2 ;;
    --nameserver) STATIC_NAMESERVER="$2"; shift 2 ;;
    -h | --help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "${VMID}" && -n "${DISK}" ]] || usage
if [[ -n "${STATIC_IP}" && -z "${STATIC_GATEWAY}" ]]; then
  echo "--ip requires --gateway (or PROXMOX_STATIC_GATEWAY)" >&2
  exit 1
fi
[[ -f "${DISK}" ]] || {
  echo "disk not found: ${DISK}" >&2
  exit 1
}

# Stable MAC in Proxmox OUI space (survives recreate on the same host).
# Mix a host salt into the first octet so the same VMID on two Proxmox nodes
# that share a LAN (e.g. amd64 lab + arm64 lab) do not collide on DHCP.
# Salt: PROXMOX_MAC_SALT > PROXMOX_NODE > PVE host from PROXMOX_URL.
mac_for_vmid() {
  local id="$1"
  local salt_src="${PROXMOX_MAC_SALT:-${PROXMOX_NODE:-}}"
  if [[ -z "$salt_src" && -n "${PROXMOX_URL:-}" ]]; then
    salt_src="$(printf '%s' "$PROXMOX_URL" | sed -E 's|https?://([^/:]+).*|\1|')"
  fi
  local salt=0
  if [[ -n "$salt_src" ]]; then
    salt=$(( $(printf '%s' "$salt_src" | cksum | awk '{print $1}') % 256 ))
  fi
  # b1=salt, b2/b3=VMID (fold high bits so large IDs still differ).
  printf 'BC:24:11:%02X:%02X:%02X' \
    "$salt" \
    $(( ((id >> 8) ^ (id >> 16)) & 255 )) \
    $(( id & 255 ))
}
NET0_MAC="$(mac_for_vmid "${VMID}")"
NET0_SPEC="virtio=${NET0_MAC},bridge=${BRIDGE}"

# Normalize / infer guest arch.
normalize_arch() {
  local a
  a="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  case "$a" in
    amd64|x86_64|x64) echo amd64 ;;
    arm64|aarch64) echo arm64 ;;
    *) echo "" ;;
  esac
}
if [[ -z "$ARCH" ]]; then
  base="$(basename "$DISK")"
  if [[ "$base" == *arm64* || "$base" == *aarch64* ]]; then
    ARCH=arm64
  elif [[ "$base" == *amd64* || "$base" == *x86_64* ]]; then
    ARCH=amd64
  else
    ARCH=amd64
  fi
fi
ARCH="$(normalize_arch "$ARCH")"
[[ -n "$ARCH" ]] || {
  echo "unsupported --arch (use amd64|arm64)" >&2
  exit 1
}
export PERTISK_ARCH="$ARCH"

# QEMU machine / Proxmox arch field.
if [[ "$ARCH" == "arm64" ]]; then
  PVE_ARCH="aarch64"
  PVE_MACHINE="virt"
else
  PVE_ARCH="x86_64"
  PVE_MACHINE="q35"
fi

: "${PROXMOX_URL:?set PROXMOX_URL}"
: "${PROXMOX_TOKEN_ID:?set PROXMOX_TOKEN_ID (user@realm!tokenid)}"
: "${PROXMOX_TOKEN_SECRET:?set PROXMOX_TOKEN_SECRET}"
: "${NODE:?set PROXMOX_NODE or --node}"

command -v curl >/dev/null || {
  echo "curl required" >&2
  exit 1
}
command -v jq >/dev/null || {
  echo "jq required" >&2
  exit 1
}

CURL=(curl -sS)
[[ "${PROXMOX_INSECURE:-0}" == "1" ]] && CURL+=(-k)
AUTH="Authorization: PVEAPIToken=${PROXMOX_TOKEN_ID}=${PROXMOX_TOKEN_SECRET}"
BASE="${PROXMOX_URL%/}/api2/json"

api_get() {
  "${CURL[@]}" -H "${AUTH}" "${BASE}$1"
}

# PVE host arch (kernel). API tokens cannot set a *non-default* guest arch.
PVE_HOST_ARCH=amd64
PVE_HOST_MACH="$(api_get "/nodes/${NODE}/status" 2>/dev/null | jq -r '.data["current-kernel"].machine // empty' 2>/dev/null || true)"
case "${PVE_HOST_MACH}" in
  aarch64|arm64) PVE_HOST_ARCH=arm64 ;;
esac
pve_arm64_is_native() {
  [[ "$ARCH" == "arm64" && "$PVE_HOST_ARCH" == "arm64" ]]
}

# arm64 guest CPU: native aarch64 PVE → host (KVM); amd64 PVE → max (TCG).
# cortex-a53/a57 often fail kvm_arch_init_vcpu on newer hosts (e.g. Cortex-A720).
resolve_arm64_cpu() {
  if [[ -n "${PROXMOX_CPU:-}" ]]; then
    echo "${PROXMOX_CPU}"
    return
  fi
  if [[ "${PVE_HOST_ARCH}" == "arm64" ]]; then
    echo host
  else
    echo max
  fi
}
ARM64_CPU=""
if [[ "$ARCH" == "arm64" ]]; then
  ARM64_CPU="$(resolve_arm64_cpu)"
  echo "==> arm64 guest cpu=${ARM64_CPU} (PVE host=${PVE_HOST_ARCH}; override with PROXMOX_CPU=)"
fi

api_post_form() {
  local path="$1"
  shift
  "${CURL[@]}" -X POST -H "${AUTH}" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    "${BASE}${path}" "$@"
}

api_put_form() {
  local path="$1"
  shift
  "${CURL[@]}" -X PUT -H "${AUTH}" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    "${BASE}${path}" "$@"
}

# PUT /config often returns {"data":null} on success; treat errors/message as failure.
# Note: jq `(.message|type=="string" and length>0)` can mis-handle null on some jq
# builds; keep the message check explicit.
api_response_ok() {
  local body="$1"
  echo "${body}" | jq -e '
    has("data")
    and (has("errors") | not)
    and (
      (.message | type) != "string"
      or (.message | length) == 0
    )
  ' >/dev/null 2>&1
}

vm_has_scsi0() {
  local conf i
  for i in 1 2 3 4 5 6; do
    conf="$(api_get "/nodes/${NODE}/qemu/${VMID}/config" 2>/dev/null || echo '{}')"
    if echo "${conf}" | jq -e '.data.scsi0 != null and (.data.scsi0|type=="string") and (.data.scsi0|length>0)' >/dev/null 2>&1; then
      return 0
    fi
    # API can lag right after qm importdisk / config PUT.
    if [[ -n "${PROXMOX_SSH:-}" ]]; then
      if ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 "${PROXMOX_SSH}" \
        "qm config ${VMID} | grep -q '^scsi0:'" >/dev/null 2>&1; then
        return 0
      fi
    fi
    sleep 1
  done
  return 1
}

# Upload/import return UPIDs; wait until stopped + exitstatus OK.
wait_task() {
  local upid="$1"
  local label="${2:-task}"
  local enc status exitst i
  [[ -n "${upid}" && "${upid}" == UPID:* ]] || return 0
  enc="$(printf '%s' "${upid}" | jq -sRr @uri)"
  echo "==> waiting for ${label}: ${upid}"
  for i in $(seq 1 600); do
    status="$(api_get "/nodes/${NODE}/tasks/${enc}/status" 2>/dev/null || echo '{}')"
    if echo "${status}" | jq -e '.data.status == "stopped"' >/dev/null 2>&1; then
      exitst="$(echo "${status}" | jq -r '.data.exitstatus // empty')"
      if [[ "${exitst}" == "OK" ]]; then
        echo "==> ${label} OK"
        return 0
      fi
      echo "${label} failed: ${exitst}" >&2
      echo "${status}" | jq . >&2 || true
      return 1
    fi
    sleep 1
  done
  echo "${label} timed out after 600s" >&2
  return 1
}

vm_stop() {
  local cur
  cur="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
  if echo "${cur}" | jq -e '.data.status == "running"' >/dev/null 2>&1; then
    echo "==> stopping VM ${VMID}"
    api_post_form "/nodes/${NODE}/qemu/${VMID}/status/stop" >/dev/null 2>&1 || true
    local i
    for i in $(seq 1 60); do
      cur="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
      echo "${cur}" | jq -e '.data.status == "stopped"' >/dev/null 2>&1 && return 0
      sleep 1
    done
    echo "WARNING: VM ${VMID} still not stopped" >&2
  fi
}

# Remove scsi0 so a fresh import-from can replace the guest disk.
detach_scsi0() {
  if ! vm_has_scsi0; then
    return 0
  fi
  echo "==> detaching scsi0 for re-import"
  vm_stop
  local att
  att="$(api_put_form "/nodes/${NODE}/qemu/${VMID}/config" --data-urlencode "delete=scsi0")"
  if ! api_response_ok "${att}"; then
    echo "detach scsi0 failed: ${att}" >&2
    exit 1
  fi
}

echo "==> Proxmox ${PROXMOX_URL} node=${NODE} storage=${STORAGE} vmid=${VMID} arch=${ARCH} (${PVE_ARCH}/${PVE_MACHINE})"

# Tokens cannot set a *non-default* arch. Native aarch64 PVE already defaults to
# aarch64, so API create (omit arch=) works like amd64-on-amd64. Cross-arch
# (x86 PVE + aarch64 guest) still needs a template clone or root SSH.
ARM64_TEMPLATE="${PROXMOX_ARM64_TEMPLATE:-}"

ssh_ok() {
  [[ -n "${PROXMOX_SSH:-}" ]] || return 1
  ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 \
    "${PROXMOX_SSH}" true 2>/dev/null
}

discover_arm64_template() {
  local data vmid arch name
  data="$(api_get "/nodes/${NODE}/qemu" 2>/dev/null || echo '{}')"
  while IFS=$'\t' read -r vmid name; do
    [[ -z "${vmid}" || "${vmid}" == "null" ]] && continue
    case "$(printf '%s' "${name}" | tr '[:upper:]' '[:lower:]')" in
      *pertisk-arm64-template*)
        echo "${vmid}"
        return 0
        ;;
    esac
  done < <(echo "${data}" | jq -r '.data[]? | [(.vmid|tostring), (.name // "")] | @tsv' 2>/dev/null)
  while read -r vmid; do
    [[ -z "${vmid}" || "${vmid}" == "null" ]] && continue
    arch="$(api_get "/nodes/${NODE}/qemu/${vmid}/config" 2>/dev/null | jq -r '.data.arch // empty')"
    if [[ "${arch}" == "aarch64" || "${arch}" == "arm64" ]]; then
      echo "${vmid}"
      return 0
    fi
  done < <(echo "${data}" | jq -r '.data[]? | select((.template // 0) == 1) | .vmid' 2>/dev/null)
  if echo "${data}" | jq -e '.data[]? | select(.vmid == 8900)' >/dev/null 2>&1; then
    arch="$(api_get "/nodes/${NODE}/qemu/8900/config" 2>/dev/null | jq -r '.data.arch // empty')"
    if [[ "${arch}" == "aarch64" || "${arch}" == "arm64" ]]; then
      echo 8900
      return 0
    fi
  fi
  return 1
}

# Multi-PVE: keys often exist on only one host. Don't scp/qm-resize then die.
if [[ -n "${PROXMOX_SSH:-}" ]] && ! ssh_ok; then
  echo "==> SSH ${PROXMOX_SSH} not usable (no key auth) — Proxmox API for this provider"
  unset PROXMOX_SSH || true
  if [[ -z "${PROXMOX_UPLOAD_STORAGE:-}" ]]; then
    export PROXMOX_UPLOAD_STORAGE=local
  fi
fi

if [[ "$ARCH" == "arm64" && -z "${ARM64_TEMPLATE}" ]] && ! pve_arm64_is_native; then
  if found="$(discover_arm64_template)"; then
    ARM64_TEMPLATE="${found}"
    echo "==> auto PROXMOX_ARM64_TEMPLATE=${ARM64_TEMPLATE} (aarch64 template on ${NODE})"
  fi
fi

require_arm64_create_path() {
  if pve_arm64_is_native; then
    echo "==> arm64 on native aarch64 PVE — API create (default arch; no SSH/template)"
    return 0
  fi
  if [[ -n "$ARM64_TEMPLATE" ]]; then
    echo "==> arm64 via API clone of template VMID=${ARM64_TEMPLATE} (no SSH)"
    return 0
  fi
  if ssh_ok; then
    echo "==> arm64 via qm create over ${PROXMOX_SSH}"
    return 0
  fi
  cat >&2 <<'EOF'
ERROR: arm64 guests on an amd64 Proxmox host cannot be created with API tokens alone.
Proxmox: only root@pam (not tokens) may set a non-default arch=aarch64.

Native aarch64 PVE does not need this — default VM arch is already aarch64.

On x86 PVE pick one:

  A) No SSH (recommended once set up) — create a template on the PVE console, then:
       # on Proxmox (root shell / Host Client):
       ./scripts/proxmox-ensure-arm64-template.sh   # or paste the qm commands from that script
       # on mgmt /etc/pertisk-mgmt/pertisk-mgmt.env:
       PROXMOX_ARM64_TEMPLATE=8900
       PROXMOX_NO_SSH=1
       sudo systemctl restart pertisk-mgmt

  B) SSH for each create — install pertisk-mgmt pubkey on PVE root, then:
       PROXMOX_SSH=root@<pve-ip>
       PROXMOX_NO_SSH=0
EOF
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    echo >&2
    echo "PROXMOX_SSH=${PROXMOX_SSH} is set but key auth failed (BatchMode)." >&2
  fi
  exit 1
}

clone_from_arm64_template() {
  local tmpl="$1"
  echo "==> cloning template ${tmpl} → VMID=${VMID} name=${NAME} (inherits arch=aarch64)"
  local resp upid
  resp="$(api_post_form "/nodes/${NODE}/qemu/${tmpl}/clone" \
    --data-urlencode "newid=${VMID}" \
    --data-urlencode "name=${NAME}" \
    --data-urlencode "full=1" \
    --data-urlencode "storage=${STORAGE}")"
  upid="$(echo "${resp}" | jq -r '.data // empty')"
  if [[ "${upid}" == UPID:* ]]; then
    wait_task "${upid}" "clone-arm64-template" || exit 1
  elif ! echo "${resp}" | jq -e '.data != null' >/dev/null 2>&1; then
    echo "clone failed: ${resp}" >&2
    echo "hint: create template first (scripts/proxmox-ensure-arm64-template.sh) or set PROXMOX_SSH" >&2
    exit 1
  fi
  # Apply sizing + pin MAC to this VMID (do not keep template MAC).
  # Strip any template scsi0 so we import the cloud image fresh.
  api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
    --data-urlencode "memory=${MEMORY}" \
    --data-urlencode "cores=${CORES}" \
    --data-urlencode "cpu=${ARM64_CPU:-host}" \
    --data-urlencode "machine=virt" \
    --data-urlencode "bios=ovmf" \
    --data-urlencode "net0=${NET0_SPEC}" >/dev/null 2>&1 || true
  echo "    net0=${NET0_SPEC} (MAC pinned from VMID)"
  if vm_has_scsi0; then
    echo "==> removing template scsi0 before cloud-image import"
    detach_scsi0
  fi
}

if [[ "$ARCH" == "arm64" ]]; then
  require_arm64_create_path
  # Template / native-API paths must not fall through to scp/qm when SSH is dead.
  if { [[ -n "$ARM64_TEMPLATE" ]] || pve_arm64_is_native; } && ! ssh_ok; then
    unset PROXMOX_SSH || true
  fi
fi

# --- Create VM skeleton (UEFI: OVMF on amd64, AAVMF on arm64) ---
EXISTS="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
if echo "${EXISTS}" | jq -e '.data != null' >/dev/null 2>&1; then
  echo "==> VM ${VMID} already exists — updating memory=${MEMORY} cores=${CORES} net0=${NET0_SPEC}"
  api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
    --data-urlencode "memory=${MEMORY}" \
    --data-urlencode "cores=${CORES}" \
    --data-urlencode "net0=${NET0_SPEC}" >/dev/null 2>&1 || true
  if [[ "$ARCH" == "arm64" && -z "$ARM64_TEMPLATE" ]] && ! pve_arm64_is_native && ssh_ok; then
    echo "==> ensure arch=aarch64 machine=virt via ${PROXMOX_SSH}"
    ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" \
      "qm set ${VMID} --arch aarch64 --machine virt --bios ovmf" || {
      echo "ERROR: qm set --arch aarch64 failed on ${PROXMOX_SSH}" >&2
      exit 1
    }
  elif [[ "$ARCH" == "arm64" ]]; then
    api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
      --data-urlencode "machine=virt" \
      --data-urlencode "bios=ovmf" >/dev/null 2>&1 || true
  fi
else
  if [[ "$ARCH" == "arm64" && -n "$ARM64_TEMPLATE" ]] && ! pve_arm64_is_native; then
    clone_from_arm64_template "$ARM64_TEMPLATE"
  elif [[ "$ARCH" == "arm64" ]] && ! pve_arm64_is_native; then
    # Cross-arch: qm as root so arch=aarch64 is set atomically (API tokens cannot).
    echo "==> creating VM ${VMID} (${NAME}) via ${PROXMOX_SSH}: arch=aarch64 bios=ovmf machine=virt agent=1"
    EFI_STORAGE="${PROXMOX_EFI_STORAGE:-${STORAGE}}"
    # cpu=host on native aarch64 PVE; cpu=max when emulating aarch64 on amd64 (TCG).
    # cortex-a* models often fail kvm_arch_init_vcpu on Cortex-A720 hosts.
    ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" \
      "qm create ${VMID} --name $(printf '%q' "$NAME") --memory ${MEMORY} --cores ${CORES} \
        --cpu ${ARM64_CPU:-host} --arch aarch64 --machine virt --bios ovmf \
        --scsihw virtio-scsi-single --net0 ${NET0_SPEC} \
        --ostype l26 --agent enabled=1 --onboot 1 \
        --efidisk0 ${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0" || {
      echo "ERROR: qm create failed on ${PROXMOX_SSH}" >&2
      echo "hint: on amd64 PVE hosts install pve-edk2-firmware-aarch64" >&2
      exit 1
    }
  else
    echo "==> creating VM ${VMID} (${NAME}) arch=${PVE_ARCH} bios=ovmf machine=${PVE_MACHINE} agent=1"
    # Fingerprint: fixed create path treats {"data":"UPID:…"} as success.
    # If logs still show: create with efidisk0 failed ({"data":"UPID:…"})
    # then /usr/share/pertisk-mgmt/scripts/proxmox-upload-vm.sh is stale — redeploy scripts.
    EFI_STORAGE="${PROXMOX_EFI_STORAGE:-${STORAGE}}"
    CREATE_ARGS=(
      --data-urlencode "vmid=${VMID}"
      --data-urlencode "name=${NAME}"
      --data-urlencode "memory=${MEMORY}"
      --data-urlencode "cores=${CORES}"
      --data-urlencode "cpu=${ARM64_CPU:-host}"
      --data-urlencode "machine=${PVE_MACHINE}"
      --data-urlencode "bios=ovmf"
      --data-urlencode "scsihw=virtio-scsi-single"
      --data-urlencode "net0=${NET0_SPEC}"
      --data-urlencode "ostype=l26"
      --data-urlencode "agent=enabled=1"
      --data-urlencode "onboot=1"
    )
    # Prefer allocating EFI vars at create time (avoids locked-VM race on a follow-up PUT).
    CREATE_WITH_EFI=(
      "${CREATE_ARGS[@]}"
      --data-urlencode "efidisk0=${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0"
    )
    # Do NOT send arch= via API: Proxmox returns "only root can set 'arch' config"
    # for API tokens. Default matches the PVE host (x86_64 on amd64, aarch64 on
    # aarch64). Native arm64 guests therefore omit arch= just like amd64.
    #
    # Create returns {"data":"UPID:…"} on success (async). Never treat that as
    # failure or retry — a second create hits "VM already exists".
    create_qemu_api() {
      local resp upid msg
      resp="$(api_post_form "/nodes/${NODE}/qemu" "$@")"
      upid="$(echo "${resp}" | jq -r 'if (.data|type)=="string" then .data else empty end' 2>/dev/null || true)"
      if [[ "${upid}" == UPID:* ]]; then
        wait_task "${upid}" "vm-create" || return 1
        return 0
      fi
      if api_response_ok "${resp:-{}}"; then
        return 0
      fi
      msg="$(echo "${resp}" | jq -r '.message // empty' 2>/dev/null || true)"
      echo "${resp}" >&2
      # Caller may inspect msg via CREATE_LAST_MSG.
      CREATE_LAST_MSG="${msg}"
      return 1
    }
    qemu_vm_exists() {
      local st
      st="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
      echo "${st}" | jq -e '.data != null and .data.status != null' >/dev/null 2>&1
    }

    CREATE_LAST_MSG=""
    if create_qemu_api "${CREATE_WITH_EFI[@]}"; then
      :
    elif qemu_vm_exists; then
      # Race: create-with-EFI actually started (UPID) but response parsing failed,
      # or a prior partial run left the VM.
      echo "    VM ${VMID} already exists after create-with-EFI attempt — continuing" >&2
    else
      echo "    create with efidisk0 failed; retrying without EFI (ensure_efidisk will attach)" >&2
      [[ -n "${CREATE_LAST_MSG}" ]] && echo "    reason: ${CREATE_LAST_MSG}" >&2
      if ! create_qemu_api "${CREATE_ARGS[@]}"; then
        if qemu_vm_exists; then
          echo "    VM ${VMID} already exists — continuing" >&2
        else
          echo "create failed" >&2
          exit 1
        fi
      fi
    fi
  fi
  if [[ "${DUAL_STACK:-${PERTISK_DUAL_STACK:-0}}" == "1" ]]; then
    echo "    net0=${NET0_SPEC} (MAC pinned from VMID; dual-stack: IPv6 after machine config apply)"
  else
    echo "    net0=${NET0_SPEC} (MAC pinned from VMID; IPv4-only by default)"
  fi
fi

# EFI vars disk — required for OVMF/AAVMF (avoids "temporary efivars disk" WARN).
# Secure Boot keys OFF (unsigned systemd-boot).
EFI_STORAGE="${PROXMOX_EFI_STORAGE:-${STORAGE}}"

vm_has_efidisk() {
  local conf val
  conf="$(api_get "/nodes/${NODE}/qemu/${VMID}/config" 2>/dev/null || echo '{}')"
  # PVE returns e.g. "local-zfs:vm-210-disk-4,efitype=4m,size=1M"
  val="$(echo "${conf}" | jq -r '.data.efidisk0 // empty' 2>/dev/null || true)"
  if [[ -n "${val}" && "${val}" != "null" ]]; then
    return 0
  fi
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${PROXMOX_SSH}" \
      "qm config ${VMID} | grep -q '^efidisk0:'" >/dev/null 2>&1 && return 0
  fi
  return 1
}

# Apply efidisk0 via API; wait if Proxmox returns an allocation UPID.
# {"data":null} is success (config applied / already present).
api_set_efidisk() {
  local spec="$1"
  local body upid
  body="$(api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
    --data-urlencode "efidisk0=${spec}" 2>/dev/null || echo '{}')"
  if ! api_response_ok "${body:-{}}"; then
    echo "    API efidisk0=${spec} rejected: ${body}" >&2
    return 1
  fi
  upid="$(echo "${body}" | jq -r 'if (.data|type)=="string" then .data else empty end')"
  if [[ "${upid}" == UPID:* ]]; then
    wait_task "${upid}" "efidisk0" || return 1
  fi
  return 0
}

ensure_efidisk() {
  # Create-with-efidisk may need a beat before config GET reflects the volume.
  local i
  for i in 1 2 3 4 5 6; do
    if vm_has_efidisk; then
      echo "==> efidisk0 already present"
      return 0
    fi
    sleep 1
  done
  echo "==> ensuring EFI disk on ${EFI_STORAGE} (efitype=4m, pre-enrolled-keys=0)"
  local ok=0
  if [[ -n "${PROXMOX_SSH:-}" ]]; then
    if ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" \
      "qm set ${VMID} --efidisk0 ${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0"; then
      ok=1
    else
      echo "    qm set efidisk0 failed; trying API" >&2
    fi
  fi
  if [[ "$ok" != "1" ]]; then
    local spec
    for spec in \
      "${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0" \
      "${EFI_STORAGE}:0,efitype=4m,pre-enrolled-keys=0" \
      "${EFI_STORAGE}:1,efitype=4m" \
      "${EFI_STORAGE}:0,efitype=4m"
    do
      if api_set_efidisk "${spec}"; then
        ok=1
        # Re-check soon — already-present disks often return {"data":null}.
        vm_has_efidisk && return 0
        break
      fi
    done
  fi
  for i in 1 2 3 4 5 6 7 8 9 10; do
    vm_has_efidisk && return 0
    sleep 1
  done
  # Last-resort: print config; if efidisk0 is actually there, continue (avoid false fail).
  local conf snap
  conf="$(api_get "/nodes/${NODE}/qemu/${VMID}/config" 2>/dev/null || echo '{}')"
  snap="$(echo "${conf}" | jq -c '{efidisk0:.data.efidisk0,bios:.data.bios,scsi0:.data.scsi0}' 2>/dev/null || true)"
  if echo "${conf}" | jq -e '(.data.efidisk0|type)=="string" and (.data.efidisk0|length)>0' >/dev/null 2>&1; then
    echo "==> efidisk0 present after settle (${snap})"
    return 0
  fi
  echo "ERROR: efidisk0 missing after create — UEFI will use a temporary efivars disk" >&2
  echo "  current config: ${snap}" >&2
  echo "  try: ssh ${PROXMOX_SSH:-root@pve} qm set ${VMID} --efidisk0 ${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0" >&2
  echo "  or set PROXMOX_SSH=root@${NODE} / PROXMOX_EFI_STORAGE=local and re-run" >&2
  exit 1
}

ensure_efidisk

# Serial as primary console: Proxmox Console opens xterm.js on serial0.
# Pertisk cmdline uses console=ttyS0; vga=serial0 makes UI default to Serial.
echo "==> serial0=socket + vga=serial0 + qemu-guest-agent"
if [[ -n "${PROXMOX_SSH:-}" ]]; then
  ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" \
    "qm set ${VMID} --serial0 socket --vga serial0 --agent enabled=1" >/dev/null 2>&1 || true
fi
api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
  --data-urlencode "serial0=socket" \
  --data-urlencode "vga=serial0" \
  --data-urlencode "agent=enabled=1" >/dev/null 2>&1 || true

# --- Disk import ---
# Prefer explicit PROXMOX_SSH (scp+qm). Otherwise Omni-style Proxmox API upload
# (provider token only — no SSH to the node). On SSH failure, fall back to API.
DISK_ATTACHED=0
if [[ -n "${PROXMOX_SSH:-}" ]]; then
  if [[ "${PROXMOX_KEEP_DISK:-0}" == "1" ]] && vm_has_scsi0; then
    echo "==> scsi0 already present — keep disk (unset PROXMOX_KEEP_DISK to re-import)"
    DISK_ATTACHED=1
  else
    detach_scsi0
    echo "==> SCP + qm importdisk via ${PROXMOX_SSH}"
    REMOTE="/var/tmp/pertisk-${VMID}.qcow2"
    if scp -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 \
         "${DISK}" "${PROXMOX_SSH}:${REMOTE}" \
      && ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" bash -s <<EOF
set -euo pipefail
VMID=${VMID}
STORAGE=${STORAGE}
REMOTE=${REMOTE}
qm importdisk "\${VMID}" "\${REMOTE}" "\${STORAGE}" --format qcow2
rm -f "\${REMOTE}"
CONF=\$(qm config "\${VMID}")
BEST_KEY=""; BEST_VOL=""; BEST_SIZE=0; BEST_N=-1
while IFS= read -r line; do
  key=\$(echo "\$line" | sed -n 's/^\\(unused[0-9]*\\):.*/\\1/p')
  vol=\$(echo "\$line" | sed -n 's/^unused[0-9]*: //p')
  [[ -n "\$vol" ]] || continue
  n=\$(echo "\$vol" | sed -n 's/.*-disk-\\([0-9]*\\)\$/\\1/p')
  n=\${n:--1}
  size=\$(pvesm list "\${STORAGE}" 2>/dev/null | awk -v v="\$vol" '\$1==v {print \$4; exit}')
  size=\${size:-0}
  echo "unused candidate: \$key \$vol size=\$size"
  if [[ "\$size" -gt "\$BEST_SIZE" || ( "\$size" -eq "\$BEST_SIZE" && "\$n" -gt "\$BEST_N" ) ]]; then
    BEST_SIZE=\$size; BEST_KEY=\$key; BEST_VOL=\$vol; BEST_N=\$n
  fi
done < <(echo "\$CONF" | grep '^unused' || true)
[[ -n "\$BEST_VOL" ]] || { echo "no unused disk after importdisk" >&2; qm config "\${VMID}"; exit 1; }
if [[ "\$BEST_SIZE" -gt 0 && "\$BEST_SIZE" -lt 1073741824 ]]; then
  echo "ERROR: best unused disk is only \${BEST_SIZE} bytes (need >=1GiB OS image)" >&2
  qm config "\${VMID}"; exit 1
fi
echo "==> attaching \$BEST_KEY -> scsi0 (\$BEST_VOL, \${BEST_SIZE} bytes)"
qm set "\${VMID}" --scsihw virtio-scsi-single
qm set "\${VMID}" --scsi0 "\${BEST_VOL}"
qm set "\${VMID}" --delete "\${BEST_KEY}" || true
qm set "\${VMID}" --boot order=scsi0
while IFS= read -r line; do
  key=\$(echo "\$line" | sed -n 's/^\\(unused[0-9]*\\):.*/\\1/p')
  vol=\$(echo "\$line" | sed -n 's/^unused[0-9]*: //p')
  size=\$(pvesm list "\${STORAGE}" 2>/dev/null | awk -v v="\$vol" '\$1==v {print \$4; exit}')
  size=\${size:-0}
  if [[ "\$size" -gt 0 && "\$size" -lt 10485760 ]]; then
    echo "==> deleting tiny unused \$key \$vol (\$size)"
    qm set "\${VMID}" --delete "\$key" || true
    pvesm free "\$vol" 2>/dev/null || true
  fi
done < <(qm config "\${VMID}" | grep '^unused' || true)
qm config "\${VMID}" | grep -E '^(scsi0|boot|efidisk0|unused):' || true
EOF
    then
      if vm_has_scsi0; then
        echo "==> scsi0 attached via ${PROXMOX_SSH}"
        DISK_ATTACHED=1
      else
        echo "WARNING: scsi0 missing after SSH import — falling back to API" >&2
      fi
    else
      echo "WARNING: SSH import failed (scp/ssh) — falling back to Proxmox API upload" >&2
      echo "         (set PROXMOX_NO_SSH=1 to skip SSH; or fix keys for ${PROXMOX_SSH})" >&2
    fi
  fi
fi

if [[ "$DISK_ATTACHED" != "1" ]]; then
  # PVE 8+: upload content=import, then scsi0 import-from= (no SSH).
  # ZFS/LVM cannot store import files — use directory storage (usually local).
  UPLOAD_STORAGE="${PROXMOX_UPLOAD_STORAGE:-}"
  if [[ -z "$UPLOAD_STORAGE" ]]; then
    case "${STORAGE}" in
      *zfs*|*lvm*|local-lvm) UPLOAD_STORAGE=local ;;
      *) UPLOAD_STORAGE="${STORAGE}" ;;
    esac
  fi
  VOL="pertisk-${VMID}.qcow2"
  IMPORT_REF="${UPLOAD_STORAGE}:import/${VOL}"

  if [[ "${PROXMOX_KEEP_DISK:-0}" == "1" ]] && vm_has_scsi0; then
    echo "==> scsi0 already present — keep disk (unset PROXMOX_KEEP_DISK to re-import)"
  else
    detach_scsi0

    echo "==> API upload content=import → storage=${UPLOAD_STORAGE} as ${VOL}"
    # Explicit octet-stream: some PVE versions require a Content-Type on the file part.
    UP="$("${CURL[@]}" -w "\n%{http_code}" -X POST -H "${AUTH}" \
      -F "content=import" \
      -F "filename=@${DISK};type=application/octet-stream;filename=${VOL}" \
      "${BASE}/nodes/${NODE}/storage/${UPLOAD_STORAGE}/upload" || true)"
    UP_BODY="$(echo "${UP}" | sed '$d')"
    UP_CODE="$(echo "${UP}" | tail -n1)"

    if [[ "${UP_CODE}" != "200" ]] || ! echo "${UP_BODY}" | jq -e '.data != null' >/dev/null 2>&1; then
      echo "upload failed (HTTP ${UP_CODE}): ${UP_BODY}" >&2
      echo >&2
      echo "API import needs directory storage with content=import (usually 'local')." >&2
      echo "  PROXMOX_UPLOAD_STORAGE=local PROXMOX_STORAGE=${STORAGE}" >&2
      echo "  Or set PROXMOX_SSH=root@host for scp + qm importdisk" >&2
      exit 1
    fi
    UPID="$(echo "${UP_BODY}" | jq -r '.data')"
    echo "==> upload accepted: ${UPID}"
    # Critical: import-from before the upload worker finishes yields a truncated
    # non-qcow2 file ("Image is not in qcow2 format").
    wait_task "${UPID}" "upload" || exit 1

    # Import into VM disk on target STORAGE (may differ from upload storage).
    # PUT /config returns {"data":null} on success — do not require a truthy .data.
    echo "==> attaching scsi0 via import-from=${IMPORT_REF} → ${STORAGE}"
    ATT="$(
      api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
        --data-urlencode "scsi0=${STORAGE}:0,import-from=${IMPORT_REF}"
    )"
    if ! api_response_ok "${ATT}"; then
      echo "import-from failed: ${ATT}" >&2
      echo "Disk is on ${IMPORT_REF}. In UI: VM ${VMID} → Hardware → Add → Import," >&2
      echo "or: PROXMOX_SSH=root@host ./scripts/proxmox-upload-vm.sh ..." >&2
      exit 1
    fi
    # Some PVE versions return a UPID for import-from as well.
    IMP_UPID="$(echo "${ATT}" | jq -r '.data // empty')"
    if [[ "${IMP_UPID}" == UPID:* ]]; then
      wait_task "${IMP_UPID}" "import-from" || exit 1
    fi
    if ! vm_has_scsi0; then
      echo "import-from reported OK but scsi0 missing" >&2
      exit 1
    fi
    echo "==> scsi0 import-from ok"
  fi
fi

api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
  --data-urlencode "boot=order=scsi0" \
  --data-urlencode "onboot=1" \
  --data-urlencode "serial0=socket" \
  --data-urlencode "vga=serial0" \
  --data-urlencode "agent=enabled=1" >/dev/null 2>&1 || true

# Never start a diskless VM — OVMF falls through to "PXE over IPv6" and looks
# like a network-boot failure when the real problem is missing scsi0.
if ! vm_has_scsi0; then
  echo "ERROR: scsi0 missing after upload — VM would only PXE-boot." >&2
  echo "  Current config snippet:" >&2
  api_get "/nodes/${NODE}/qemu/${VMID}/config" \
    | jq '{scsi0:.data.scsi0, unused0:.data.unused0, unused1:.data.unused1, boot:.data.boot, efidisk0:.data.efidisk0}' >&2 || true
  echo "  Fix:" >&2
  echo "  PROXMOX_SSH=root@host ./scripts/proxmox-fix-boot.sh ${VMID}" >&2
  echo "  PROXMOX_SSH=root@host ./scripts/proxmox-reattach-disk.sh ${VMID} ${DISK}" >&2
  exit 1
fi

# Grow guest disk beyond the imported qcow2 size (grow-only).
# Note: GPT/EPHEMERAL only match if the qcow2 was built with PERTISK_DISK_GB>=N.
# Prefer importing a role-sized qcow2 (lab-up writes out/*-Ng.qcow2) instead of shrinking.
if [[ -n "${DISK_GB}" ]]; then
  if ! [[ "${DISK_GB}" =~ ^[0-9]+$ ]] || [[ "${DISK_GB}" -lt 1 ]]; then
    echo "ERROR: --disk-gb must be a positive integer (GiB), got: ${DISK_GB}" >&2
    exit 1
  fi
  IMG_BYTES=0
  if command -v qemu-img >/dev/null 2>&1; then
    IMG_BYTES="$(qemu-img info --output=json "${DISK}" 2>/dev/null | jq -r '.["virtual-size"] // 0' || echo 0)"
  elif command -v docker >/dev/null 2>&1; then
    IMG_BYTES="$(
      docker run --rm -v "$(cd "$(dirname "${DISK}")" && pwd):/d:ro" alpine:3.22 \
        sh -c "apk add --no-cache qemu-img >/dev/null && qemu-img info --output=json /d/$(basename "${DISK}")" \
        2>/dev/null | jq -r '.["virtual-size"] // 0' || echo 0
    )"
  fi
  TARGET_BYTES=$((DISK_GB * 1024 * 1024 * 1024))
  if [[ "${IMG_BYTES}" =~ ^[0-9]+$ ]] && [[ "${IMG_BYTES}" -gt 0 ]]; then
    if [[ "${TARGET_BYTES}" -lt "${IMG_BYTES}" ]]; then
      echo "WARN: --disk-gb ${DISK_GB}G < imported image (~$((IMG_BYTES / 1024 / 1024 / 1024))G); qm resize cannot shrink — keeping image size" >&2
      echo "      Rebuild with lab-up (no --skip-build) so CP/worker get separate *-Ng.qcow2 images." >&2
      DISK_GB=""
    elif [[ "${TARGET_BYTES}" -eq "${IMG_BYTES}" ]] || [[ "${TARGET_BYTES}" -le $((IMG_BYTES + 64 * 1024 * 1024)) ]]; then
      echo "==> scsi0 already ~${DISK_GB}G (image virtual size) — skip resize"
      DISK_GB=""
    fi
  fi
fi
if [[ -n "${DISK_GB}" ]]; then
  echo "==> resizing scsi0 → ${DISK_GB}G"
  vm_stop 2>/dev/null || true
  resized=0
  if ssh_ok; then
    if ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 \
      "${PROXMOX_SSH}" \
      "qm resize ${VMID} scsi0 ${DISK_GB}G && qm config ${VMID} | grep '^scsi0:'"; then
      resized=1
    else
      echo "WARNING: qm resize via SSH failed — trying Proxmox API" >&2
    fi
  elif [[ -n "${PROXMOX_SSH:-}" ]]; then
    echo "WARNING: SSH ${PROXMOX_SSH} unreachable — trying Proxmox API resize" >&2
  fi
  if [[ "${resized}" != "1" ]]; then
    RESZ="$(
      api_put_form "/nodes/${NODE}/qemu/${VMID}/resize" \
        --data-urlencode "disk=scsi0" \
        --data-urlencode "size=${DISK_GB}G"
    )"
    if ! api_response_ok "${RESZ}"; then
      UPID="$(echo "${RESZ}" | jq -r '.data // empty')"
      if [[ "${UPID}" == UPID:* ]]; then
        wait_task "${UPID}" "resize" || exit 1
      else
        echo "resize failed: ${RESZ}" >&2
        echo "  Hint: set PROXMOX_SSH=root@<this-pve> for qm resize" >&2
        exit 1
      fi
    else
      echo "    API resize ok (verify Hardware → Hard Disk size in Proxmox UI)"
    fi
  fi
fi

# --- Static IP netcfg disk (Talos-style: guest never DHCPs, address is fixed
# in the machine config and survives reboot/shutdown; no external DHCP/router
# reservation needed). Same PERTISK-NET format the guest already reads on
# Nutanix AHV (crates/pertisk-net/src/provider_net.rs) — attach before first
# boot so pertiskd applies it before it would otherwise try DHCP.
attach_static_netcfg() {
  [[ -n "${STATIC_IP}" ]] || return 0
  local ns="${STATIC_NAMESERVER:-${STATIC_GATEWAY}}"
  echo "==> static IP ${STATIC_IP} gw=${STATIC_GATEWAY} (no DHCP) → netcfg disk"
  local raw upload_storage vol import_ref up up_body up_code upid att imp_upid
  raw="$(mktemp /tmp/pertisk-netcfg.XXXXXX.raw)"
  python3 - "${raw}" "${STATIC_IP}" "${STATIC_GATEWAY}" "${ns}" <<'PY'
import sys
path, cidr, gw, ns = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
blob = f"PERTISK-NET\nIPV4={cidr}\nGATEWAY={gw}\nNAMESERVER={ns}\nINTERFACE=eth0\n".encode()
size = 16 * 1024 * 1024
open(path, "wb").write(blob + b"\x00" * (size - len(blob)))
PY
  upload_storage="${PROXMOX_UPLOAD_STORAGE:-}"
  if [[ -z "${upload_storage}" ]]; then
    case "${STORAGE}" in
      *zfs*|*lvm*|local-lvm) upload_storage=local ;;
      *) upload_storage="${STORAGE}" ;;
    esac
  fi
  vol="pertisk-${VMID}-netcfg.raw"
  import_ref="${upload_storage}:import/${vol}"
  echo "    API upload content=import → storage=${upload_storage} as ${vol}"
  up="$("${CURL[@]}" -w "\n%{http_code}" -X POST -H "${AUTH}" \
    -F "content=import" \
    -F "filename=@${raw};type=application/octet-stream;filename=${vol}" \
    "${BASE}/nodes/${NODE}/storage/${upload_storage}/upload" || true)"
  rm -f "${raw}"
  up_body="$(echo "${up}" | sed '$d')"
  up_code="$(echo "${up}" | tail -n1)"
  if [[ "${up_code}" != "200" ]] || ! echo "${up_body}" | jq -e '.data != null' >/dev/null 2>&1; then
    echo "WARN: netcfg upload failed (HTTP ${up_code}); guest will fall back to DHCP: ${up_body}" >&2
    return 0
  fi
  upid="$(echo "${up_body}" | jq -r '.data')"
  wait_task "${upid}" "netcfg-upload" || {
    echo "WARN: netcfg upload task failed; guest will fall back to DHCP" >&2
    return 0
  }
  echo "    attaching virtio1 via import-from=${import_ref} → ${STORAGE}"
  att="$(api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
    --data-urlencode "virtio1=${STORAGE}:0,import-from=${import_ref}")"
  if ! api_response_ok "${att}"; then
    echo "WARN: netcfg disk attach failed; guest will fall back to DHCP: ${att}" >&2
    return 0
  fi
  imp_upid="$(echo "${att}" | jq -r '.data // empty')"
  if [[ "${imp_upid}" == UPID:* ]]; then
    wait_task "${imp_upid}" "netcfg-import" || true
  fi
  echo "    netcfg disk attached (virtio1, static IP ${STATIC_IP})"
}
attach_static_netcfg

if [[ "${START}" == "1" ]]; then
  # Re-check EFI before start (Proxmox warns and uses a temp efivars disk otherwise).
  ensure_efidisk
  echo "==> starting VM ${VMID}"
  started=0
  if ssh_ok; then
    if ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "${PROXMOX_SSH}" \
      "qm start ${VMID}"; then
      started=1
    else
      echo "WARNING: qm start via SSH failed — trying Proxmox API" >&2
    fi
  fi
  if [[ "$started" != "1" ]]; then
    START_RESP="$(api_post_form "/nodes/${NODE}/qemu/${VMID}/status/start" 2>/dev/null || true)"
    if echo "${START_RESP:-}" | jq -e '.data != null' >/dev/null 2>&1 \
      || api_response_ok "${START_RESP:-{}}"; then
      started=1
    else
      # Some PVE versions return null data on accepted start — confirm status.
      sleep 2
      st="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
      if echo "${st}" | jq -e '.data.status == "running"' >/dev/null 2>&1; then
        started=1
      fi
    fi
  fi
  if [[ "$started" != "1" ]]; then
    echo "ERROR: failed to start VM ${VMID}" >&2
    echo "  API: ${START_RESP:-}" >&2
    exit 1
  fi
fi

MAC="$(api_get "/nodes/${NODE}/qemu/${VMID}/config" | jq -r '.data.net0 // empty' | grep -oE '([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}' | head -1 || true)"
echo "==> done — open Console for ${NAME} (vmid ${VMID})"
[[ -n "${MAC}" ]] && echo "    MAC=${MAC}"
if [[ -n "${STATIC_IP}" ]]; then
  echo "    static IP=${STATIC_IP} gw=${STATIC_GATEWAY} (netcfg disk, no DHCP)"
fi
echo "    Console uses serial (vga=serial0 / xterm.js). Host: qm terminal ${VMID}"
echo "    QEMU guest agent enabled on the VM (Options → QEMU Guest Agent)."
echo "    Guest image runs qemu-ga (Summary IP + Shutdown). Rebuild cloud image if missing."
echo "    If PXE / UEFI shell: scsi0 missing or Secure Boot — run proxmox-fix-boot.sh"
echo "    join cluster: docs/PROXMOX.md"
