#!/usr/bin/env bash
# Install / configure NFS server on this host for lab RWX storage.
# Typical: run on mgmt (e.g. almalinux@10.1.1.150).
#
#   sudo ./scripts/lab-nfs-server.sh
#   sudo ./scripts/lab-nfs-server.sh --export /mnt/nfs_share --subnet 10.1.1.0/24
set -euo pipefail

EXPORT_PATH="/mnt/nfs_share"
SUBNET="*"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --export) EXPORT_PATH="$2"; shift 2 ;;
    --subnet) SUBNET="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,8p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root (sudo)" >&2
  exit 1
fi

if command -v dnf >/dev/null 2>&1; then
  dnf install -y nfs-utils
elif command -v apt-get >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y nfs-kernel-server
else
  echo "unsupported distro (need dnf or apt)" >&2
  exit 1
fi

mkdir -p "$EXPORT_PATH"
chmod 777 "$EXPORT_PATH"

LINE="${EXPORT_PATH} ${SUBNET}(rw,sync,no_subtree_check,no_root_squash)"
if [[ -f /etc/exports ]] && grep -qF "$EXPORT_PATH" /etc/exports; then
  echo "==> updating existing export for ${EXPORT_PATH}"
  # shellcheck disable=SC2016
  grep -vF "$EXPORT_PATH" /etc/exports > /etc/exports.tmp || true
  mv /etc/exports.tmp /etc/exports
fi
echo "$LINE" >> /etc/exports

exportfs -ra
systemctl enable --now nfs-server 2>/dev/null || systemctl enable --now nfs-kernel-server
exportfs -v
echo "==> NFS export ready: $(hostname -I 2>/dev/null | awk '{print $1}'):${EXPORT_PATH}"
echo "    Cluster nodes need the nfs-client image extension (see image/extensions/nfs-client/)."
