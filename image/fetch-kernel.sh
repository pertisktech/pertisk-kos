#!/usr/bin/env bash
# Fetch a prebuilt Linux kernel + essential virtio modules (Alpine linux-virt).
# Usage:
#   ./image/fetch-kernel.sh                 # amd64 → out/bzImage + out/modules-amd64/
#   PERTISK_ARCH=arm64 ./image/fetch-kernel.sh  # → out/vmlinuz-arm64 + out/modules-arm64/
#   PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh  # re-download
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${ROOT}/out"
mkdir -p "${OUT}"

ARCH="${PERTISK_ARCH:-amd64}"
case "${ARCH}" in
  amd64)
    PLATFORM=linux/amd64
    KERNEL_OUT="${OUT}/bzImage"
    MODULES_OUT="${OUT}/modules-amd64"
    ;;
  arm64)
    PLATFORM=linux/arm64
    KERNEL_OUT="${OUT}/vmlinuz-arm64"
    MODULES_OUT="${OUT}/modules-arm64"
    ;;
  *)
    echo "unsupported PERTISK_ARCH=${ARCH}" >&2
    exit 1
    ;;
esac

NEED_KERNEL=1
NEED_MODULES=1
if [[ "${PERTISK_FORCE_KERNEL:-0}" != "1" ]]; then
  [[ -f "${KERNEL_OUT}" ]] && NEED_KERNEL=0
  # sd_mod needs t10-pi → crc64-rocksoft → crc64; require the leaf dep as a
  # freshness check so older module packs without SCSI deps get refreshed.
  # SCSI disk nodes need sd_mod deps; STATE/EPHEMERAL mounts need ext4.
  [[ -f "${MODULES_OUT}/virtio_net.ko" && -f "${MODULES_OUT}/sd_mod.ko" && -f "${MODULES_OUT}/t10-pi.ko" && -f "${MODULES_OUT}/crc64.ko" && -f "${MODULES_OUT}/ext4.ko" && -f "${MODULES_OUT}/jbd2.ko" && -f "${MODULES_OUT}/overlay.ko" && -f "${MODULES_OUT}/vxlan.ko" && -f "${MODULES_OUT}/nf_tables.ko" && -f "${MODULES_OUT}/br_netfilter.ko" && -f "${MODULES_OUT}/x_tables.ko" && -f "${MODULES_OUT}/xt_tcpudp.ko" && -f "${MODULES_OUT}/xt_CT.ko" && -f "${MODULES_OUT}/xfrm_user.ko" && -f "${MODULES_OUT}/version" ]] && NEED_MODULES=0
fi

# Kernel and modules must come from the same linux-virt package (vermagic).
if [[ "${NEED_MODULES}" == "1" ]]; then
  NEED_KERNEL=1
fi

if [[ "${NEED_KERNEL}" == "0" && "${NEED_MODULES}" == "0" ]]; then
  echo "==> kernel + modules already present"
  ls -lh "${KERNEL_OUT}"
  ls -lh "${MODULES_OUT}"
  exit 0
fi

echo "==> extracting linux-virt kernel/modules via alpine (${ARCH})"
docker run --rm --platform "${PLATFORM}" \
  -v "${OUT}:/out" \
  -e "NEED_KERNEL=${NEED_KERNEL}" \
  -e "NEED_MODULES=${NEED_MODULES}" \
  -e "KERNEL_NAME=$(basename "${KERNEL_OUT}")" \
  -e "MODULES_NAME=$(basename "${MODULES_OUT}")" \
  alpine:3.20 sh -c '
  set -e
  apk add --no-cache linux-virt gzip >/dev/null
  KVER=$(ls /lib/modules | head -1)
  echo "KVER=$KVER"

  if [ "${NEED_KERNEL}" = "1" ]; then
    img=$(ls /boot/vmlinuz* | head -1)
    cp "$img" "/out/${KERNEL_NAME}"
    echo "copied kernel $img"
  fi

  if [ "${NEED_MODULES}" = "1" ]; then
    rm -rf "/out/${MODULES_NAME}"
    mkdir -p "/out/${MODULES_NAME}"

    # Recursively copy a module and its depends= chain (modinfo null-records).
    copy_module() {
      name="$1"
      dest="/out/${MODULES_NAME}/${name}.ko"
      if [ -f "$dest" ]; then
        return 0
      fi
      src=$(find "/lib/modules/${KVER}" \( -name "${name}.ko.gz" -o -name "${name}.ko" \) | head -1)
      if [ -z "$src" ]; then
        echo "WARNING: module ${name} not found" >&2
        return 0
      fi
      case "$src" in
        *.gz) gzip -dc "$src" > "$dest" ;;
        *) cp "$src" "$dest" ;;
      esac
      echo "module ${name} <- $src"
      deps=$(tr "\0" "\n" < "$dest" | sed -n "s/^depends=//p" | head -1 | tr "," " ")
      for dep in $deps; do
        [ -n "$dep" ] && copy_module "$dep"
      done
    }

    # Roots: NIC + SCSI/blk disk (Proxmox virtio-scsi / QEMU virtio-blk)
    # + ext4/vfat (STATE/EPHEMERAL/EFI mounts; linux-virt builds these as modules)
    # + overlay (containerd)
    # + Flannel/Calico/CNI bridge (llc/stp/bridge/br_netfilter/veth)
    # + Calico IPIP + ipset (iptables dataplane)
    # + kube-proxy iptables (xt_tcpudp/xt_nat/xt_statistic/…)
    # + Cilium datapath (vxlan, nft/iptables, xt_socket, xfrm_user for NETLINK_XFRM,
    #   inet_diag for socket LB, cls_bpf/sch_fq for tc)
    for name in failover net_failover virtio_net virtio_scsi virtio_blk sd_mod \
                ext4 crc32c_generic vfat nls_cp437 nls_iso8859-1 overlay \
                llc stp bridge br_netfilter veth \
                tunnel4 ipip \
                nfnetlink nf_tables nft_compat \
                ip_set ip_set_hash_ip ip_set_hash_net xt_set \
                ip_tables iptable_filter iptable_nat \
                iptable_mangle iptable_raw \
                ip6_tables ip6table_filter ip6table_nat \
                ip6table_mangle ip6table_raw \
                xt_mark xt_conntrack \
                nf_socket_ipv4 nf_socket_ipv6 xt_socket \
                x_tables \
                xt_tcpudp xt_nat xt_statistic xt_multiport xt_MASQUERADE xt_addrtype \
                xt_comment xt_CT xt_TPROXY xt_REDIRECT xt_rpfilter \
                nf_tproxy_ipv4 nf_tproxy_ipv6 \
                nf_conntrack nf_nat vxlan geneve \
                xfrm_algo xfrm_user \
                inet_diag tcp_diag udp_diag \
                cls_bpf act_bpf sch_fq; do
      copy_module "$name"
    done
    printf "%s\n" "$KVER" > "/out/${MODULES_NAME}/version"
  fi
'

ls -lh "${KERNEL_OUT}"
ls -lh "${MODULES_OUT}"
echo "==> wrote ${KERNEL_OUT} and ${MODULES_OUT}"
