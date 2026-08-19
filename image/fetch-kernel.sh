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

DOCKER_NET=()
if [[ "$(uname -s)" == Linux ]]; then
  # Self-hosted CI: docker-bridge DNS to dl-cdn.alpinelinux.org often flakes.
  DOCKER_NET+=(--network host)
fi

ARCH="${PERTISK_ARCH:-amd64}"
case "${ARCH}" in
  amd64)
    KERNEL_OUT="${OUT}/bzImage"
    MODULES_OUT="${OUT}/modules-amd64"
    ;;
  arm64)
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
  # af_packet: kube-vip gratuitous ARP (CONFIG_PACKET=m on linux-virt)
  # nfs.ko: in-tree NFS PVs / nfs-subdir-external-provisioner (ENODEV without it)
  # fscache+netfs: required deps for nfs on linux-virt 6.6+
  # vmwgfx/simpledrm: ESXi Host Client VGA (CONFIG_FB=m — without these the
  # console freezes at "EFI stub: Loaded initrd..." even when the guest is fine)
  [[ -f "${MODULES_OUT}/virtio_net.ko" && -f "${MODULES_OUT}/scsi_common.ko" && -f "${MODULES_OUT}/scsi_mod.ko" && -f "${MODULES_OUT}/virtio_scsi.ko" && -f "${MODULES_OUT}/sd_mod.ko" && -f "${MODULES_OUT}/t10-pi.ko" && -f "${MODULES_OUT}/crc64.ko" && -f "${MODULES_OUT}/ext4.ko" && -f "${MODULES_OUT}/jbd2.ko" && -f "${MODULES_OUT}/overlay.ko" && -f "${MODULES_OUT}/vxlan.ko" && -f "${MODULES_OUT}/nf_tables.ko" && -f "${MODULES_OUT}/br_netfilter.ko" && -f "${MODULES_OUT}/x_tables.ko" && -f "${MODULES_OUT}/xt_tcpudp.ko" && -f "${MODULES_OUT}/xt_CT.ko" && -f "${MODULES_OUT}/xfrm_user.ko" && -f "${MODULES_OUT}/af_packet.ko" && -f "${MODULES_OUT}/nfs.ko" && -f "${MODULES_OUT}/nfsv3.ko" && -f "${MODULES_OUT}/nfsv4.ko" && -f "${MODULES_OUT}/sunrpc.ko" && -f "${MODULES_OUT}/fscache.ko" && -f "${MODULES_OUT}/netfs.ko" && -f "${MODULES_OUT}/mptspi.ko" && -f "${MODULES_OUT}/e1000e.ko" && -f "${MODULES_OUT}/vmxnet3.ko" && -f "${MODULES_OUT}/vmwgfx.ko" && -f "${MODULES_OUT}/simpledrm.ko" && -f "${MODULES_OUT}/sr_mod.ko" && -f "${MODULES_OUT}/version" ]] && NEED_MODULES=0
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

# Map ARCH to Alpine APK architecture name.
case "${ARCH}" in
  amd64) APK_ARCH=x86_64 ;;
  arm64) APK_ARCH=aarch64 ;;
esac

echo "==> extracting linux-virt kernel/modules via alpine (${ARCH})"
# Run in an amd64 container even for arm64 — we only extract files from the
# foreign APK, never execute arm64 binaries (QEMU binfmt is unavailable on
# the self-hosted runner).
case "$(uname -m)" in
  x86_64 | amd64) HOST_PLATFORM=linux/amd64 ;;
  aarch64 | arm64) HOST_PLATFORM=linux/arm64 ;;
  *) HOST_PLATFORM=linux/amd64 ;;
esac
docker run --rm \
  --platform "${HOST_PLATFORM}" \
  ${DOCKER_NET[@]+"${DOCKER_NET[@]}"} \
  -v "${OUT}:/out" \
  -v "${ROOT}/image/apk-retry.sh:/apk-retry.sh:ro" \
  -e "NEED_KERNEL=${NEED_KERNEL}" \
  -e "NEED_MODULES=${NEED_MODULES}" \
  -e "KERNEL_NAME=$(basename "${KERNEL_OUT}")" \
  -e "MODULES_NAME=$(basename "${MODULES_OUT}")" \
  -e "APK_ARCH=${APK_ARCH}" \
  alpine:3.20 sh -c '
  set -e
  # Install gzip (for .ko.gz) plus apk tools; then fetch the foreign-arch linux-virt.
  sh /apk-retry.sh gzip
  # Fetch linux-virt for the target arch into a staging root (no exec).
  STAGING=/tmp/alpine-root
  mkdir -p "${STAGING}/etc/apk"
  cp /etc/apk/repositories "${STAGING}/etc/apk/"
  apk fetch --root "${STAGING}" --arch "${APK_ARCH}" --no-cache -o /tmp linux-virt 2>/dev/null || true
  # Fallback: direct fetch if apk fetch does not support --arch.
  if ! ls /tmp/linux-virt-*.apk >/dev/null 2>&1; then
    . /etc/os-release
    ver=$(echo "$VERSION_ID" | cut -d. -f1,2)
    url="https://dl-cdn.alpinelinux.org/alpine/v${ver}/main/${APK_ARCH}"
    idx=$(wget -qO- "${url}/APKINDEX.tar.gz" | tar -tzf - 2>/dev/null | head -1 || true)
    # Just download the package directly.
    pkg=$(wget -qO- "${url}/" 2>/dev/null | sed -n "s/.*href=\"\\(linux-virt-[^\"]*\\.apk\\)\".*/\\1/p" | head -1 || true)
    if [ -n "$pkg" ]; then
      wget -q "${url}/${pkg}" -O "/tmp/${pkg}"
    else
      # Use apk with --root to a fake sysroot.
      apk add --root "${STAGING}" --arch "${APK_ARCH}" --no-cache --no-scripts --initdb linux-virt 2>/dev/null || {
        echo "Could not fetch linux-virt for ${APK_ARCH}" >&2
        exit 1
      }
    fi
  fi

  # Extract: APK is a gzipped tar.
  EXTRACT=/tmp/extract
  mkdir -p "${EXTRACT}"
  if ls /tmp/linux-virt-*.apk >/dev/null 2>&1; then
    for f in /tmp/linux-virt-*.apk; do
      tar -xzf "$f" -C "${EXTRACT}" 2>/dev/null || true
    done
  fi
  # Also check apk --root install path.
  if [ -d "${STAGING}/lib/modules" ]; then
    cp -a "${STAGING}/." "${EXTRACT}/"
  fi

  KVER=$(ls "${EXTRACT}/lib/modules" 2>/dev/null | head -1)
  if [ -z "${KVER}" ]; then
    echo "ERROR: no kernel modules found after extraction" >&2
    find "${EXTRACT}" -maxdepth 3 >&2 || true
    exit 1
  fi
  echo "KVER=$KVER"

  if [ "${NEED_KERNEL}" = "1" ]; then
    img=$(ls "${EXTRACT}"/boot/vmlinuz* 2>/dev/null | head -1)
    [ -n "$img" ] || img=$(find "${EXTRACT}" -name "vmlinuz*" -o -name "bzImage" | head -1)
    [ -n "$img" ] || { echo "kernel image not found" >&2; exit 1; }
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
      src=$(find "${EXTRACT}/lib/modules/${KVER}" \( -name "${name}.ko.gz" -o -name "${name}.ko" \) | head -1)
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

    for name in failover net_failover virtio_net \
                scsi_common scsi_mod virtio_scsi virtio_blk sd_mod \
                cdrom sr_mod isofs ata_piix ahci \
                scsi_transport_spi mptbase mptscsih mptspi \
                e1000e vmxnet3 \
                simpledrm vmwgfx \
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
                cls_bpf act_bpf sch_fq \
                af_packet \
                netfs fscache \
                sunrpc lockd grace nfs nfsv2 nfsv3 nfsv4 auth_rpcgss; do
      copy_module "$name"
    done
    printf "%s\n" "$KVER" > "/out/${MODULES_NAME}/version"
  fi
'

ls -lh "${KERNEL_OUT}"
ls -lh "${MODULES_OUT}"
echo "==> wrote ${KERNEL_OUT} and ${MODULES_OUT}"
