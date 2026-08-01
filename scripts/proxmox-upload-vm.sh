#!/usr/bin/env bash
# Upload a Pertisk cloud qcow2 to Proxmox and create a UEFI (OVMF) VM.
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
set -euo pipefail

VMID=""
NAME="pertisk-worker"
DISK=""
MEMORY=4096
CORES=2
BRIDGE="vmbr0"
START=1
STORAGE="${PROXMOX_STORAGE:-local}"
NODE="${PROXMOX_NODE:-}"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/proxmox-upload-vm.sh --vmid ID --disk PATH [options]

Options:
  --vmid N          VM id (required)
  --disk PATH       qcow2 path (required)
  --name NAME       VM name (default pertisk-worker)
  --memory MB       RAM (default 4096)
  --cores N         vCPUs (default 2)
  --bridge NAME     bridge (default vmbr0)
  --storage NAME    datastore (default $PROXMOX_STORAGE or local)
  --node NAME       node (default $PROXMOX_NODE)
  --no-start        do not start after create
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vmid) VMID="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --cores) CORES="$2"; shift 2 ;;
    --bridge) BRIDGE="$2"; shift 2 ;;
    --storage) STORAGE="$2"; shift 2 ;;
    --node) NODE="$2"; shift 2 ;;
    --no-start) START=0; shift ;;
    -h | --help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "${VMID}" && -n "${DISK}" ]] || usage
[[ -f "${DISK}" ]] || {
  echo "disk not found: ${DISK}" >&2
  exit 1
}

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

echo "==> Proxmox ${PROXMOX_URL} node=${NODE} storage=${STORAGE} vmid=${VMID}"

# --- Create VM skeleton (UEFI / q35) ---
EXISTS="$(api_get "/nodes/${NODE}/qemu/${VMID}/status/current" 2>/dev/null || echo '{}')"
if echo "${EXISTS}" | jq -e '.data' >/dev/null 2>&1; then
  echo "==> VM ${VMID} already exists"
else
  echo "==> creating VM ${VMID} (${NAME}) bios=ovmf machine=q35"
  RESP="$(
    api_post_form "/nodes/${NODE}/qemu" \
      --data-urlencode "vmid=${VMID}" \
      --data-urlencode "name=${NAME}" \
      --data-urlencode "memory=${MEMORY}" \
      --data-urlencode "cores=${CORES}" \
      --data-urlencode "cpu=host" \
      --data-urlencode "machine=q35" \
      --data-urlencode "bios=ovmf" \
      --data-urlencode "scsihw=virtio-scsi-single" \
      --data-urlencode "net0=virtio,bridge=${BRIDGE}" \
      --data-urlencode "ostype=l26"
  )"
  echo "${RESP}" | jq -e '.data' >/dev/null || {
    echo "create failed: ${RESP}" >&2
    exit 1
  }
fi

# EFI vars disk — must exist for OVMF; Secure Boot keys OFF (unsigned systemd-boot).
EFI_STORAGE="${PROXMOX_EFI_STORAGE:-${STORAGE}}"
echo "==> ensuring EFI disk on ${EFI_STORAGE} (pre-enrolled-keys=0)"
api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
  --data-urlencode "efidisk0=${EFI_STORAGE}:1,efitype=4m,pre-enrolled-keys=0" >/dev/null 2>&1 || \
  echo "    (efidisk0 may already exist — check Hardware)"

# Serial console: Pertisk cmdline uses console=ttyS0 (VGA may look blank).
echo "==> serial0=socket + boot order"
api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
  --data-urlencode "serial0=socket" \
  --data-urlencode "vga=std" \
  --data-urlencode "boot=order=scsi0;net0" >/dev/null 2>&1 || true

# --- Disk import ---
if [[ -n "${PROXMOX_SSH:-}" ]]; then
  echo "==> SCP + qm importdisk via ${PROXMOX_SSH}"
  REMOTE="/var/tmp/pertisk-${VMID}.qcow2"
  scp -o StrictHostKeyChecking=accept-new "${DISK}" "${PROXMOX_SSH}:${REMOTE}"
  ssh "${PROXMOX_SSH}" "qm importdisk ${VMID} ${REMOTE} ${STORAGE} --format qcow2 && rm -f ${REMOTE}"
  # Attach unused disk0 if present.
  CONF="$(api_get "/nodes/${NODE}/qemu/${VMID}/config")"
  UNUSED="$(echo "${CONF}" | jq -r '.data | to_entries[] | select(.key|startswith("unused")) | "\(.key)=\(.value)"' | head -1 || true)"
  if [[ -n "${UNUSED}" ]]; then
    KEY="${UNUSED%%=*}"
    VAL="${UNUSED#*=}"
    api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
      --data-urlencode "scsihw=virtio-scsi-single" \
      --data-urlencode "scsi0=${VAL}" \
      --data-urlencode "delete=${KEY}" \
      --data-urlencode "boot=order=scsi0" >/dev/null
    echo "==> attached ${VAL} as scsi0"
  else
    echo "==> WARNING: no unused disk found after importdisk" >&2
    echo "    Run: PROXMOX_SSH=${PROXMOX_SSH} ./scripts/proxmox-fix-boot.sh ${VMID}" >&2
  fi
else
  # PVE 8+: upload only accepts content ∈ {iso, vztmpl, import} on many backends.
  # ZFS/LVM thin: use content=import, then scsi0 import-from=.
  # Directory storage "local" can also use import.
  UPLOAD_STORAGE="${PROXMOX_UPLOAD_STORAGE:-${STORAGE}}"
  VOL="pertisk-${VMID}.qcow2"
  echo "==> API upload content=import → storage=${UPLOAD_STORAGE} as ${VOL}"

  UP="$("${CURL[@]}" -w "\n%{http_code}" -X POST -H "${AUTH}" \
    -F "content=import" \
    -F "filename=@${DISK};filename=${VOL}" \
    "${BASE}/nodes/${NODE}/storage/${UPLOAD_STORAGE}/upload" || true)"
  UP_BODY="$(echo "${UP}" | sed '$d')"
  UP_CODE="$(echo "${UP}" | tail -n1)"

  if [[ "${UP_CODE}" != "200" ]] || ! echo "${UP_BODY}" | jq -e '.data' >/dev/null 2>&1; then
    echo "upload failed (HTTP ${UP_CODE}): ${UP_BODY}" >&2
    echo >&2
    echo "local-zfs cannot use content=images. Fixes:" >&2
    echo "  1) Recommended: export PROXMOX_SSH=root@${NODE%%.*}  # or root@host" >&2
    echo "     and re-run (scp + qm importdisk → ${STORAGE})" >&2
    echo "  2) Or upload via directory storage first:" >&2
    echo "     PROXMOX_UPLOAD_STORAGE=local PROXMOX_STORAGE=${STORAGE} ./scripts/proxmox-upload-vm.sh ..." >&2
    echo "  3) Or UI: upload qcow2 → Hardware → Import disk" >&2
    exit 1
  fi
  echo "==> uploaded: $(echo "${UP_BODY}" | jq -r '.data')"

  # Import into VM disk on target STORAGE (may differ from upload storage).
  IMPORT_REF="${UPLOAD_STORAGE}:import/${VOL}"
  echo "==> attaching scsi0 via import-from=${IMPORT_REF} → ${STORAGE}"
  ATT="$(
    api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
      --data-urlencode "scsi0=${STORAGE}:0,import-from=${IMPORT_REF}" \
      --data-urlencode "boot=order=scsi0"
  )"
  if ! echo "${ATT}" | jq -e '.data' >/dev/null 2>&1; then
    echo "import-from failed: ${ATT}" >&2
    echo "Disk is on ${IMPORT_REF}. In UI: VM ${VMID} → Hardware → Add → Import," >&2
    echo "or: PROXMOX_SSH=root@host ./scripts/proxmox-upload-vm.sh ..." >&2
    exit 1
  fi
  echo "==> scsi0 import-from ok"
fi

api_put_form "/nodes/${NODE}/qemu/${VMID}/config" \
  --data-urlencode "boot=order=scsi0" \
  --data-urlencode "serial0=socket" \
  --data-urlencode "vga=std" >/dev/null 2>&1 || true

if [[ "${START}" == "1" ]]; then
  echo "==> starting VM ${VMID}"
  api_post_form "/nodes/${NODE}/qemu/${VMID}/status/start" >/dev/null 2>&1 || true
fi

echo "==> done — open console for ${NAME} (vmid ${VMID})"
echo "    Use Console → xterm.js, or Serial if VGA is blank (Pertisk logs on ttyS0)."
echo "    If stuck in UEFI: Options → disable Secure Boot; ensure efidisk pre-enrolled-keys=0."
echo "    join cluster: docs/PROXMOX.md"
