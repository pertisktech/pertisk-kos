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

## Runtime versions (default pins)

| Component | Default | Override |
|-----------|---------|----------|
| Kubernetes kubelet | `v1.32.5` | `K8S_VER` |
| containerd | `2.0.5` | `CONTAINERD_VER` |
| glibc (loader + libc for containerd/kubelet) | Debian bookworm via `fetch-runtime.sh` | — |
| runc | `v1.2.6` | `RUNC_VER` |
| CNI plugins | `v1.6.2` | `CNI_VER` |
| Kernel (QEMU virt) | Alpine `linux-virt` | via `image/fetch-kernel.sh` |
| Bootloader | systemd-boot (Debian) | via `image/fetch-bootloader.sh` |

### Kubernetes control-plane pairing

| Worker kubelet | Expected API server | Notes |
|----------------|---------------------|-------|
| v1.32.x (default) | v1.32.x (±1 skew OK) | Follow [version skew policy](https://kubernetes.io/releases/version-skew-policy/) |
| Custom `K8S_VER` | Matching minor | Rebuild runtime overlay + initramfs |

**In-OS control plane (Phase A):** `machine.type: controlplane` + `pertiskctl bootstrap` writes kubeadm-shaped static pods (etcd + apiserver + controller-manager + scheduler). Images default to `registry.k8s.io/*:v1.32.5` and `registry.k8s.io/etcd:3.5.16-0` — guests must pull from a registry (or mirror). Single-CP first; stacked HA (3 CP) is planned next.

Workers still join with `cluster.endpoint` / `token` / `ca` (`pertiskctl gen config` + `join-config`).

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
