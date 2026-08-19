#!/bin/sh
# Extract Alpine tools binaries + musl libs for the TARGETARCH.
# Runs on the build host (always amd64). For arm64 targets, installs foreign-
# arch packages via apk --root --arch without executing any aarch64 code.
#
# Output: /tools/{bin,lib,certs,xtables}
set -eu

TARGETARCH="${TARGETARCH:-amd64}"
echo "==> tools-extract TARGETARCH=${TARGETARCH}"

PKGS="sgdisk e2fsprogs e2fsprogs-extra dosfstools busybox parted util-linux ca-certificates \
  iptables iptables-legacy nfs-utils qemu-guest-agent iproute2"

# Map Alpine arch names.
case "${TARGETARCH}" in
  amd64) APK_ARCH=x86_64 ;;
  arm64) APK_ARCH=aarch64 ;;
  *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;;
esac

HOST_ARCH="$(apk --print-arch)"

if [ "${APK_ARCH}" = "${HOST_ARCH}" ]; then
  # Native: install directly.
  sh /apk-retry.sh ${PKGS}
  ROOT=""
else
  # Cross: install into a sysroot for the foreign arch.
  ROOT="/sysroot"
  mkdir -p "${ROOT}/etc/apk"
  cp /etc/apk/repositories "${ROOT}/etc/apk/"
  apk add --initdb --root "${ROOT}" --arch "${APK_ARCH}" --no-cache --no-scripts ${PKGS} || {
    # Retry via apk-retry (mirror rotation).
    # apk-retry writes /etc/apk/repositories inside the container; point it at sysroot.
    cp /etc/apk/repositories "${ROOT}/etc/apk/"
    n=0; max=8
    while [ "$n" -lt "$max" ]; do
      n=$((n + 1))
      echo "==> cross apk install attempt ${n}/${max}" >&2
      if apk add --initdb --root "${ROOT}" --arch "${APK_ARCH}" --no-cache --no-scripts ${PKGS}; then
        break
      fi
      [ "$n" -lt "$max" ] && sleep $((n * 4))
    done
  }
fi

# Helper: resolve a path in sysroot or host.
r() { echo "${ROOT}$1"; }

mkdir -p /tools/bin /tools/lib /tools/certs /tools/xtables

# Binaries.
for src in \
  /usr/bin/sgdisk /usr/sbin/partprobe \
  /sbin/mkfs.ext4 /sbin/mkfs.vfat \
  /usr/sbin/resize2fs /usr/sbin/tune2fs /sbin/blkid \
  /bin/busybox \
  /usr/bin/qemu-ga \
  /bin/mount /bin/umount \
  /sbin/ip \
  /sbin/mount.nfs /sbin/mount.nfs4 /sbin/umount.nfs /sbin/umount.nfs4; do
  [ -f "$(r "$src")" ] && cp "$(r "$src")" /tools/bin/ || echo "WARN: missing $(r "$src")" >&2
done

# CA certs — arch-independent. With --no-scripts the cross-install sysroot won't
# have the bundle (update-ca-certificates never ran). Use the host copy instead.
if [ -f "$(r /etc/ssl/certs/ca-certificates.crt)" ]; then
  cp "$(r /etc/ssl/certs/ca-certificates.crt)" /tools/certs/
elif [ -f /etc/ssl/certs/ca-certificates.crt ]; then
  cp /etc/ssl/certs/ca-certificates.crt /tools/certs/
else
  # Install ca-certificates on the host and generate the bundle.
  apk add --no-cache ca-certificates
  cp /etc/ssl/certs/ca-certificates.crt /tools/certs/
fi

# iptables.
for src in /sbin/xtables-legacy-multi /usr/sbin/xtables-legacy-multi; do
  if [ -f "$(r "$src")" ]; then
    cp -a "$(r "$src")" /tools/bin/xtables-legacy-multi
    break
  fi
done
ln -sf xtables-legacy-multi /tools/bin/iptables
ln -sf xtables-legacy-multi /tools/bin/iptables-legacy
ln -sf xtables-legacy-multi /tools/bin/iptables-save
ln -sf xtables-legacy-multi /tools/bin/iptables-restore
ln -sf xtables-legacy-multi /tools/bin/ip6tables
ln -sf xtables-legacy-multi /tools/bin/ip6tables-legacy
if [ -d "$(r /usr/lib/xtables)" ]; then
  cp -a "$(r /usr/lib/xtables)/." /tools/xtables/
fi

# Musl shared libs — glob copy everything; ldd won't work cross-arch.
for d in /lib /usr/lib; do
  for lib in "$(r "$d")"/*.so* "$(r "$d")"/*.so; do
    [ -e "$lib" ] || continue
    cp -an "$lib" /tools/lib/ 2>/dev/null || cp -n "$lib" /tools/lib/ || true
  done
done

# Musl dynamic linker.
case "${TARGETARCH}" in
  amd64)
    [ -f "$(r /lib/ld-musl-x86_64.so.1)" ] && cp "$(r /lib/ld-musl-x86_64.so.1)" /tools/lib/
    ;;
  arm64)
    [ -f "$(r /lib/ld-musl-aarch64.so.1)" ] && cp "$(r /lib/ld-musl-aarch64.so.1)" /tools/lib/
    ;;
esac

# Verify critical files exist (COPY in Dockerfile will fail otherwise).
fail=0
for f in /tools/bin/sgdisk /tools/bin/busybox /tools/bin/qemu-ga \
         /tools/bin/mount /tools/bin/umount /tools/bin/ip \
         /tools/bin/xtables-legacy-multi \
         /tools/certs/ca-certificates.crt; do
  if [ ! -e "$f" ]; then
    echo "ERROR: missing $f" >&2
    fail=1
  fi
done
[ "$fail" = "0" ] || exit 1

echo "==> tools extracted"
ls /tools/bin/
ls /tools/lib/ | head -20
