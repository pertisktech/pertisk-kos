#!/usr/bin/env bash
# Fix Proxmox VM boot settings for an existing Pertisk worker (UEFI + serial).
#
#   PROXMOX_SSH=root@10.1.1.197 ./scripts/proxmox-fix-boot.sh 9100
#   # or on the node: ./scripts/proxmox-fix-boot.sh 9100
set -euo pipefail

VMID="${1:?usage: $0 <vmid>}"
SSH_HOST="${PROXMOX_SSH:-}"

# Run a whole script on the node in one SSH session (one password prompt).
remote_bash() {
  local script="$1"
  if [[ -n "${SSH_HOST}" ]]; then
    ssh -o StrictHostKeyChecking=accept-new "${SSH_HOST}" "bash -s" <<EOF
set -euo pipefail
${script}
EOF
  else
    bash -c "set -euo pipefail; ${script}"
  fi
}

echo "==> fixing boot for VM ${VMID}"

remote_bash "
VMID=${VMID}
STORAGE=${PROXMOX_STORAGE:-local-zfs}

qm stop \"\${VMID}\" >/dev/null 2>&1 || true
for i in 1 2 3 4 5 6 7 8 9 10; do
  qm status \"\${VMID}\" | grep -q stopped && break
  sleep 1
done

CONF=\$(qm config \"\${VMID}\")
echo \"\$CONF\" | sed -n '1,40p'

# Attach largest unused disk as scsi0 if scsi0 is missing OR scsi0 is tiny (<1GiB).
SCSI0=\$(echo \"\$CONF\" | sed -n 's/^scsi0: \\([^,]*\\).*/\\1/p')
SCSI0_SIZE=0
if [[ -n \"\$SCSI0\" ]]; then
  SCSI0_SIZE=\$(pvesm list \"\${STORAGE}\" 2>/dev/null | awk -v v=\"\$SCSI0\" '\$1==v {print \$4; exit}')
  SCSI0_SIZE=\${SCSI0_SIZE:-0}
fi
NEED_ATTACH=0
if [[ -z \"\$SCSI0\" ]]; then
  NEED_ATTACH=1
elif [[ \"\$SCSI0_SIZE\" -gt 0 && \"\$SCSI0_SIZE\" -lt 1073741824 ]]; then
  echo \"==> scsi0 is only \${SCSI0_SIZE} bytes — reattach real OS disk\"
  NEED_ATTACH=1
fi
if [[ \"\$NEED_ATTACH\" == \"1\" ]]; then
  BEST=\"\"; BEST_SIZE=0; BEST_KEY=\"\"; BEST_N=-1
  while IFS= read -r line; do
    key=\$(echo \"\$line\" | sed -n 's/^\\(unused[0-9]*\\):.*/\\1/p')
    vol=\$(echo \"\$line\" | sed -n 's/^unused[0-9]*: //p')
    n=\$(echo \"\$vol\" | sed -n 's/.*-disk-\\([0-9]*\\)\$/\\1/p')
    n=\${n:--1}
    size=\$(pvesm list \"\${STORAGE}\" 2>/dev/null | awk -v v=\"\$vol\" '\$1==v {print \$4; exit}')
    size=\${size:-0}
    if [[ \"\$size\" -gt \"\$BEST_SIZE\" || ( \"\$size\" -eq \"\$BEST_SIZE\" && \"\$n\" -gt \"\$BEST_N\" ) ]]; then
      BEST=\$vol; BEST_SIZE=\$size; BEST_KEY=\$key; BEST_N=\$n
    fi
  done < <(echo \"\$CONF\" | grep '^unused' || true)
  if [[ -n \"\$BEST\" && \"\$BEST_SIZE\" -ge 1073741824 ]]; then
    echo \"==> attaching unused disk as scsi0: \$BEST (\$BEST_SIZE bytes)\"
    qm set \"\${VMID}\" --scsihw virtio-scsi-single
    qm set \"\${VMID}\" --scsi0 \"\${BEST}\"
    qm set \"\${VMID}\" --delete \"\${BEST_KEY}\" || true
  else
    echo \"ERROR: no usable unused OS disk (>=1GiB) — re-import the qcow2\" >&2
    qm config \"\${VMID}\"
    exit 1
  fi
fi

qm set \"\${VMID}\" --bios ovmf --machine q35
qm set \"\${VMID}\" --scsihw virtio-scsi-single
qm set \"\${VMID}\" --boot order=scsi0
qm set \"\${VMID}\" --serial0 socket --vga serial0

# Recreate EFI disk without Microsoft Secure Boot keys.
if qm config \"\${VMID}\" | grep -q '^efidisk0:'; then
  ESTOR=\$(qm config \"\${VMID}\" | sed -n 's/^efidisk0: \\([^:]*\\):.*/\\1/p')
  echo \"==> recreating efidisk0 on \${ESTOR} (pre-enrolled-keys=0)\"
  qm set \"\${VMID}\" --delete efidisk0 || true
  qm set \"\${VMID}\" --efidisk0 \"\${ESTOR}:1,efitype=4m,pre-enrolled-keys=0\"
else
  echo \"==> adding efidisk0 on \${STORAGE}\"
  qm set \"\${VMID}\" --efidisk0 \"\${STORAGE}:1,efitype=4m,pre-enrolled-keys=0\"
fi

echo \"==> final config (disks/boot):\"
qm config \"\${VMID}\" | grep -E '^(scsi|unused|efidisk|boot|bios|machine|serial|vga):' || true

qm start \"\${VMID}\"
echo \"==> started \${VMID}\"
echo \"    Console opens serial (vga=serial0). Host: qm terminal \${VMID}\"
"
