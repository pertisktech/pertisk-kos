# OS image / QEMU (M1)

Build a minimal bootable smoke environment: Linux kernel + initramfs where
`/init` is `pertiskd`. STATE is baked at `/system/state` (including config).

## Prerequisites

- Docker (builds musl `pertiskd` + packs cpio)
- QEMU: `brew install qemu`
- Optional: `cargo` on the host for local M1 dev without QEMU

## Quick path

```bash
./image/build-initramfs.sh
./image/fetch-kernel.sh
./image/run-qemu.sh
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

## Disk layout (next: P1)

GPT labels: `EFI`, `BOOT_A`, `BOOT_B`, `META`, `STATE`, `EPHEMERAL`  
Mounts: `/system/state`, `/system/ephemeral`, `/var`
