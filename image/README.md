# OS image / QEMU (M1)

Build a minimal bootable smoke environment: Linux kernel + initramfs where
`/init` is `pertiskd`. STATE is baked at `/system/state` (including config).

## Prerequisites

- Docker (builds musl `pertiskd` + packs cpio)
- QEMU: `brew install qemu`
- Optional: `cargo` on the host for local M1 dev without QEMU

## Quick path

```bash
./image/build-initramfs.sh   # → out/initramfs.cpio.gz (linux/amd64)
# or: make build VERSION=0.2.0 ARCH=amd64
./image/fetch-kernel.sh      # → out/bzImage (Alpine virt, amd64)
brew install qemu            # once
./image/run-qemu.sh
```

Defaults to **linux/amd64** so artifacts match `qemu-system-x86_64` on Apple Silicon.

Production images omit `/bin/busybox` (API-only). For a recovery shell:

```bash
make build PROFILE=debug
# → out/initramfs-amd64-debug.cpio.gz
```

You should see serial logs roughly like:

```
pertiskd starting ... is_pid1=true
STATE ready ...
config loaded ...
hostname applied hostname="pertisk-qemu-1"
M1 smoke complete
```

## Local (no QEMU)

```bash
mkdir -p /tmp/pertisk-state
cp examples/worker.yaml /tmp/pertisk-state/config.yaml
cargo run -p pertiskd -- --state-dir /tmp/pertisk-state --smoke
```

## Disk + network (M2)

```bash
./image/create-disk.sh
./image/run-qemu-disk.sh   # virtio disk + user NIC; GPT install on /dev/vda
```

## Runtime (M3)

```bash
./image/fetch-runtime.sh                 # containerd + kubelet + runc + CNI loopback
PERTISK_EMBED_RUNTIME=1 ./image/build-initramfs.sh
./image/fetch-kernel.sh
# provide examples/worker-join.yaml with real endpoint/token/ca in STATE
./image/run-qemu-disk.sh
```

Without embedded binaries, `pertiskd` logs `containerd=absent kubelet=absent` and continues.
With binaries + valid `cluster:` config, kubelet should register and the node becomes Ready
once networking/CNI beyond loopback is configured for your cluster.

## Cloud golden image

```bash
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
./image/build-cloud-image.sh
PERTISK_DISK=out/pertisk-cloud-amd64.raw ./image/run-qemu-uefi.sh
```

Details: [cloud/README.md](./cloud/README.md).
