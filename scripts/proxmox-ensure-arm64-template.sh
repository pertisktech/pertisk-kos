#!/usr/bin/env bash
# One-time: create an empty aarch64 Proxmox template so Pertisk can clone it via
# API token (no SSH). Run on the Proxmox node as root, or with PROXMOX_SSH.
#
#   ./scripts/proxmox-ensure-arm64-template.sh
#   PROXMOX_ARM64_TEMPLATE=8900 PROXMOX_STORAGE=local-zfs ./scripts/proxmox-ensure-arm64-template.sh
#
# Then on mgmt (/etc/pertisk-mgmt/pertisk-mgmt.env):
#   PROXMOX_ARM64_TEMPLATE=8900
#   PROXMOX_NO_SSH=1
#   # comment out PROXMOX_SSH
#   sudo systemctl restart pertisk-mgmt
set -euo pipefail

VMID="${PROXMOX_ARM64_TEMPLATE:-8900}"
NAME="${PROXMOX_ARM64_TEMPLATE_NAME:-pertisk-arm64-template}"
STORAGE="${PROXMOX_EFI_STORAGE:-${PROXMOX_STORAGE:-local-zfs}}"
BRIDGE="${PROXMOX_BRIDGE:-vmbr0}"
SSH_HOST="${PROXMOX_SSH:-}"

# Stable MAC in Proxmox OUI space: BC:24:11 + 24-bit VMID (clones overwrite net0).
mac_for_vmid() {
  local id="$1"
  printf 'BC:24:11:%02X:%02X:%02X' $(( (id >> 16) & 255 )) $(( (id >> 8) & 255 )) $(( id & 255 ))
}
NET0_MAC="$(mac_for_vmid "${VMID}")"
NET0_SPEC="virtio=${NET0_MAC},bridge=${BRIDGE}"

run() {
  if [[ -n "$SSH_HOST" ]]; then
    ssh -o StrictHostKeyChecking=accept-new "$SSH_HOST" "$@"
  else
    bash -c "$*"
  fi
}

echo "==> ensure arm64 template VMID=${VMID} name=${NAME} storage=${STORAGE} net0=${NET0_SPEC}"

if run "qm status ${VMID} >/dev/null 2>&1"; then
  echo "==> VM ${VMID} already exists — converting to template if needed"
  run "qm stop ${VMID} >/dev/null 2>&1 || true"
  run "qm template ${VMID} >/dev/null 2>&1 || true"
  run "qm config ${VMID} | grep -E '^(arch|machine|bios|efidisk|template|net0):' || true"
  echo "==> done — set PROXMOX_ARM64_TEMPLATE=${VMID} on mgmt"
  exit 0
fi

# Must run as real root (not API token): arch=aarch64 is root-only.
run "qm create ${VMID} \
  --name $(printf '%q' "$NAME") \
  --memory 1024 --cores 1 --cpu max \
  --arch aarch64 --machine virt --bios ovmf \
  --scsihw virtio-scsi-single \
  --net0 ${NET0_SPEC} \
  --ostype l26 --agent enabled=1 \
  --serial0 socket --vga serial0 \
  --efidisk0 ${STORAGE}:1,efitype=4m,pre-enrolled-keys=0"

run "qm template ${VMID}"
run "qm config ${VMID} | grep -E '^(arch|machine|bios|efidisk|template|name):' || true"

cat <<EOF

==> template ready (VMID=${VMID})

On mgmt host:
  echo 'PROXMOX_ARM64_TEMPLATE=${VMID}' | sudo tee -a /etc/pertisk-mgmt/pertisk-mgmt.env
  # prefer API-only like amd64:
  sudo sed -i 's/^PROXMOX_NO_SSH=.*/PROXMOX_NO_SSH=1/' /etc/pertisk-mgmt/pertisk-mgmt.env
  sudo sed -i 's/^PROXMOX_SSH=/# PROXMOX_SSH=/' /etc/pertisk-mgmt/pertisk-mgmt.env
  sudo systemctl restart pertisk-mgmt

New arm64 clusters will API-clone this template (no SSH), then import the cloud qcow2.
EOF
