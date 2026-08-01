# Pertisk KOS

Immutable, API-only Kubernetes OS (Talos-shaped), node control plane in **Rust**.

See [DESIGN.md](./DESIGN.md) for architecture and phases.

## Status

**P5 (partial)** — systemd-boot A/B slot switching + multi-arch initramfs builds (amd64/arm64).

Prior: **M5** signed A/B upgrades + rollback; management API mTLS.

## Quick start (mTLS + upgrade)

```bash
# Certs + OS trust key
./scripts/gen-mtls-certs.sh
mkdir -p /tmp/pertisk-state/secrets
cargo run -p pertisk-update --bin pertisk-sign -- keygen \
  --secret /tmp/pertisk-state/secrets/os-trust.sk \
  --public /tmp/pertisk-state/secrets/os-trust.pk

# Sign a bundle
mkdir -p /tmp/bundle && echo k >/tmp/bundle/kernel && echo i >/tmp/bundle/initramfs
cargo run -p pertisk-update --bin pertisk-sign -- sign \
  --bundle /tmp/bundle --version 0.2.0 \
  --secret /tmp/pertisk-state/secrets/os-trust.sk

# Node
cp examples/worker.yaml /tmp/pertisk-state/config.yaml
cargo run -p pertiskd -- --state-dir /tmp/pertisk-state --force-init --skip-runtime \
  --api-listen 127.0.0.1:50001 \
  --tls-ca out/mtls/ca.crt --tls-cert out/mtls/server.crt --tls-key out/mtls/server.key \
  --trust-key /tmp/pertisk-state/secrets/os-trust.pk

# Client
cargo run -p pertiskctl -- -e 127.0.0.1:50001 \
  --ca out/mtls/ca.crt --cert out/mtls/client.crt --key out/mtls/client.key \
  upgrade --bundle /tmp/bundle
cargo run -p pertiskctl -- -e 127.0.0.1:50001 \
  --ca out/mtls/ca.crt --cert out/mtls/client.crt --key out/mtls/client.key \
  upgrade-status
```

Plaintext API still works when TLS flags are omitted (dev only).

## Images

```bash
./image/build-initramfs.sh                          # out/initramfs.cpio.gz (amd64)
PERTISK_PLATFORM=linux/arm64 ./image/build-initramfs.sh
./image/build-all.sh                                # both arches
PERTISK_ARCH=arm64 ./image/fetch-kernel.sh
PERTISK_ARCH=arm64 ./image/fetch-runtime.sh
```

On upgrade, if an ESP is mounted (`/boot/efi`, `/efi`, or `/boot` with EFI/loader), `pertiskd` copies the staged kernel/initramfs and flips systemd-boot `loader.conf` default. Without ESP (dev/QEMU), staging is meta-only.

## Next

P5 remainder: metal EFI install images, full CNI, SBOM/reproducible builds, observability.
