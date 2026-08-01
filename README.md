# Pertisk KOS

Immutable, API-only Kubernetes OS (Talos-shaped), node control plane in **Rust**.

See [DESIGN.md](./DESIGN.md) for architecture and phases.

## Status

**P5 (partial)** — metal EFI first-boot install (systemd-boot + slot A), multi-arch initramfs, signed A/B upgrades + mTLS.

## Quick start (mTLS + upgrade)

```bash
./scripts/gen-mtls-certs.sh
mkdir -p /tmp/pertisk-state/secrets
cargo run -p pertisk-update --bin pertisk-sign -- keygen \
  --secret /tmp/pertisk-state/secrets/os-trust.sk \
  --public /tmp/pertisk-state/secrets/os-trust.pk

mkdir -p /tmp/bundle && echo k >/tmp/bundle/kernel && echo i >/tmp/bundle/initramfs
cargo run -p pertisk-update --bin pertisk-sign -- sign \
  --bundle /tmp/bundle --version 0.2.0 \
  --secret /tmp/pertisk-state/secrets/os-trust.sk

cp examples/worker.yaml /tmp/pertisk-state/config.yaml
cargo run -p pertiskd -- --state-dir /tmp/pertisk-state --force-init --skip-runtime \
  --api-listen 127.0.0.1:50001 \
  --tls-ca out/mtls/ca.crt --tls-cert out/mtls/server.crt --tls-key out/mtls/server.key \
  --trust-key /tmp/pertisk-state/secrets/os-trust.pk
```

## Images + UEFI install smoke

```bash
# Installer image with embedded kernel + systemd-boot (+ self as initramfs)
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh

./image/create-disk.sh
./image/run-qemu-disk.sh          # first boot: GPT install + ESP bootstrap
./image/run-qemu-uefi.sh          # second boot: OVMF from disk only
```

`examples/worker-install.yaml` / rootfs config set `machine.install.disk: /dev/vda`.

## Next

P5 remainder: fuller CNI, SBOM/CI, observability, cloud images.
