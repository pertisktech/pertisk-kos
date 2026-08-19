#!/bin/sh
# Extract Alpine tools binaries + musl libs for the TARGETARCH.
# Runs on the build host (always amd64). For arm64 targets, installs foreign-
# arch packages via apk --root --arch without executing any aarch64 code.
#
# Output: /tools/{bin,lib,certs,xtables}
set -eu

TARGETARCH="${TARGETARCH:-amd64}"
echo "==> tools-extract TARGETARCH=${TARGETARCH}"

PKGS="sgdisk e2fsprogs e2fsprogs-extra dosfstools busybox parted util-linux ca-certificates
  iptables iptables-legacy nfs-utils qemu-guest-agent iproute2"

case "${TARGETARCH}" in
  amd64) APK_ARCH=x86_64 ;;
  arm64) APK_ARCH=aarch64 ;;
  *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;;
esac

HOST_ARCH="$(apk --print-arch)"

if [ "${APK_ARCH}" = "${HOST_ARCH}" ]; then
  sh /apk-retry.sh ${PKGS}
  ROOT=""
else
  ROOT="/sysroot"
  mkdir -p "${ROOT}/etc/apk" "${ROOT}/etc/apk/keys"
  cp /etc/apk/repositories "${ROOT}/etc/apk/"
  # Copy Alpine signing keys so apk can verify packages.
  cp /etc/apk/keys/* "${ROOT}/etc/apk/keys/" 2>/dev/null || true

  echo "==> cross-installing ${APK_ARCH} packages into ${ROOT}"
  n=0; max=10
  ok=0
  while [ "$n" -lt "$max" ]; do
    n=$((n + 1))
    echo "==> apk --root (attempt ${n}/${max})" >&2
    if apk add --initdb --root "${ROOT}" --arch "${APK_ARCH}" \
         --no-cache --no-scripts --allow-untrusted ${PKGS} 2>&1; then
      ok=1; break
    fi
    # Rotate mirrors on failure.
    . /etc/os-release
    ver=$(echo "$VERSION_ID" | cut -d. -f1,2)
    case $(( (n - 1) % 5 )) in
      0) base="https://dl-cdn.alpinelinux.org/alpine" ;;
      1) base="https://mirrors.edge.kernel.org/alpine" ;;
      2) base="https://uk.alpinelinux.org/alpine" ;;
      3) base="https://mirror.csclub.uwaterloo.ca/alpine" ;;
      4) base="https://dl-4.alpinelinux.org/alpine" ;;
    esac
    printf '%s\n' "${base}/v${ver}/main" "${base}/v${ver}/community" \
      > "${ROOT}/etc/apk/repositories"
    sleep $((n * 3))
  done
  [ "$ok" = "1" ] || { echo "cross apk install failed after ${max} attempts" >&2; exit 1; }

  echo "==> sysroot contents:"
  find "${ROOT}" -maxdepth 4 -type f | head -80
fi

mkdir -p /tools/bin /tools/lib /tools/certs /tools/xtables

# Find a binary by name inside the sysroot (or host root).
# Alpine sysroots may not have /bin→/usr/bin symlinks.
find_bin() {
  local name="$1"
  local found=""
  if [ -n "${ROOT}" ]; then
    found=$(find "${ROOT}" -name "${name}" -type f 2>/dev/null | head -1)
  fi
  if [ -z "$found" ] && [ -z "${ROOT}" ]; then
    # Native install: check standard paths.
    for d in /usr/bin /usr/sbin /bin /sbin; do
      [ -f "${d}/${name}" ] && { found="${d}/${name}"; break; }
    done
  fi
  echo "$found"
}

# Copy a binary to /tools/bin/ by name.
copy_bin() {
  local name="$1"
  local src
  src="$(find_bin "$name")"
  if [ -n "$src" ] && [ -f "$src" ]; then
    cp "$src" "/tools/bin/${name}"
    echo "  bin: ${name} <- ${src}"
  else
    echo "WARN: ${name} not found" >&2
  fi
}

# Required binaries.
for name in sgdisk partprobe mkfs.ext4 mkfs.vfat resize2fs tune2fs blkid \
            busybox qemu-ga mount umount ip \
            mount.nfs mount.nfs4 umount.nfs umount.nfs4; do
  copy_bin "$name"
done

# iptables — may be named xtables-legacy-multi or in various paths.
xtables_src="$(find_bin xtables-legacy-multi)"
if [ -n "$xtables_src" ] && [ -f "$xtables_src" ]; then
  cp "$xtables_src" /tools/bin/xtables-legacy-multi
  echo "  bin: xtables-legacy-multi <- ${xtables_src}"
else
  echo "WARN: xtables-legacy-multi not found" >&2
fi
ln -sf xtables-legacy-multi /tools/bin/iptables
ln -sf xtables-legacy-multi /tools/bin/iptables-legacy
ln -sf xtables-legacy-multi /tools/bin/iptables-save
ln -sf xtables-legacy-multi /tools/bin/iptables-restore
ln -sf xtables-legacy-multi /tools/bin/ip6tables
ln -sf xtables-legacy-multi /tools/bin/ip6tables-legacy

# xtables shared objects.
if [ -n "${ROOT}" ]; then
  xtdir=$(find "${ROOT}" -type d -name xtables 2>/dev/null | head -1)
else
  xtdir="/usr/lib/xtables"
fi
if [ -n "$xtdir" ] && [ -d "$xtdir" ]; then
  cp -a "${xtdir}/." /tools/xtables/
fi

# CA certs — arch-independent. With --no-scripts the cross sysroot won't have
# the generated bundle. Use the host copy.
if [ -f /etc/ssl/certs/ca-certificates.crt ]; then
  cp /etc/ssl/certs/ca-certificates.crt /tools/certs/
else
  apk add --no-cache ca-certificates
  cp /etc/ssl/certs/ca-certificates.crt /tools/certs/
fi

# Musl shared libs — glob copy from sysroot or host.
if [ -n "${ROOT}" ]; then
  find "${ROOT}" -name '*.so' -o -name '*.so.*' 2>/dev/null | while read -r lib; do
    [ -f "$lib" ] || continue
    cp -n "$lib" /tools/lib/ 2>/dev/null || true
  done
else
  for d in /lib /usr/lib; do
    for lib in "${d}"/*.so* "${d}"/*.so; do
      [ -e "$lib" ] || continue
      cp -an "$lib" /tools/lib/ 2>/dev/null || cp -n "$lib" /tools/lib/ || true
    done
  done
fi

# Musl dynamic linker.
case "${TARGETARCH}" in
  amd64)
    src="$(find ${ROOT:-/} -name 'ld-musl-x86_64.so.1' -type f 2>/dev/null | head -1)"
    [ -n "$src" ] && cp "$src" /tools/lib/
    ;;
  arm64)
    src="$(find ${ROOT:-/} -name 'ld-musl-aarch64.so.1' -type f 2>/dev/null | head -1)"
    [ -n "$src" ] && cp "$src" /tools/lib/
    ;;
esac

# Verify critical files.
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

echo "==> tools extracted successfully"
ls /tools/bin/
echo "---"
ls /tools/lib/ | wc -l
echo "shared libs copied"
