# Pertisk KOS layout

Immutable initramfs OS. A/B swaps **kernel + initramfs** only. STATE and EPHEMERAL survive upgrades.

- PID 1 = `pertiskd`
- No SSH
- musl / Alpine `linux-virt`

> Guest kernel tracks Alpine `linux-virt` from the image fetch pipeline (now Alpine 3.22). Verify current pin in `out/modules-*/version` after `image/fetch-kernel.sh`. See [PACKAGE.md](./PACKAGE.md).

| EFI | BOOT A / B | STATE | EPHEMERAL |
|-----|------------|-------|-----------|
| 512 MiB (vfat) | 768 MiB × 2 | 1 GiB | rest of disk → `/var` |

## GPT disk

Default plan from `crates/pertisk-disk`. EPHEMERAL minimum ~256 MiB (grows to remaining disk). Fixed partitions ≈ 3.1 GiB + remainder.

```text
| EFI 512 | BOOT_A 768 | BOOT_B 768 | META 32 | STATE 1GiB | EPHEMERAL … |
```

| Partition | Size | FS | Survives A/B | Contents |
|-----------|------|----|--------------|----------|
| 1 EFI | 512 MiB | vfat | yes (entries flip) | systemd-boot + slot copies of kernel/initramfs |
| 2 BOOT_A | 768 MiB | ext4 | inactive slot kept | kernel + initramfs (slot A) |
| 3 BOOT_B | 768 MiB | ext4 | inactive slot kept | kernel + initramfs (slot B) |
| 4 META | 32 MiB | ext4 | yes | reserved; boot-meta lives on STATE today |
| 5 STATE | 1 GiB | ext4 | yes | `config.yaml`, secrets, `boot-meta.json`, etcd |
| 6 EPHEMERAL | remainder | ext4 | yes | `/var` — containerd, images, logs |

## Boot stack

| Layer | What runs |
|-------|-----------|
| Firmware | UEFI (OVMF / AAVMF / hypervisor EFI) |
| Bootloader | systemd-boot on ESP · `loader.conf` picks A or B |
| Kernel | Alpine `linux-virt` · `rdinit=/init` |
| Initramfs | `pertiskd` as PID 1 + musl tools + runtime overlay |
| Runtime | containerd + runc + kubelet + CNI modules |
| Control plane | static pods: apiserver, etcd, scheduler, kube-vip |

### A/B upgrade

Bundle is signed Ed25519 (`manifest.sig` + `os-trust.pk` on STATE). Inactive slot is written, next boot flips, then `mark-boot-good`.

| Step | Where |
|------|-------|
| Upload catalog / cluster OS packages | pertisk-mgmt |
| Stage kernel + initramfs to inactive slot | guest via hostPID pod |
| Reboot into new slot | `pertiskctl upgrade --reboot` |
| Mark boot good (or auto-rollback) | Machine API `:50000` |

- **Swapped:** kernel, initramfs, pertiskd, containerd, kubelet
- **Kept:** STATE (etcd, config, secrets), EPHEMERAL (`/var`)

## Guest process tree

```text
UEFI
 └─ systemd-boot  (ESP loader.conf → slot A or B)
     └─ linux-virt + initramfs
         └─ pertiskd  PID 1
             ├─ serial dashboard TUI
             ├─ containerd   : runtime
             ├─ kubelet      : node agent
             ├─ qemu-ga      : hypervisor shutdown / IP
             └─ Machine API  :50000   metrics :50001
                 └─ static pods (control plane)
                     kube-apiserver · etcd · scheduler · controller · kube-vip
```

## Package versions (Aug 2026)

| Layer | Pertisk pin | Current upstream | Ship how |
|-------|-------------|------------------|----------|
| Kernel | linux-virt 6.12.x (Alpine 3.22) | kernel.org LTS track | OS A/B bundle |
| Alpine tools | alpine:3.22 | latest stable Alpine | OS A/B bundle |
| Kubernetes | v1.36.3 | v1.36.3 stable.txt | cluster Upgrade tab |
| containerd | fetch-time latest | GitHub releases | OS A/B (in initramfs) |
| etcd | 3.5.16-0 | 3.6.x line | static pod image |
| kube-vip | v0.8.9 | check GitHub releases | static pod (VIP clusters) |

Sources: [PACKAGE.md](./PACKAGE.md), `image/fetch-kernel.sh`, `pertisk-bootstrap` defaults.

## Related

- [MIGRATE.md](./MIGRATE.md) — move workloads / join external CP / etcd backup-restore
- [MGMT.md](./MGMT.md) — OS packages, adopt, join tokens
