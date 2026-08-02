# Pertisk KOS — Design Draft

**Goal:** Build an immutable, API-only Kubernetes OS (like [Talos Linux](https://www.talos.dev/)), implemented in **Rust**.

**Positioning:** Talos uses Go (`machined`). Pertisk KOS is the same *shape* of product — minimal kernel, custom init, no SSH, gRPC management, containerd + kubelet — but the control plane on the node is Rust.

---

## 1. Summary (what we are building)

| Layer | Talos | Pertisk KOS |
|-------|-------|-------------|
| Kernel | Custom minimal Linux | Same approach (~5–10MB compressed) |
| Init (PID 1) | `machined` (Go) | `pertiskd` (Rust) |
| Management | gRPC + `talosctl` | gRPC/REST + `pertiskctl` |
| Runtime | containerd | containerd (vendored/integrated) |
| Root FS | SquashFS, immutable | SquashFS or EROFS |
| Updates | A/B slots, atomic | A/B slots + signed images |
| Shell/SSH | None by default | None by default |

**One sentence:** Pertisk KOS boots a locked-down Linux image whose only job is to run Kubernetes nodes, managed only through a typed API.

---

## 2. Component map (what you code vs integrate)

| Component | You code (Rust) | Integrate / compile |
|-----------|-----------------|---------------------|
| Kernel | Config + build pipeline | Linux kernel sources |
| Init system | `pertiskd` (PID 1, supervision) | — |
| API layer | `pertisk-api` (tonic gRPC + optional REST gateway) | protobuf IDL |
| Container runtime | Thin supervisor + CRI wiring | containerd |
| Kubelet manager | Spawn/monitor/restart kubelet | upstream kubelet binary |
| Disk management | Partition, LUKS, mount, image apply | cryptsetup, sfdisk/parted, squashfs-tools |
| Network stack | Static/DHCP, routes, DNS, CNI drop-in | CNI plugins (e.g. Cilium/Flannel later) |
| Update mechanism | A/B apply, verify, reboot, rollback | bootloader (systemd-boot or GRUB) |

---

## 3. Target architecture

```
┌─────────────────────────────────────────────────────────┐
│                     pertiskctl / API clients            │
└───────────────────────────┬─────────────────────────────┘
                            │ mTLS + gRPC
┌───────────────────────────▼─────────────────────────────┐
│  pertiskd (PID 1)                                       │
│  ├── api          — node lifecycle, config, logs        │
│  ├── disk         — GPT, A/B, encryption, mounts        │
│  ├── net          — links, addr, DNS, CNI prep          │
│  ├── runtime      — start/stop containerd               │
│  ├── kubelet      — spawn/health/restart kubelet        │
│  └── update       — pull signed image → inactive slot   │
└───────┬─────────────┬──────────────┬────────────────────┘
        │             │              │
   containerd      kubelet      / (EROFS/SquashFS RO)
        │             │
        └────── pods / CRI ──────┘
```

**Boot path (simplified):**

1. Firmware → bootloader (slot A or B)
2. Kernel + initramfs
3. `pertiskd` mounts immutable root, prepares `/var` (writable), applies machine config
4. Starts containerd → kubelet → joins/forms cluster
5. Serves management API (no SSH)

---

## 4. Rust crate layout (draft monorepo)

```
pertisk-kos/
├── Cargo.toml                 # workspace
├── crates/
│   ├── pertiskd/              # PID 1 + service orchestration
│   ├── pertisk-api/           # gRPC server (tonic)
│   ├── pertisk-proto/         # prost-build from .proto
│   ├── pertisk-disk/          # GPT, LUKS, mounts, A/B
│   ├── pertisk-net/           # netlink, DNS, host net
│   ├── pertisk-runtime/       # containerd lifecycle
│   ├── pertisk-kubelet/       # kubelet process manager
│   ├── pertisk-update/        # image verify + slot switch
│   ├── pertisk-config/        # machine config schema (serde)
│   └── pertiskctl/            # CLI client
├── proto/                     # .proto definitions
├── kernel/                    # kernel .config fragments
├── image/                     # OS image build (mkosi / custom)
└── docs/
```

**Key crates / crates.io deps (indicative):**

- `tokio`, `tonic`, `prost` — async + gRPC
- `serde`, `schemars` — machine config
- `nix`, `rustix` — syscalls, mounts, process
- `rtnetlink`, `netlink-packet-*` — networking
- `gpt`, `libcryptsetup-rs` (or FFI) — disk
- `x509-certificate` / `rustls` — mTLS
- `sha2`, `ed25519-dalek` — image signatures

---

## 5. Phased implementation

### Phase 0 — Foundations (weeks 1–3)

**Outcome:** Boots to a Rust PID 1 on QEMU with a read-only root and a writable `/var`.

| Work item | Detail |
|-----------|--------|
| Kernel | Minimal `x86_64` (and later `aarch64`) defconfig: virtio, ext4/erofs/squashfs, overlay, cgroup v2, nftables |
| Initramfs | Mount root, pivot, exec `pertiskd` |
| `pertiskd` skeleton | Become PID 1, reap zombies, mount proc/sys/dev, signal handling |
| Image pipeline | Build SquashFS/EROFS root + kernel + initramfs → QEMU script |
| Machine config v0 | YAML/JSON: hostname, time, basic files |

**Exit criteria:** `qemu-system-x86_64 -kernel …` lands in `pertiskd`; `dmesg`-equivalent via early log; no shell needed for smoke test (serial log is enough).

---

### Phase 1 — Disk + Network (weeks 4–6)

**Outcome:** First-boot can partition disk, set up A/B + STATE/EPHEMERAL, and bring up basic networking.

| Work item | Detail |
|-----------|--------|
| Disk layout | GPT: EFI, BOOT_A, BOOT_B, META, STATE, EPHEMERAL (Talos-inspired) |
| Encryption | Optional LUKS on STATE/EPHEMERAL |
| Net | DHCP or static via rtnetlink; `/etc/resolv.conf` managed |
| Time | chrony or `systemd-timesyncd`-less NTP client (or embed simple SNTP) |

**Exit criteria:** Fresh disk → `pertiskd` installs layout; after reboot, config persists on STATE; eth0 has address.

---

### Phase 2 — Runtime + Kubelet (weeks 7–10)

**Outcome:** Node can run pods (single-node or join existing control plane).

| Work item | Detail |
|-----------|--------|
| containerd | Package binary + config; `pertisk-runtime` starts and watches it |
| kubelet | Drop kubelet binary; generate kubelet config + kubeconfig from machine config |
| CNI prep | Install loopback + chosen CNI; bridge host net namespace expectations |
| Health | Restart policies, crash backoff, status exported to API |

**Exit criteria:** Apply config → containerd healthy → kubelet registers → `pause` pod runs.

---

### Phase 3 — Management API (weeks 8–12, overlaps Phase 2)

**Outcome:** Node is operable without SSH, like Talos.

| Work item | Detail |
|-----------|--------|
| Proto API | `ApplyConfiguration`, `Reboot`, `Shutdown`, `Upgrade`, `Logs`, `Dmesg`, `ServiceList`, `Containers`, `Kubeconfig` (subset first) |
| Auth | Bootstrap certs + mTLS; later SPIFFE/OIDC optional |
| `pertiskctl` | Apply config, get logs, reboot, upgrade |
| REST gateway | Optional `tonic` + HTTP for simpler clients |

**Exit criteria:** Full smoke: create VM → apply config via `pertiskctl` → cluster node Ready → reboot via API.

---

### Phase 4 — A/B updates + immutability hardening (weeks 12–16)

**Outcome:** Atomic OS upgrades with rollback.

| Work item | Detail |
|-----------|--------|
| Image format | Signed OS bundle (kernel + initramfs + rootfs + manifest) |
| Apply | Write inactive slot; verify signature + hash; set bootloader next |
| Rollback | Boot failure counter / “mark good” after API health |
| Secure boot | Optional UKI + enrolled keys (stretch) |

**Exit criteria:** Upgrade A→B; kill boot on B → auto fallback to A.

---

### Phase 5 — Productize (ongoing)

- Multi-arch initramfs (`amd64` / `arm64` via `image/build-all.sh`)
- systemd-boot A/B slot switching when ESP is present
- Metal EFI first-boot install (`PERTISK_EMBED_BOOT=1`, `run-qemu-uefi.sh`)
- Bridge CNI (`bridge` / `host-local` / `portmap`) + `cluster.podCidr`
- Cluster CNI mode `none` + Flannel/Cilium examples (`examples/cni/`)
- CI (fmt/clippy/test + initramfs) and CycloneDX SBOM (`scripts/generate-sbom.sh`)
- Observability: `Logs` RPC + Prometheus `/metrics` (`:50001`)
- Cloud disk images (`image/build-cloud-image.sh` → raw/qcow2; AWS/GCP/Azure notes)
- Compatibility matrix: Kubernetes versions, CNI choices (bridge, Flannel, Cilium) — [docs/COMPATIBILITY.md](./docs/COMPATIBILITY.md)
- CIS-ish hardening checklist ([docs/HARDENING.md](./docs/HARDENING.md)): kubelet 4.2.x, sysctls, STATE secrets modes, metrics bearer token, CI gate (`scripts/check-hardening.sh`)
- Image profiles: `production` (no `/bin/busybox`) vs `debug` (BusyBox ash) via `PERTISK_IMAGE_PROFILE` / `make PROFILE=`
- UKI build + ESP install (`image/build-uki.sh`, optional SB signing) — [docs/SECURE_BOOT.md](./docs/SECURE_BOOT.md)

---

## 6. Machine config (draft schema)

```yaml
version: v1alpha1
machine:
  type: worker          # controlplane | worker
  network:
    hostname: node-1
    interfaces:
      - interface: eth0
        dhcp: true
  install:
    disk: /dev/sda
    wipe: true
cluster:
  endpoint: https://192.168.1.10:6443
  token: <bootstrap-or-join>
  ca: |
    -----BEGIN CERTIFICATE-----
    ...
```

Stored on STATE partition; applied transactionally; API `ApplyConfiguration` validates then stages.

---

## 7. API surface (MVP vs later)

**MVP**

- `ApplyConfiguration` / `ValidateConfiguration`
- `Reboot` / `Shutdown`
- `Version` / `Health`
- `Logs` (pertiskd, containerd, kubelet, dmesg)
- `Upgrade`
- Metrics HTTP `/metrics` (Prometheus text)
**Control plane (Phase A — in progress)**

- `Bootstrap` / `Kubeconfig` RPCs
- `pertiskctl gen config` / `bootstrap` / `kubeconfig` / `join-config`
- Static-pod etcd + apiserver + controller-manager + scheduler (`pertisk-bootstrap`)

**Later (Talos / Omni parity)**

- Stacked etcd HA (3 CP)
- etcd snapshot
- Proxmox provider automation + Omni-like web
- container/CRI introspection
- net / disk inspect
- reset / wipe
- dashboard events stream

---

## 8. Security model (non-negotiable)

1. **No SSH, no shell** in production images (debug image optional, signed separately).
2. **Immutable root** — only STATE/EPHEMERAL writable.
3. **mTLS** for all management traffic.
4. **Signed OS images**; reject unsigned upgrades.
5. **Least kernel surface** — drop unused drivers/modules.
6. **Root of trust** — measured boot path where feasible (Phase 5).

---

## 9. How this links to Talos (compatibility strategy)

| Strategy | Choice for Pertisk |
|----------|-------------------|
| Wire-compatible with Talos API | **No** (different protos; avoid forever-chasing Talos) |
| Conceptual / UX parity | **Yes** — same operator mental model as `talosctl` |
| Kubernetes compatibility | **Yes** — stock kubelet + containerd |
| Image/format compatible | **No** — own image + config schema |
| Learn from | Talos disk layout names, immutability, API-only ops |

Pertisk is a *sibling* of Talos in design, not a fork or drop-in replacement.

---

## 10. Suggested first milestones (concrete)

1. **M0:** Workspace + `pertiskd` PID 1 in QEMU (serial hello + zombie reaping).
2. **M1:** Immutable root + STATE mount + config load.
3. **M2:** DHCP + containerd start.
4. **M3:** kubelet → Ready node.
5. **M4:** gRPC `ApplyConfiguration` + `pertiskctl apply`.
6. **M5:** A/B upgrade with rollback.

---

## 11. Risks & decisions to lock early

| Decision | Options | Recommendation |
|----------|---------|----------------|
| Root FS | SquashFS vs EROFS | EROFS if kernel ≥ 5.15 target; else SquashFS |
| Bootloader | systemd-boot vs GRUB vs UKI | systemd-boot for metal; UKI later |
| Init style | Monolithic `pertiskd` vs many supervisors | Single binary, internal modules (like machined) |
| Config lang | YAML vs CUE | YAML + JSON Schema for MVP |
| Control plane | kubeadm vs custom | Use kubelet + external CP first; self-hosted CP later |
| containerd | System package vs pinned binary in image | Pin version inside OS image |

---

## 12. What “done” looks like for v0.1

- One command builds a bootable image.
- QEMU and one bare-metal path documented.
- Worker joins an existing Kubernetes cluster via `pertiskctl`.
- Upgrade and reboot only through the API.
- No SSH in the default image.
