# Pertisk KOS

Immutable, API-only Kubernetes node OS, plus an optional management plane for provisioning HA clusters.

- **Node OS** — Rust `pertiskd` as PID 1, gRPC management (`pertiskctl`), containerd + kubelet; no SSH in production images
- **Serial dashboard** — Talos-style fullscreen status TUI on Proxmox / ESXi Serial (xterm.js)
- **Management plane** — `pertisk-mgmt` (API + React UI) creates and operates clusters on **Proxmox** and standalone **ESXi**

Architecture and phases: [DESIGN.md](./DESIGN.md). Secure Boot / TPM lab: [docs/SECURE_BOOT.md](./docs/SECURE_BOOT.md). Kernel cmdline (dashboard knobs): [docs/KERNEL.md](./docs/KERNEL.md).

![Pertisk KOS management dashboard](docs/resources/1786259533199.jpg)

## Status

**P5 (productize) — done for lab / HA.**

| Area | Progress |
|------|----------|
| Node OS + A/B + hardening CI | done |
| Serial console dashboard (dual-stack, themes) | done |
| HA bootstrap (stacked etcd + kube-vip) | done |
| CNI lab (Cilium / Calico / Flannel) | done |
| Mgmt UI (Proxmox + ESXi) | done |
| Metrics mTLS (`:50001` with API PEMs) | done |
| UKI / OVMF enroll (`make enroll-ovmf`) | done |
| TPM PCR Attest (sysfs / `pertiskctl attest`) | done (lab) |
| BusyBox-free DHCPv4 (builtin only) | done — T1 renew / T2 rebind |
| util-linux mount/umount + iproute2 ip | done |
| CRI introspection (`pertiskctl containers`) | done (lab) — kind / pod / ns labels |
| Net / disk inspect (`pertiskctl interfaces` / `disks`) | done (lab) |
| TPM2 Quote (`pertiskctl quote --verify`) | done (lab) |
| EK cert + manufacturer CA chain | done (lab) — `PERTISK_TPM_EK_CAS` / `--ek-cas` |
| Mgmt Quote trust store (AK enroll / verify) | done (lab) |
| etcd snapshot / restore (`pertiskctl etcd …`) | done (lab) |

---

## Features

### Platform / node OS

- Immutable node OS: same cloud image for `controlplane` and `worker` (role from machine config)
- `pertiskd` PID 1: GPT / STATE / EPHEMERAL disks, DHCP or static net, containerd, kubelet, signed A/B updates, serial console dashboard
- Multi-arch **amd64** / **arm64** (initramfs + cloud qcow2/raw)
- A/B OS updates with Ed25519-signed bundles (`pertisk-update` / `pertisk-sign`)
- In-process DHCPv4 only (no BusyBox `udhcpc`); T1 renew + T2 rebind maintainer
- util-linux `mount`/`umount` + iproute2 `ip` (no BusyBox applets)
- UKI / Secure Boot lab path (`make uki`, `make enroll-ovmf`, `PERTISK_TPM=1`) — see [docs/SECURE_BOOT.md](./docs/SECURE_BOOT.md)
- Guest extensions: **nfs-client**, **qemu-guest-agent**
- Machine config `v1alpha1`: network, install disk, cluster endpoint/token/CA, kubelet `maxPods`, `machine.dashboard`
- Observability: gRPC mTLS **:50000**, Prometheus **:50001** (mTLS when TLS PEMs set; optional bearer), `pertiskctl logs` / `attest` / `quote` / `etcd` / `containers` / `interfaces` / `disks`
- Hardening checklist + `make check-hardening` — [docs/HARDENING.md](./docs/HARDENING.md)

### Serial console dashboard

Fullscreen status TUI on the Serial / xterm.js console (Proxmox / ESXi). Enabled by default; disable with `pertisk.dashboard.disabled=1`, `PERTISK_DASHBOARD_DISABLED=1`, or `--no-dashboard`.

| Piece | What you get |
|-------|----------------|
| **Layout** | Header (hostname / READY / CPU·RAM·load / uptime) → summary → scrolling logs → footer |
| **Summary (≥80 cols)** | `PERTISK` \| `KUBERNETES` side-by-side, then full-width **NETWORK** (so dual-stack IPv6 is not clipped) |
| **Summary (&lt;80 cols)** | Compact single `SYSTEM` panel |
| **PERTISK** | Machine type, READY, containerd / kubelet status + PIDs |
| **KUBERNETES** | Version, API endpoint, CNI, dual-stack POD / SVC CIDRs |
| **NETWORK** | Primary iface IPv4 + global IPv6 (GUA + ULA; link-local `fe80::` hidden), one address per line |
| **Logs** | Word-wrapped ring buffer; horizontal rules only (no vertical `\|` borders) |
| **Borders** | Default `line`: continuous ASCII `-` (Serial-safe). Optional Unicode styles when UTF-8 works |
| **Themes** | `catppuccin` (default), `dracula`, `nord`, `gruvbox`, `tokyo-night`, `solarized`, `cyberpunk`, `wild-cherry`, `mono` |
| **Sizing** | Prefer Serial / `dashboard.console` winsize (never VGA `tty0`); pin with `PERTISK_DASHBOARD_COLS` / `_ROWS` |
| **Config** | `machine.dashboard` YAML, kernel cmdline / env — see [docs/KERNEL.md](./docs/KERNEL.md) |

Local preview (same glyphs as deploy):

```bash
cargo run -p pertiskd --bin pertiskd -- --dashboard-preview
```

### Cluster lifecycle

- `pertiskctl gen config` — controlplane + worker YAML (HA multi-CP, dual-stack CIDRs, `maxPods`, `mgmt_url`)
- Bootstrap first CP (PKI + static pods: etcd, apiserver, controller-manager, scheduler)
- Join workers and **additional control planes** (stacked etcd)
- **HA**: `controlplanes > 1` → stacked etcd + **kube-vip** VIP (IPv4 ARP and/or IPv6 ND)
- Post-bootstrap finalize: bootstrap-token Secret, node RBAC, control-plane labels/taints, CoreDNS + metrics-server
- Mgmt UI / lab scripts: create, add nodes, bulk reboot/delete, hardware resize (grow EPHEMERAL), delete cluster
- Rolling Kubernetes upgrade (drain → bump version → Ready → uncordon; CPs then workers)
- Download / show / copy cluster kubeconfig

### Networking

| Mode / CNI | Notes |
|------------|--------|
| IPv4 / IPv6 / **dual-stack** | Pod + service CIDRs; optional VIP6; dashboard shows both families |
| Built-in `cluster.cni: bridge` | Unique `podCidr` — single-node / lab |
| **Cilium** (default in lab-up) | `kubeProxyReplacement`; guest needs shared bpffs |
| Calico / Flannel | Via lab-up or `examples/cni/` with `cni: none` |
| kube-vip | Static pod on CPs (needs guest `af_packet`) |

Optional addons: CoreDNS, metrics-server, kubernetes-reflector, NFS provisioner — see [examples/addons/](./examples/addons/).

### Management UI

Single-port API + UI (`pertisk-mgmt`). Details: [docs/MGMT.md](./docs/MGMT.md).

| Area | Capabilities |
|------|----------------|
| **Auth** | Local, Auth0, or both; roles `admin` \| `operator` \| `viewer` |
| **Dashboard** | Cluster counts, online/offline reachability, CPU / memory / disk gauges |
| **Providers** | Proxmox (API token) and **vSphere ESXi** (SOAP `/sdk`); test probe |
| **Create cluster** | Provider, CP/worker counts, arch, CNI, K8s version, max pods, VIP / dual-stack, VMID + live conflict checks, HW sizes |
| **Cluster detail** | Overview, Nodes, **K8s** workloads, **Shell** (mgmt-host PTY + kubeconfig), Config, Upgrade, Jobs |
| **Node detail** | Inventory, live health, metrics charts, log tail |
| **Machines** | Cross-cluster node inventory (Phase D) |
| **Templates** | Reusable machine-config blueprints (Phase D) |
| **Audit** | Management action log (Phase D) |
| **Adopt / join** | Register existing/bare-metal nodes; join-token snapshots (Phase D2) |
| **Settings** | Session, listen/public URL, JWT TTL, paths, auth mode |

### Providers

| Provider | Status | Docs |
|----------|--------|------|
| Proxmox VE | Supported (API token; optional SSH for arm64 create) | [docs/PROXMOX.md](./docs/PROXMOX.md) |
| VMware ESXi (standalone) | Supported (qcow2→VMDK) — not vCenter | [docs/VSPHERE.md](./docs/VSPHERE.md) |
| QEMU / bare metal EFI | Supported | [image/README.md](./image/README.md) |
| AWS / GCP / Azure | Outlined only (**paused**) | [image/cloud/README.md](./image/cloud/README.md) |

---

## Architecture

```
pertiskctl / mgmt UI ──gRPC mTLS──► pertiskd (PID 1) ──► containerd + kubelet
pertisk-mgmt ──HTTPS──► Proxmox API / ESXi SOAP
             └── runs lab-up jobs; stores kubeconfigs + inventory
```

| Crate | Role |
|-------|------|
| `pertiskd` | PID 1 / node supervisor + Serial dashboard |
| `pertisk-api` / `pertisk-proto` | gRPC management API |
| `pertisk-config` | Machine config schema |
| `pertisk-disk` / `pertisk-net` | Volumes + host networking |
| `pertisk-runtime` / `pertisk-kubelet` | containerd + kubelet |
| `pertisk-update` | A/B update / sign |
| `pertisk-bootstrap` | PKI, static pods, join, gen config, kube-vip, addons |
| `pertiskctl` | Node management CLI |
| `pertisk-mgmt` | Cluster mgmt API + embedded React UI |

---

## Quick starts

### 1. Management UI (local)

```bash
export MGMT_ADMIN_USER=admin
export MGMT_ADMIN_PASSWORD=admin
export MGMT_SECRET_KEY=$(openssl rand -hex 32)

make mgmt
./out/bin/pertisk-mgmt --listen 0.0.0.0:8080 --db ./data/mgmt.db
# open http://127.0.0.1:8080
```

Dev (UI hot reload):

```bash
MGMT_ADMIN_PASSWORD=admin cargo run -p pertisk-mgmt -- --listen 127.0.0.1:8080
cd web/mgmt-ui && npm run dev   # :5173 proxies /api → :8080
```

### 2. Deploy mgmt RPM + cloud images to a lab host

```bash
./scripts/deploy-mgmt-lab.sh --mgmt user@host --version 0.2.3
# Host wrappers (edit VERSION / MGMT / PVE inside):
#   ./deploy-285h.sh | ./deploy-13900hx.sh | ./deploy-h255.sh
#   ARCH=both ./deploy-h255.sh    # amd64 + arm64 images
```

Full RPM + Proxmox SSH notes: [docs/MGMT.md](./docs/MGMT.md#rpm-deploy-linuxamd64).

### 3. CLI cluster (Proxmox lab-up)

```bash
make cloud ARCH=amd64
make pertiskctl
export PROXMOX_SSH=root@<pve>

# Example HA: 3 CP + 3 workers, Cilium, VIP
./scripts/proxmox-lab-up.sh \
  --controlplanes 3 --workers 3 \
  --vip <free-ip> --cni cilium

# ESXi: ./scripts/vsphere-lab-up.sh …
```

Manual cluster flow (`gen config` → apply → bootstrap → join): [docs/PROXMOX.md](./docs/PROXMOX.md).

### 4. Build node OS images

```bash
make help
make build VERSION=0.2.0 ARCH=amd64 EMBED_BOOT=1 EMBED_RUNTIME=1
make build-all VERSION=0.2.0            # amd64 + arm64
make cloud VERSION=0.2.0 ARCH=amd64     # golden disk → out/pertisk-cloud-*.qcow2
make uki ARCH=amd64                     # Unified Kernel Image
make pertiskctl                         # → out/bin/pertiskctl
make mgmt / make mgmt-rpm               # UI+API binary / RPM
```

Artifacts: `out/initramfs-<arch>.cpio.gz` (or `-debug`), versioned copies, `out/uki/`, `out/rpm/`.

### 5. Preview Serial dashboard locally

```bash
cargo run -p pertiskd --bin pertiskd -- --dashboard-preview
```

---

## Observability

```bash
# Plaintext (lab default when TLS unset)
curl -s http://127.0.0.1:50001/metrics

# mTLS (same PEMs as the management API; enabled whenever --tls-* is set)
curl -s --cacert out/mtls/ca.crt \
  --cert out/mtls/client.crt --key out/mtls/client.key \
  https://127.0.0.1:50001/metrics
# Optional bearer still applies: --metrics-token / PERTISK_METRICS_TOKEN / STATE secrets/metrics.token

./out/bin/pertiskctl -e 127.0.0.1:50000 logs dmesg -n 50
./out/bin/pertiskctl -e 127.0.0.1:50000 logs pertiskd
./out/bin/pertiskctl -e 127.0.0.1:50000 containers # containerd k8s.io via ctr (+ CRI labels)
./out/bin/pertiskctl -e 127.0.0.1:50000 interfaces
./out/bin/pertiskctl -e 127.0.0.1:50000 disks
./out/bin/pertiskctl -e 127.0.0.1:50000 logs container:<id> -n 200  # CRI app logs via /var/log/pods
./out/bin/pertiskctl -e 127.0.0.1:50000 attest      # TPM PCRs when present
./out/bin/pertiskctl -e 127.0.0.1:50000 quote --verify  # TPM2 Quote + local verify
# Optional EK manufacturer chain: --ek-cas /path/to/cas  (or PERTISK_TPM_EK_CAS on the node)
./out/bin/pertiskctl -e 127.0.0.1:50000 etcd snapshot
```

## mTLS + signed upgrade smoke

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

## QEMU / UEFI install smoke

```bash
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
./image/create-disk.sh
./image/run-qemu-disk.sh          # first boot: GPT install + ESP
./image/run-qemu-uefi.sh          # boot from disk (OVMF)
# PERTISK_TPM=1 ./image/run-qemu-uefi.sh   # soft-TPM (swtpm) for PCR Attest lab
```

## Compatibility / CI / SBOM

- Matrix (K8s, containerd, CNI, arch): [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md)
- Hardening: [docs/HARDENING.md](./docs/HARDENING.md) — `make check-hardening`
- SBOM: `./scripts/generate-sbom.sh` → `out/sbom/`
- CI: fmt, clippy, tests, SBOM, amd64 initramfs build

---

## Documentation

| Doc | Topic |
|-----|--------|
| [DESIGN.md](./DESIGN.md) | Architecture, phases, security model |
| [docs/MGMT.md](./docs/MGMT.md) | Management UI/API, auth, create, RPM |
| [docs/KERNEL.md](./docs/KERNEL.md) | Kernel cmdline (dashboard, defaults, deferred) |
| [docs/PROXMOX.md](./docs/PROXMOX.md) | Proxmox token, upload, cluster bootstrap |
| [docs/VSPHERE.md](./docs/VSPHERE.md) | ESXi provider |
| [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md) | Platforms, runtime pins, CNI |
| [docs/HARDENING.md](./docs/HARDENING.md) | CIS-ish worker checklist |
| [docs/SECURE_BOOT.md](./docs/SECURE_BOOT.md) | UKI + OVMF enroll + TPM PCR Attest lab |
| [image/README.md](./image/README.md) | Initramfs / QEMU |
| [image/cloud/README.md](./image/cloud/README.md) | Cloud upload outlines |
| [image/extensions/README.md](./image/extensions/README.md) | nfs-client, qemu-ga |
| [examples/cni/README.md](./examples/cni/README.md) | Cilium / Calico / Flannel |
| [examples/addons/README.md](./examples/addons/README.md) | CoreDNS, metrics-server, reflector, NFS |

## Next

**Phase D** — D0–D2 done (Audit, Machines, Templates, adopt/join tokens). Next: D3 multi-tenant (later). AWS/GCP/Azure providers paused.
