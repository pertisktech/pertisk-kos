# Pertisk KOS — Compatibility matrix

Pinned defaults come from [`image/fetch-runtime.sh`](../image/fetch-runtime.sh) and the image/build scripts. Override with env vars when building a custom image.

## Supported platforms

| Platform | Arch | Status | Notes |
|----------|------|--------|-------|
| QEMU (UEFI) | amd64 | supported | `./image/run-qemu-uefi.sh` + OVMF |
| QEMU (UEFI) | arm64 | supported | auto-selects `qemu-system-aarch64` + AAVMF |
| Bare metal EFI | amd64 / arm64 | supported | `PERTISK_EMBED_BOOT=1` first-boot install |
| Proxmox VE | amd64 | documented | [PROXMOX.md](./PROXMOX.md) + `scripts/proxmox-upload-vm.sh` |
| AWS (raw/qcow2 upload) | amd64 / arm64 | outlined | See [`image/cloud/README.md`](../image/cloud/README.md) |
| GCP / Azure | amd64 / arm64 | outlined | Same cloud image pipeline |

Host build (dev): macOS / Linux with Rust + Docker (initramfs cross via Zig).

## Runtime versions

Defaults resolve to **latest stable** at fetch time (`dl.k8s.io` + GitHub releases).
Pins are written to `out/runtime/versions.txt`.

| Component | Default | Override |
|-----------|---------|----------|
| Kubernetes kubelet | `latest` → `https://dl.k8s.io/release/stable.txt` | `K8S_VER=v1.36.3` |
| containerd | `latest` GitHub release | `CONTAINERD_VER=2.0.5` (no `v`) |
| runc | `latest` GitHub release | `RUNC_VER=v1.2.6` |
| CNI plugins | `latest` GitHub release | `CNI_VER=v1.6.2` |
| glibc (loader + libc) | Debian bookworm via `fetch-runtime.sh` | — |
| Kernel (QEMU virt) | Alpine `linux-virt` | via `image/fetch-kernel.sh` |
| Bootloader | systemd-boot (Debian) | via `image/fetch-bootloader.sh` |

```bash
make fetch-runtime ARCH=amd64                    # all latest
K8S_VER=v1.36.3 make fetch-runtime ARCH=amd64    # pin kubelet only
PERTISK_RUNTIME_LATEST=1 make fetch-runtime      # force latest even if pins set in env
```

### Kubernetes control-plane pairing

| Worker kubelet | Expected API server | Notes |
|----------------|---------------------|-------|
| same minor as `cluster.kubernetesVersion` | match (±1 skew OK) | Follow [version skew policy](https://kubernetes.io/releases/version-skew-policy/) |
| Custom `K8S_VER` | Matching minor | Rebuild runtime overlay + initramfs |

**In-OS control plane:** `pertiskctl gen config … -k v1.36.3` (default) sets static-pod image tags. Guests must pull `registry.k8s.io/*` (or a mirror). Keep **kubelet** (`fetch-runtime`) and **static-pod tags** on the same minor.

## CNI choices

| Mode | Config | Status | When to use |
|------|--------|--------|-------------|
| Bridge + host-local + portmap | `cluster.cni: bridge` + `podCidr` | default | Single-node / lab; unique `/24` per node |
| Flannel VXLAN | `cluster.cni: none` + `examples/cni/kube-flannel.yaml` | example | Classic overlay; CP must allocate Node PodCIDR |
| Cilium | `cluster.cni: none` + Helm ([`examples/cni/cilium.md`](../examples/cni/cilium.md)) | example | Policy / Hubble; do not combine with Flannel |

Built-in bridge and a cluster CNI DaemonSet must not both own `/etc/cni/net.d`.

## Management / observability

| Surface | Port | Auth | Notes |
|---------|------|------|-------|
| gRPC Machine API | `:50000` | mTLS (`PERTISK_TLS_*`) | Required for production |
| Prometheus metrics | `:50001` | optional bearer (`PERTISK_METRICS_TOKEN`) | Prefer loopback or token on untrusted nets |
| Logs RPC | via gRPC | mTLS | `pertiskctl logs` |

## Image / build matrix

| Artifact | Command |
|----------|---------|
| Initramfs amd64 (production) | `make build ARCH=amd64` |
| Initramfs arm64 | `make build ARCH=arm64` |
| Debug (BusyBox ash) | `make build PROFILE=debug` |
| Both arches | `make build-all` |
| Versioned release | `make build VERSION=0.2.0 ARCH=amd64` |
| Cloud raw + qcow2 | `make cloud VERSION=… ARCH=…` |

## Explicitly out of scope (v0.1)

- Talos API wire compatibility
- Stacked etcd HA / multi-CP join (Phase B)
- Omni-like web fleet manager (Phase D)
- Secure Boot / UKI enrollment (tracked as hardening gap)
- Windows / non-Linux hosts as nodes
