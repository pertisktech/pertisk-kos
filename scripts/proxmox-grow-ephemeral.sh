#!/usr/bin/env bash
# Offline-grow EPHEMERAL (partition 6 + ext4) on a Proxmox QEMU VM disk.
#
# Use when Proxmox `qm resize` already enlarged scsi0 but the guest still shows
# the old /var size (GPT grew or not; resize2fs never ran).
#
# Requires SSH to the PVE node (PROXMOX_SSH). Stops the VM briefly.
#
#   PROXMOX_SSH=root@pve ./scripts/proxmox-grow-ephemeral.sh --vmid 210
#   PROXMOX_SSH=root@pve ./scripts/proxmox-grow-ephemeral.sh --vmid 210 --disk-id 1
set -euo pipefail

VMID=""
DISK_ID="${PROXMOX_DISK_ID:-1}"
STORAGE_POOL="${PROXMOX_ZFS_POOL:-rpool/data}"

usage() {
  sed -n '2,14p' "$0"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --vmid) VMID="$2"; shift 2 ;;
    --disk-id) DISK_ID="$2"; shift 2 ;;
    --pool) STORAGE_POOL="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

[[ -n "$VMID" ]] || { echo "ERROR: --vmid required" >&2; exit 1; }
[[ -n "${PROXMOX_SSH:-}" ]] || { echo "ERROR: set PROXMOX_SSH=root@pve" >&2; exit 1; }

ZVOL="${STORAGE_POOL}/vm-${VMID}-disk-${DISK_ID}"

echo "==> grow EPHEMERAL on VM ${VMID} via ${PROXMOX_SSH} (${ZVOL})"

SSH_OPTS=(
  -o BatchMode=yes
  -o StrictHostKeyChecking=accept-new
  -o ConnectTimeout=15
)
# Prefer dedicated key when present (RPM user: /var/lib/pertisk-mgmt/.ssh/…).
if [[ -n "${PROXMOX_SSH_KEY:-}" && -f "${PROXMOX_SSH_KEY}" ]]; then
  SSH_OPTS+=(-i "${PROXMOX_SSH_KEY}")
elif [[ -f "${HOME}/.ssh/id_ed25519" ]]; then
  SSH_OPTS+=(-i "${HOME}/.ssh/id_ed25519")
elif [[ -f "${HOME}/.ssh/id_rsa" ]]; then
  SSH_OPTS+=(-i "${HOME}/.ssh/id_rsa")
fi

ssh "${SSH_OPTS[@]}" "${PROXMOX_SSH}" bash -s <<EOF
set -euo pipefail
VMID=${VMID}
ZVOL=${ZVOL}
DISK="/dev/zvol/\${ZVOL}"

[[ -b "\$DISK" ]] || { echo "missing \$DISK" >&2; exit 1; }

st=\$(qm status "\$VMID" | awk '{print \$2}')
if [[ "\$st" != "stopped" ]]; then
  echo "==> stopping VM \$VMID"
  qm stop "\$VMID" --timeout 120 || true
  for i in \$(seq 1 90); do
    st=\$(qm status "\$VMID" | awk '{print \$2}')
    [[ "\$st" == "stopped" ]] && break
    sleep 2
  done
fi
qm status "\$VMID" | grep -q stopped

sgdisk -e "\$DISK" >/dev/null
# First sector only (do not swallow "3.0 GiB" digits into the number).
START=\$(sgdisk -i 6 "\$DISK" | sed -n 's/^First sector: *\\([0-9][0-9]*\\).*/\\1/p' | head -1)
END=\$(sgdisk -i 6 "\$DISK" | sed -n 's/^Last sector: *\\([0-9][0-9]*\\).*/\\1/p' | head -1)
LAST=\$(sgdisk -p "\$DISK" | awk '/last usable sector is/{print \$NF}')
echo "START=\$START END=\$END LAST=\$LAST"
if [[ -z "\$START" ]]; then
  # Default Pertisk layout: EFI+BOOT*+META+STATE ≈ 3Gi → sector 6359040
  START=6359040
  echo "WARN: no part6; recreating from START=\$START"
fi
if [[ -n "\$LAST" ]] && { [[ -z "\$END" ]] || [[ "\$END" -lt \$((LAST - 2048)) ]]; }; then
  echo "==> expanding GPT part6 to disk end"
  sgdisk -d 6 "\$DISK" >/dev/null 2>&1 || true
  sgdisk -n "6:\${START}:0" -t 6:8300 -c 6:EPHEMERAL "\$DISK" >/dev/null
else
  echo "==> GPT part6 already near disk end"
fi
partprobe "\$DISK" || true
sleep 2
PART=\$(lsblk -lnpo NAME,PARTLABEL "\$DISK" | awk '\$2=="EPHEMERAL"{print \$1; exit}')
[[ -n "\$PART" && -b "\$PART" ]] || { echo "no EPHEMERAL partition" >&2; lsblk "\$DISK"; exit 1; }
echo "PART=\$PART part_bytes=\$(blockdev --getsize64 "\$PART")"
echo "fs before: \$(tune2fs -l "\$PART" | awk '/Block count/{print}')"
e2fsck -f -y "\$PART" || { ec=\$?; [[ \$ec -le 1 ]] || exit \$ec; }
resize2fs "\$PART"
echo "fs after: \$(tune2fs -l "\$PART" | awk '/Block count/{print}')"
qm start "\$VMID"
sleep 2
qm status "\$VMID"
echo "==> done VM \$VMID"
EOF
