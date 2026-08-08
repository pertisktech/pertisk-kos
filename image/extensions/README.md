# Image extensions (storage / clients)

Pertisk ships a minimal linux-virt module set. **Storage clients** that need
extra kernel modules or mount helpers are tracked here so we do not forget
them when growing the image.

| Extension | Purpose | In default image? |
|-----------|---------|-------------------|
| [nfs-client](./nfs-client/) | Mount NFS PVs / nfs-subdir-external-provisioner | **Yes** (modules + `mount.nfs`) |
| qemu-guest-agent | Proxmox/QEMU Shutdown + Summary IP (`qemu-ga`) | **Yes** (`/usr/bin/qemu-ga`, started by pertiskd) |
| nfs-server | Export NFS from a node (unusual; prefer external NAS/mgmt) | Docs only — run on mgmt/lab host |

## Build wiring

1. **Kernel modules** — listed in each extension’s `modules.txt`, copied by
   [`../fetch-kernel.sh`](../fetch-kernel.sh) and loaded by `pertiskd` at boot.
2. **Userspace** — packages/binaries noted in the extension README; installed
   via [`../Dockerfile.initramfs`](../Dockerfile.initramfs) tools stage.

After changing modules, force a refresh:

```bash
PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh
make cloud ARCH=amd64
```

Existing clusters need **new guest images** (or a live module inject — not
supported); recreate nodes or rebuild qcow2 and roll VMs.
