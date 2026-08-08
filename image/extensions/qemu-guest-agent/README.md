# Extension: qemu-guest-agent

Enables Proxmox/QEMU **Shutdown**, `guest-ping`, and Summary IP reporting.

## Symptom without this extension

```text
QEMU Guest Agent is not running - VM … qga command 'guest-ping' failed - got timeout
TASK ERROR: VM quit/powerdown failed - got timeout
```

Host side already has `agent=enabled=1` (see `scripts/proxmox-upload-vm.sh`). The guest must run `qemu-ga` on the virtio-serial channel.

## Userspace

Alpine `qemu-guest-agent` → `/usr/bin/qemu-ga` (+ musl deps: glib, numa, uring, …).

`pertiskd` starts it at boot and, without udev, symlinks:

`/dev/virtio-ports/org.qemu.guest_agent.0` → `/dev/vportNpM`

**Shutdown:** Alpine's qemu-ga runs `/sbin/shutdown` then `/sbin/poweroff`. The image
ships `pertisk-power` (reboot(2) directly) as those names — BusyBox `poweroff`
without `-f` waits on PID 1 and would hang under pertiskd.

Kernel `CONFIG_VIRTIO_CONSOLE=y` (linux-virt builtin) — no extra module.

## Rebuild

```bash
make cloud ARCH=amd64
# re-upload / recreate VMs (existing qcow2 keeps old initramfs)
```
