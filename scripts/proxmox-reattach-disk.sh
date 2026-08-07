#!/usr/bin/env bash
# Re-attach Pertisk cloud disk to an existing Proxmox VM (when scsi0 is missing).
#
#   PROXMOX_SSH=root@10.1.1.197 ./scripts/proxmox-reattach-disk.sh 9100 out/pertisk-cloud-amd64.qcow2
#   ARCH=arm64 PROXMOX_SSH=root@pve ./scripts/proxmox-reattach-disk.sh 9200 out/pertisk-cloud-arm64.qcow2
set -euo pipefail

VMID="${1:?usage: $0 <vmid> <disk.qcow2>}"
DISK="${2:?usage: $0 <vmid> <disk.qcow2>}"
SSH_HOST="${PROXMOX_SSH:?set PROXMOX_SSH=root@host}"
STORAGE="${PROXMOX_STORAGE:-local-zfs}"
ARCH="${PERTISK_ARCH:-${ARCH:-}}"
if [[ -z "$ARCH" ]]; then
  base="$(basename "$DISK")"
  if [[ "$base" == *arm64* || "$base" == *aarch64* ]]; then
    ARCH=arm64
  else
    ARCH=amd64
  fi
fi
case "$(printf '%s' "$ARCH" | tr '[:upper:]' '[:lower:]')" in
  amd64|x86_64|x64) ARCH=amd64; PVE_MACHINE=q35; PVE_ARCH=x86_64 ;;
  arm64|aarch64) ARCH=arm64; PVE_MACHINE=virt; PVE_ARCH=aarch64 ;;
  *) echo "unsupported ARCH=${ARCH}" >&2; exit 1 ;;
esac

[[ -f "${DISK}" ]] || {
  echo "missing ${DISK}" >&2
  exit 1
}

REMOTE="/var/tmp/pertisk-${VMID}.qcow2"

echo "==> stop VM ${VMID} (arch=${ARCH} machine=${PVE_MACHINE})"
ssh -o StrictHostKeyChecking=accept-new "${SSH_HOST}" "qm stop ${VMID} >/dev/null 2>&1 || true; sleep 2"

echo "==> scp $(basename "${DISK}") → ${SSH_HOST}:${REMOTE}"
scp -o StrictHostKeyChecking=accept-new "${DISK}" "${SSH_HOST}:${REMOTE}"

echo "==> qm importdisk + attach scsi0 + boot order (one SSH session)"
ssh "${SSH_HOST}" bash -s <<EOF
set -euo pipefail
VMID=${VMID}
STORAGE=${STORAGE}
REMOTE=${REMOTE}
PVE_MACHINE=${PVE_MACHINE}
PVE_ARCH=${PVE_ARCH}

qm importdisk "\${VMID}" "\${REMOTE}" "\${STORAGE}" --format qcow2
rm -f "\${REMOTE}"

CONF=\$(qm config "\${VMID}")
# Prefer largest unused (highest disk-N on ties); never attach a 1M stub.
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
if [[ -z "\$BEST_VOL" ]]; then
  echo "ERROR: importdisk did not create unused disk" >&2
  qm config "\${VMID}"
  exit 1
fi
if [[ "\$BEST_SIZE" -gt 0 && "\$BEST_SIZE" -lt 1073741824 ]]; then
  echo "ERROR: best unused disk is only \${BEST_SIZE} bytes (need >=1GiB)" >&2
  exit 1
fi
UKEY=\$BEST_KEY
UVAL=\$BEST_VOL
echo "==> \${UKEY} -> scsi0 (\${UVAL}, \${BEST_SIZE} bytes)"

qm set "\${VMID}" --scsihw virtio-scsi-single
qm set "\${VMID}" --scsi0 "\${UVAL}"
qm set "\${VMID}" --delete "\${UKEY}" || true
qm set "\${VMID}" --boot order=scsi0
qm set "\${VMID}" --arch "\${PVE_ARCH}" --bios ovmf --machine "\${PVE_MACHINE}"
qm set "\${VMID}" --serial0 socket --vga serial0

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
