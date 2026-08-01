# Pertisk KOS

Immutable, API-only Kubernetes OS (Talos-shaped), node control plane in **Rust**.

See [DESIGN.md](./DESIGN.md) for architecture and phases.

## Status

**P5 (partial)** — hardening CI gate, compatibility matrix, metrics auth, CNI modes, cloud images, Make VERSION/ARCH.

## Build (Make)

```bash
make help
make build                              # initramfs amd64, Cargo.toml version
make build VERSION=0.2.0 ARCH=arm64     # custom version + arch
make build VERSION=0.2.0 ARCH=amd64 EMBED_BOOT=1 EMBED_RUNTIME=1
make build-all VERSION=0.2.0            # amd64 + arm64
make build-host VERSION=0.2.0           # host cargo release bins → out/bin/
make cloud VERSION=0.2.0 ARCH=amd64     # golden disk image
```

Artifacts: `out/initramfs-<arch>.cpio.gz` and `out/initramfs-<arch>-v<version>.cpio.gz`.

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
  --api-listen 127.0.0.1:50000 \
  --metrics-listen 127.0.0.1:50001 \
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

## Runtime + CNI (join a cluster)

```bash
./image/fetch-runtime.sh
PERTISK_EMBED_RUNTIME=1 ./image/build-initramfs.sh
# machine config: examples/worker-join.yaml (cluster.endpoint/token/podCidr)
```

Kubelet gets bridge CNI at `/etc/cni/net.d/10-pertisk.conflist` (`cni0` + host-local + portmap). Set a unique `cluster.podCidr` per node (e.g. `10.244.1.0/24`).

For Flannel / Cilium, set `cluster.cni: none` and apply a cluster CNI DaemonSet:

```bash
# On control plane:
kubectl apply -f examples/cni/kube-flannel.yaml
# Nodes: examples/worker-join-flannel.yaml
```

See `examples/cni/cilium.md` for Helm install.

## Observability

```bash
# Prometheus text metrics (default :50001)
curl -s http://127.0.0.1:50001/metrics

# Optional bearer (also: STATE secrets/metrics.token)
# --metrics-token "$TOKEN"   or   PERTISK_METRICS_TOKEN=...
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:50001/metrics

# Tail logs via management API (gRPC :50000)
cargo run -p pertiskctl -- -e 127.0.0.1:50000 logs dmesg -n 50
cargo run -p pertiskctl -- -e 127.0.0.1:50000 logs pertiskd
```

## Compatibility

See [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md) for Kubernetes / containerd / CNI / arch matrix.

## CI / SBOM

```bash
./scripts/generate-sbom.sh   # → out/sbom/
```

GitHub Actions runs fmt, clippy, tests, SBOM, and amd64 initramfs build.

## Hardening

See [docs/HARDENING.md](./docs/HARDENING.md) for the CIS-ish worker checklist (kubelet 4.2.x, sysctls, secrets modes, operator steps).

```bash
./scripts/check-hardening.sh   # or: make check-hardening
```

## Cloud / golden disk image

```bash
./image/fetch-kernel.sh && ./image/fetch-bootloader.sh
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
./image/build-cloud-image.sh
# → out/pertisk-cloud-amd64.raw + .qcow2
PERTISK_DISK=out/pertisk-cloud-amd64.raw ./image/run-qemu-uefi.sh
```

See [image/cloud/README.md](./image/cloud/README.md) for AWS / GCP / Azure upload outlines.

## Next

P5 remainder: Secure Boot UKI (stretch); production image without BusyBox.
