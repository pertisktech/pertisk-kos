# Pertisk KOS

Immutable, API-only Kubernetes OS (Talos-shaped), node control plane in **Rust**.

See [DESIGN.md](./DESIGN.md) for architecture and phases.

## Status

**M1** — STATE volume + config boot path + QEMU initramfs pipeline.

| Crate | Role | Phase |
|-------|------|-------|
| `pertiskd` | Init / supervisor (PID 1) | P0/M1 |
| `pertisk-disk` | GPT labels, STATE mount | M1 |
| `pertisk-config` | Machine config schema | P0 |
| `pertiskctl` | Management CLI | P3 (stub) |
| others | net, runtime, … | stubs |

## Quick start (dev / M1)

```bash
# Unit tests
cargo test -p pertisk-config -p pertisk-disk

# Local STATE smoke (no QEMU)
mkdir -p /tmp/pertisk-state
cp examples/worker.yaml /tmp/pertisk-state/config.yaml
cargo run -p pertiskd -- --state-dir /tmp/pertisk-state --smoke

# QEMU smoke (Docker + qemu)
./image/build-initramfs.sh
./image/fetch-kernel.sh
brew install qemu   # once
./image/run-qemu.sh
```

Details: [image/README.md](./image/README.md).

## Example machine config

```yaml
version: v1alpha1
machine:
  type: worker
  network:
    hostname: pertisk-node-1
    interfaces:
      - interface: eth0
        dhcp: true
```

## Next (P1 / M2)

GPT install on virtio disk, DHCP networking, then containerd.
