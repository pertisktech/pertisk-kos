#!/bin/sh
# Extract Alpine tools binaries + musl libs for the TARGETARCH.
# Runs on the build host (always amd64). For arm64 targets, installs foreign-
# arch packages via apk --root --arch without executing any aarch64 code.
#
# Output: /tools/rootfs/ — a tree matching the final initramfs layout:
#   usr/sbin/  bin/  sbin/  usr/bin/  lib/  etc/ssl/certs/  usr/lib/xtables/
#   usr/lib/pertisk/.busybox-debug
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
  cp /etc/apk/keys/* "${ROOT}/etc/apk/keys/" 2>/dev/null || true

  echo "==> cross-installing ${APK_ARCH} packages into ${ROOT}"
  n=0; max=10; ok=0
  while [ "$n" -lt "$max" ]; do
    n=$((n + 1))
    echo "==> apk --root (attempt ${n}/${max})" >&2
    if apk add --initdb --root "${ROOT}" --arch "${APK_ARCH}" \
         --no-cache --no-scripts --allow-untrusted ${PKGS} 2>&1; then
      ok=1; break
    fi
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
  echo "==> sysroot installed OK"
fi

# Build the final rootfs tree that the Dockerfile will COPY in one shot.
OUT="/tools/rootfs"
mkdir -p "${OUT}/usr/sbin" "${OUT}/usr/bin" "${OUT}/bin" "${OUT}/sbin" \
         "${OUT}/lib" "${OUT}/usr/lib/pertisk" "${OUT}/usr/lib/xtables" \
         "${OUT}/etc/ssl/certs"

# Find a file by name inside the sysroot (or host root).
find_file() {
  local name="$1"
  if [ -n "${ROOT}" ]; then
    find "${ROOT}" -name "${name}" \( -type f -o -type l \) 2>/dev/null | head -1
  else
    for d in /usr/bin /usr/sbin /bin /sbin /usr/lib; do
      [ -e "${d}/${name}" ] && { echo "${d}/${name}"; return; }
    done
  fi
}

# Copy a binary to a destination, resolving symlinks in the sysroot.
# Alpine cross sysroots often have dangling symlinks (e.g. mkfs.ext4 → mke2fs)
# so we resolve and follow them.
copy_to() {
  local name="$1" dest="$2"
  local src
  src="$(find_file "$name")"
  if [ -z "$src" ]; then
    echo "WARN: ${name} not found" >&2
    return 1
  fi
  # If it's a symlink, find the target in the sysroot.
  if [ -L "$src" ]; then
    local target
    target="$(readlink "$src")"
    # Relative symlink — resolve against parent dir.
    case "$target" in
      /*) target="${ROOT}${target}" ;;
      *)  target="$(dirname "$src")/${target}" ;;
    esac
    if [ -f "$target" ]; then
      cp "$target" "${dest}/${name}"
      echo "  ${name} <- ${target} (via symlink)"
      return 0
    fi
    # Symlink target might also be a name we can find.
    local base_target
    base_target="$(basename "$target")"
    local resolved
    resolved="$(find_file "$base_target")"
    if [ -n "$resolved" ] && [ -f "$resolved" ]; then
      cp "$resolved" "${dest}/${name}"
      echo "  ${name} <- ${resolved} (resolved symlink target ${base_target})"
      return 0
    fi
  fi
  if [ -f "$src" ]; then
    cp "$src" "${dest}/${name}"
    echo "  ${name} <- ${src}"
    return 0
  fi
  echo "WARN: ${name} found at ${src} but not a regular file" >&2
  return 1
}

echo "==> extracting binaries"

# usr/sbin/ tools
for name in sgdisk partprobe mkfs.ext4 mkfs.vfat resize2fs tune2fs blkid; do
  copy_to "$name" "${OUT}/usr/sbin" || true
done
# mke2fs is the real binary; mkfs.ext4 is often a symlink to it.
if [ ! -f "${OUT}/usr/sbin/mkfs.ext4" ]; then
  if copy_to "mke2fs" "${OUT}/usr/sbin"; then
    cp "${OUT}/usr/sbin/mke2fs" "${OUT}/usr/sbin/mkfs.ext4"
  fi
fi

# busybox → usr/lib/pertisk/.busybox-debug
copy_to "busybox" "${OUT}/usr/lib/pertisk" || true
if [ -f "${OUT}/usr/lib/pertisk/busybox" ]; then
  cp "${OUT}/usr/lib/pertisk/busybox" "${OUT}/usr/lib/pertisk/.busybox-debug"
  rm -f "${OUT}/usr/lib/pertisk/busybox"
fi

# qemu-ga → usr/bin/
copy_to "qemu-ga" "${OUT}/usr/bin" || true

# mount/umount → bin/
copy_to "mount" "${OUT}/bin" || true
copy_to "umount" "${OUT}/bin" || true

# ip → sbin/
copy_to "ip" "${OUT}/sbin" || true

# NFS helpers → sbin/
copy_to "mount.nfs" "${OUT}/sbin" || true
# mount.nfs4, umount.nfs, umount.nfs4 are usually symlinks to mount.nfs
if ! copy_to "mount.nfs4" "${OUT}/sbin" 2>/dev/null; then
  [ -f "${OUT}/sbin/mount.nfs" ] && cp "${OUT}/sbin/mount.nfs" "${OUT}/sbin/mount.nfs4"
fi
if ! copy_to "umount.nfs" "${OUT}/sbin" 2>/dev/null; then
  [ -f "${OUT}/sbin/mount.nfs" ] && cp "${OUT}/sbin/mount.nfs" "${OUT}/sbin/umount.nfs"
fi
if ! copy_to "umount.nfs4" "${OUT}/sbin" 2>/dev/null; then
  [ -f "${OUT}/sbin/mount.nfs" ] && cp "${OUT}/sbin/mount.nfs" "${OUT}/sbin/umount.nfs4"
fi

# iptables
if copy_to "xtables-legacy-multi" "${OUT}/usr/sbin"; then
  for link in iptables iptables-legacy iptables-save iptables-restore \
              ip6tables ip6tables-legacy; do
    ln -sf xtables-legacy-multi "${OUT}/usr/sbin/${link}"
  done
  ln -sf /usr/sbin/iptables "${OUT}/sbin/iptables" 2>/dev/null || true
  ln -sf /usr/sbin/iptables-legacy "${OUT}/sbin/iptables-legacy" 2>/dev/null || true
fi

# xtables shared objects
if [ -n "${ROOT}" ]; then
  xtdir=$(find "${ROOT}" -type d -name xtables 2>/dev/null | head -1)
else
  xtdir="/usr/lib/xtables"
fi
if [ -n "$xtdir" ] && [ -d "$xtdir" ]; then
  cp -a "${xtdir}/." "${OUT}/usr/lib/xtables/"
fi

# CA certs — arch-independent, use host copy.
if [ -f /etc/ssl/certs/ca-certificates.crt ]; then
  cp /etc/ssl/certs/ca-certificates.crt "${OUT}/etc/ssl/certs/"
else
  apk add --no-cache ca-certificates
  cp /etc/ssl/certs/ca-certificates.crt "${OUT}/etc/ssl/certs/"
fi

echo "==> extracting shared libs"
# Musl shared libs.
if [ -n "${ROOT}" ]; then
  find "${ROOT}" \( -name '*.so' -o -name '*.so.*' \) -type f 2>/dev/null | while read -r lib; do
    cp -n "$lib" "${OUT}/lib/" 2>/dev/null || true
  done
  # Also copy symlinks (e.g. libfoo.so.1 → libfoo.so.1.2).
  find "${ROOT}" \( -name '*.so' -o -name '*.so.*' \) -type l 2>/dev/null | while read -r lib; do
    cp -an "$lib" "${OUT}/lib/" 2>/dev/null || true
  done
else
  for d in /lib /usr/lib; do
    for lib in "${d}"/*.so* "${d}"/*.so; do
      [ -e "$lib" ] || continue
      cp -an "$lib" "${OUT}/lib/" 2>/dev/null || cp -n "$lib" "${OUT}/lib/" || true
    done
  done
fi

# Musl dynamic linker.
case "${TARGETARCH}" in
  amd64) linker_name="ld-musl-x86_64.so.1" ;;
  arm64) linker_name="ld-musl-aarch64.so.1" ;;
esac
src="$(find ${ROOT:-/} -name "${linker_name}" \( -type f -o -type l \) 2>/dev/null | head -1)"
if [ -n "$src" ]; then
  cp -aL "$src" "${OUT}/lib/${linker_name}"
fi

# Verify critical files.
echo "==> verifying"
fail=0
for f in "${OUT}/usr/sbin/sgdisk" "${OUT}/usr/bin/qemu-ga" \
         "${OUT}/bin/mount" "${OUT}/bin/umount" "${OUT}/sbin/ip" \
         "${OUT}/usr/sbin/xtables-legacy-multi" \
         "${OUT}/etc/ssl/certs/ca-certificates.crt"; do
  if [ ! -e "$f" ]; then
    echo "ERROR: missing $f" >&2
    fail=1
  fi
done
[ "$fail" = "0" ] || { echo "==> rootfs tree:"; find "${OUT}" -type f | sort | head -60; exit 1; }

echo "==> tools extracted successfully"
find "${OUT}" -type f | wc -l
echo "files in /tools/rootfs"
