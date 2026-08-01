#!/usr/bin/env bash
# Re-attach Pertisk cloud disk to an existing Proxmox VM (when scsi0 is missing).
#
#   PROXMOX_SSH=root@10.1.1.197 ./scripts/proxmox-reattach-disk.sh 9100 out/pertisk-cloud-amd64.qcow2
set -euo pipefail

VMID="${1:?usage: $0 <vmid> <disk.qcow2>}"
DISK="${2:?usage: $0 <vmid> <disk.qcow2>}"
SSH_HOST="${PROXMOX_SSH:?set PROXMOX_SSH=root@host}"
STORAGE="${PROXMOX_STORAGE:-local-zfs}"

[[ -f "${DISK}" ]] || {
  echo "missing ${DISK}" >&2
  exit 1
}

REMOTE="/var/tmp/pertisk-${VMID}.qcow2"

echo "==> stop VM ${VMID}"
ssh -o StrictHostKeyChecking=accept-new "${SSH_HOST}" "qm stop ${VMID} >/dev/null 2>&1 || true; sleep 2"

echo "==> scp $(basename "${DISK}") → ${SSH_HOST}:${REMOTE}"
scp -o StrictHostKeyChecking=accept-new "${DISK}" "${SSH_HOST}:${REMOTE}"

echo "==> qm importdisk + attach scsi0 + boot order (one SSH session)"
ssh "${SSH_HOST}" bash -s <<EOF
set -euo pipefail
VMID=${VMID}
STORAGE=${STORAGE}
REMOTE=${REMOTE}

qm importdisk "\${VMID}" "\${REMOTE}" "\${STORAGE}" --format qcow2
rm -f "\${REMOTE}"

CONF=\$(qm config "\${VMID}")
UNUSED=\$(echo "\$CONF" | sed -n 's/^\\(unused[0-9]*\\): \\(.*\\)/\\1|\\2/p' | head -1)
if [[ -z "\$UNUSED" ]]; then
  echo "ERROR: importdisk did not create unused disk" >&2
  qm config "\${VMID}"
  exit 1
fi
UKEY=\${UNUSED%%|*}
UVAL=\${UNUSED#*|}
echo "==> \${UKEY} -> scsi0 (\${UVAL})"

qm set "\${VMID}" --scsihw virtio-scsi-single
qm set "\${VMID}" --scsi0 "\${UVAL}"
qm set "\${VMID}" --delete "\${UKEY}" || true
qm set "\${VMID}" --boot order=scsi0
qm set "\${VMID}" --bios ovmf --machine q35
qm set "\${VMID}" --serial0 socket --vga std

# Ensure EFI vars without Secure Boot MS keys
if qm config "\${VMID}" | grep -q '^efidisk0:'; then
  ESTOR=\$(qm config "\${VMID}" | sed -n 's/^efidisk0: \\([^:]*\\):.*/\\1/p')
  qm set "\${VMID}" --delete efidisk0 || true
  qm set "\${VMID}" --efidisk0 "\${ESTOR}:1,efitype=4m,pre-enrolled-keys=0"
else
  qm set "\${VMID}" --efidisk0 "\${STORAGE}:1,efitype=4m,pre-enrolled-keys=0"
fi

echo "==> config:"
qm config "\${VMID}" | grep -E '^(scsi|unused|efidisk|boot|bios|serial):' || true
qm start "\${VMID}"
echo "==> started \${VMID} — use Console → Serial (ignore VNC timeout)"
EOF
