# Pertisk KOS — Compatibility matrix

Pinned defaults come from [`image/fetch-runtime.sh`](../image/fetch-runtime.sh) and the image/build scripts. Override with env vars when building a custom image.

## Supported platforms

| Platform | Arch | Status | Notes |
|----------|------|--------|-------|
| QEMU (UEFI) | amd64 | supported | `./image/run-qemu-uefi.sh` + OVMF |
| QEMU (UEFI) | arm64 | supported | auto-selects `qemu-system-aarch64` + AAVMF |
| Bare metal EFI | amd64 / arm64 | supported | `PERTISK_EMBED_BOOT=1` first-boot install |
| Proxmox VE | amd64 | documented | [PROXMOX.md](./PROXMOX.md) + `scripts/proxmox-upload-vm.sh` |
| VMware ESXi (standalone) | amd64 | documented | [VSPHERE.md](./VSPHERE.md) + `scripts/vsphere-upload-vm.sh` |
| Nutanix AHV (Prism Element) | amd64 | documented | [NUTANIX.md](./NUTANIX.md) + `scripts/nutanix-upload-vm.sh` |
| Pertisk VMs (pertiskd) | amd64 / arm64 | documented | [PERTISK_VMS.md](./PERTISK_VMS.md) + `scripts/pertisk-vms-upload-vm.sh` |
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
| Flannel VXLAN | `cluster.cni: none` + `--cni flannel` / [`kube-flannel.yaml`](../examples/cni/kube-flannel.yaml) | lab-up | Classic overlay + kube-proxy |
| Calico VXLAN | `cluster.cni: none` + `--cni calico` / [`calico.md`](../examples/cni/calico.md) | lab-up | Policy + kube-proxy |
| Cilium | `cluster.cni: none` + `--cni cilium` / [`cilium.md`](../examples/cni/cilium.md) | lab-up | Policy / Hubble; kubeProxyReplacement (no kube-proxy) |

Built-in bridge and a cluster CNI DaemonSet must not both own `/etc/cni/net.d`. Install only **one** of Flannel / Calico / Cilium.

Image needs shared kernel modules + host `iptables-legacy` — see [examples/cni/README.md](../examples/cni/README.md).

## Management / observability

| Surface | Port | Auth | Notes |
|---------|------|------|-------|
| gRPC Machine API | `:50000` | mTLS (`PERTISK_TLS_*`) | Required for production |
| Prometheus metrics | `:50001` | mTLS when `PERTISK_TLS_*` set; optional bearer (`PERTISK_METRICS_TOKEN`) | Health/boot/API + host CPU/RAM/net/disk I/O; plain HTTP if TLS unset; prefer mTLS or loopback on untrusted nets |
| Logs RPC | via gRPC | mTLS | `pertiskctl logs`; optional Loki push (`PERTISK_LOKI_URL` / `machine.observability.lokiUrl`) |

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

- Third-party node OS API wire compatibility
- Secure Boot / UKI enrollment automation (lab path done; see [SECURE_BOOT.md](./SECURE_BOOT.md))
- Windows / non-Linux hosts as nodes
- AWS / GCP / Azure cloud providers (**paused**; outlines only — [image/cloud/README.md](../image/cloud/README.md))

## Phase D — Omni-like web fleet manager (in progress)

Building on `pertisk-mgmt` (single-tenant multi-cluster today):

| Milestone | Work | Status |
|-----------|------|--------|
| D0 | Audit log API/UI + cross-cluster Machines inventory | done |
| D1 | Config templates / machine-config blueprints | done |
| D2 | Bare-metal / machine registration | done |
| D3 | Multi-tenant orgs / SaaS | later |

See [DESIGN.md](../DESIGN.md) §7 and [MGMT.md](./MGMT.md).

## Multi-CP HA (lab)

Stacked etcd + kube-vip ARP is supported via:

```bash
./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.200 --workers 2 --cni cilium
```

`--vip` must be a free L2 address on the guest network. Workers and CNI use the VIP as `cluster.endpoint` / `k8sServiceHost`.