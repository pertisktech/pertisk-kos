# Deploy Pertisk on Proxmox VE

Pertisk KOS is a **Talos-shaped** Kubernetes OS: the **same cloud image** runs as `controlplane` or `worker` (role comes from machine config). Create/join clusters with `pertiskctl` (see [Talos-shaped cluster](#talos-shaped-cluster-1-cp--workers) below).

Do **not** put Proxmox root passwords in git, chat, or scripts. Use an **API token**.

## 1. Build a cloud disk

On your build machine (Docker required). Guest arch is `amd64` (default) or `arm64`:

```bash
./image/fetch-kernel.sh
./image/fetch-bootloader.sh
./image/fetch-runtime.sh

PERTISK_EMBED_BOOT=1 PERTISK_EMBED_RUNTIME=1 ./image/build-initramfs.sh
# Optional: bake join settings into STATE before imaging
# PERTISK_SEED_CONFIG=examples/worker-join.yaml ./image/build-cloud-image.sh
./image/build-cloud-image.sh
# → out/pertisk-cloud-amd64.qcow2
```

Or: `make cloud ARCH=amd64` / `make cloud ARCH=arm64` (fetches/embeds boot + containerd/kubelet, then builds qcow2).

## 2. Create a Proxmox API token

In the UI: **Datacenter → Permissions → API Tokens → Add**

- User: e.g. `root@pam` (or a dedicated user with VM.Allocate / Datastore.AllocateSpace / SDN.Use)
- Token ID: e.g. `pertisk`
- **Uncheck** “Privilege Separation” only if you intentionally want full user rights; prefer a limited role in production
- Copy the **secret** once (shown only at creation)

Export on the build host (shell profile / direnv — never commit):

```bash
export PROXMOX_URL="https://proxmox.example:8006"
export PROXMOX_TOKEN_ID="root@pam!pertisk"    # user@realm!tokenid
export PROXMOX_TOKEN_SECRET="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
export PROXMOX_NODE="pve"                     # node name
export PROXMOX_STORAGE="local-lvm"            # or local, ceph, etc.
```

For **ZFS** (`local-zfs`), either:

```bash
export PROXMOX_STORAGE=local-zfs
export PROXMOX_UPLOAD_STORAGE=local   # directory storage for the HTTP upload
# or:
export PROXMOX_SSH=root@10.1.1.197    # scp + qm importdisk (most reliable)
```

Self-signed TLS lab:

```bash
export PROXMOX_INSECURE=1
```

## 3. Upload disk + create UEFI VM

```bash
./scripts/proxmox-upload-vm.sh \
  --vmid 9100 \
  --name pertisk-worker-1 \
  --disk out/pertisk-cloud-amd64.qcow2 \
  --memory 4096 \
  --cores 2 \
  --bridge vmbr0

# arm64 guest (PVE arm64 host, or amd64 host with pve-edk2-firmware-aarch64):
ARCH=arm64 ./scripts/proxmox-upload-vm.sh \
  --vmid 9200 --name lab-cp-1 --disk out/pertisk-cloud-arm64.qcow2
```

Defaults: **4096 MB** / **2 vCPUs**. Role overrides: `--cp-memory` / `--cp-cores` / `--worker-memory` / `--worker-cores` and `--cp-disk-gb` / `--worker-disk-gb`.

**Disk sizing:** `--cp-disk-gb` / `--worker-disk-gb` build from a small (~4G) populate, then `qemu-img resize` to role-sized qcow2s (`out/pertisk-cloud-<arch>-Ng.qcow2`). First boot grows GPT **EPHEMERAL** and `mkfs.ext4` at final size (so containerd has enough inodes). `qm resize` alone does **not** rewrite GPT — use sized images. Dashboard **STATE** stays ~1 GiB by layout; usable space for containers is **EPHEMERAL** (`/var`).

The script:

1. Uploads the qcow2 to the datastore as an importable disk
2. Creates a UEFI VM: **amd64** → `arch=x86_64` / `machine=q35` / OVMF; **arm64** → `arch=aarch64` / `machine=virt` / AAVMF
3. Attaches the imported disk and starts the VM (unless `--no-start`)

Arch is taken from `--arch`, `ARCH` / `PERTISK_ARCH`, or the disk filename (`*-arm64*` / `*-amd64*`).

Manual UI alternative: upload `pertisk-cloud-amd64.qcow2` → Create VM (UEFI, q35) → attach disk → start. For arm64: UEFI + machine type **virt** (AAVMF).

### arm64 notes

- Build: `make cloud ARCH=arm64` → `out/pertisk-cloud-arm64.qcow2`
- Lab / create-cluster: `--arch arm64` or `ARCH=arm64` / `PERTISK_ARCH=arm64`
- Mgmt jobs forward `ARCH` / `PERTISK_ARCH` from the host environment into lab-up / add-node (`--arch`)
- **Proxmox API tokens cannot set `arch=`** (`only root can set 'arch' config`). amd64 omits it (default x86_64). For arm64 pick one:
  - **No SSH (like amd64):** one-time root template → `PROXMOX_ARM64_TEMPLATE=8900` → API clone (`scripts/proxmox-ensure-arm64-template.sh` on the PVE node)
  - **SSH:** `PROXMOX_SSH=root@<pve>` so upload can `qm create --arch aarch64` (unset `PROXMOX_NO_SSH` / use deploy `--with-ssh`)
- On **amd64** Proxmox hosts running aarch64 guests, install `pve-edk2-firmware-aarch64` and use **`cpu=max`** (not `cpu=host` — host CPU passthrough is same-arch only and causes guest kernel panic)

### Boot menu

Cloud images set systemd-boot `timeout 0` so there is **no countdown menu**. With `vga=serial0`, that menu’s text is usually garbled on Serial; the VM boots straight into the kernel.

### Console dashboard + finding the guest IP

Proxmox VMs created by `proxmox-upload-vm.sh` enable **QEMU Guest Agent** (`agent=enabled=1`). Cloud images ship `/usr/bin/qemu-ga` plus `/sbin/{poweroff,shutdown,reboot,halt}` → `pertisk-power` (direct `reboot(2)`; BusyBox poweroff would hang waiting on PID 1). `pertiskd` starts qemu-ga at boot (and links `/dev/virtio-ports/org.qemu.guest_agent.0` without udev). That enables Proxmox **Shutdown** / Summary IP. Rebuild the guest image after upgrading — existing VMs keep the old initramfs until replaced.

Guests default to **IPv4-only** at runtime: `pertiskd` disables IPv6 via sysctl before DHCP (no hard `ipv6.disable=1` on the cmdline). Opt into dual-stack with `cluster.networkMode: dual-stack` / `pertiskctl gen config --dual-stack` / lab `--dual-stack` so SLAAC/global IPv6 is allowed. Rebuild the cloud image only when you need a fresh qcow2 for other changes — IPv4-only vs dual-stack is config-driven.

### Resume HA lab (IPv4)

If a previous run stopped after CP join / waiting on the VIP:

```bash
# Reuse existing VMs; skip image build. Adjust VIP / --cp-vmid to match the lab.
./scripts/proxmox-lab-up.sh --skip-build --skip-vms \
  --cp-vmid 210 --controlplanes 3 --vip 10.1.1.210 --workers 3 --cni cilium
```

Fresh HA with sizing (rebuild so disk layout matches):

```bash
./scripts/proxmox-lab-up.sh \
  --cp-memory 4096 --cp-cores 2 \
  --worker-memory 8192 --worker-cores 4 \
  --cp-disk-gb 50 --worker-disk-gb 75 \
  --controlplanes 3 --vip 10.1.1.210 --workers 3 --cni cilium
```

Requires: `./proxmox.sh`, `PROXMOX_SSH`, a free VIP on L2, outbound pulls for registry/Cilium.

After boot, `pertiskd` shows a Serial TUI **as soon as Serial is ready** (before DHCP / STATE / containerd finish). Status line cycles `booting` → `network` → `mounting STATE` → `starting runtime`. Cursor is hidden; ~2s refresh.

Nothing else may write to the console while the dashboard owns it: tracing stays in the ring, and `udhcpc` / module load no longer `eprintln!` onto Serial.

Colors use the **16 base ANSI colors only** — 256-color and truecolor SGR arrive mangled through Serial. Status follows the usual convention: `up`/`ready` green, `failed`/`absent` red, anything else amber; memory and disk meters redden past 70% and 90%. Log lines are word-wrapped (continuations indented two columns) and colored by severity.

#### Size and glyph detection

A serial line carries no `SIGWINCH`, and interactive CSI size probes often leave Proxmox xterm.js blank, so **live probing is off by default**. Size comes from `PERTISK_DASHBOARD_COLS`/`_ROWS` when set, else a safe **80×24** fallback (ioctl accepted only when it looks sane). Set `PERTISK_DASHBOARD_PROBE=1` to re-enable CSI `18 t` / cursor-extent.

The TUI paints with home + `\r\n` per row (banner-style), not per-row CUP — CUP left some Serial sessions blank after clear.

#### Overrides

Priority (highest first):

1. Kernel cmdline env (`PERTISK_DASHBOARD_*` — the kernel forwards unknown `KEY=value` tokens to PID 1)
2. `machine.dashboard` in `config.yaml`
3. Built-in defaults (`catppuccin` / `ascii`; size + UTF-8 from console probe)

**config.yaml:**

```yaml
machine:
  type: controlplane
  dashboard:
    theme: catppuccin  # optional override; omit machine.dashboard for built-ins
    border: ascii      # auto | ascii | light | rounded | heavy | double
    cols: 93           # optional — pin width (else probe, fallback 80)
    rows: 25           # optional — pin height (else probe, fallback 24)
    utf8: true         # optional — force Unicode borders on Serial
    mgmt_url: https://ptkos.apps.thaidevops.co   # shown on node panel
```

Built-in defaults (no YAML needed): `catppuccin` / `ascii`. Size and UTF-8 follow the console probe — only pin `cols`/`rows`/`utf8` when the probe is wrong (wrong size blanks the Serial console).

**Kernel cmdline / env:**

| Variable | Values |
| --- | --- |
| `PERTISK_DASHBOARD_THEME` | `catppuccin` (default), `dracula`, `nord`, `gruvbox`, `wild-cherry`, `tokyo-night`, `solarized`, `cyberpunk`, `mono` |
| `PERTISK_DASHBOARD_BORDER` | `ascii` (default), `rounded`, `auto`, `light`, `heavy`, `double` |
| `PERTISK_DASHBOARD_COLS` / `_ROWS` | optional pin; else probe (fallback `80` / `24`) |
| `PERTISK_DASHBOARD_UTF8` | optional; else follow the UTF-8 probe |
| `MGMT_PUBLIC_URL` | optional public management UI URL (also `PERTISK_MGMT_URL` or `machine.dashboard.mgmt_url`) |

`mono` drops all frame color and keeps only status colors. On Proxmox Serial the UTF-8 probe often fails; `auto` then picks ASCII. Explicit `double` / `heavy` without `utf8: true` use `=` / `#` ASCII stand-ins so the frame still renders. With `utf8: true` you get real box-drawing (`╔═╗`). Check the startup line for `border=double` vs `border=double-ascii`.

#### Making the font smaller (more rows and columns)

Font size lives in the Proxmox web UI, not in the guest — the guest only ever learns the resulting column count. Two ways to change it:

- **Persistent:** click your username (top right) → **My Settings** → the **xterm.js** panel → set **Font-Size** (try 10, or 8 for a very dense console) → **Save**. This sticks for every console you open.
- **Ad hoc:** **Ctrl+-** inside an active console tab.

Size is probed **once at startup**. After changing the font, reopen the Console tab (or reboot the VM) so `pertiskd` re-measures — or pin the size with `PERTISK_DASHBOARD_COLS` / `_ROWS` on the kernel cmdline.

Borders default to ASCII (`+ - |`). For Unicode rules: `PERTISK_DASHBOARD_BORDER=rounded` with a UTF-8-capable console.

#### If the panel still looks too small for the window

Check the startup log line `console TUI WxH (source)` — it tells you what the guest thinks the size is.

- **Says `80x24 (default)` or `80x24 (ioctl)`.** Both terminal queries went unanswered, so nothing was detected. Override: add `PERTISK_DASHBOARD_COLS=140 PERTISK_DASHBOARD_ROWS=40` to the kernel cmdline.
- **Smaller than the window looks.** xterm.js measured the pane before the browser finished laying it out. Reload the console tab and wait one resize check.
Deploy scripts set `serial0=socket` and **`vga=serial0`**, so Proxmox **Console** opens serial/xterm.js. Guest cmdline ends with `console=ttyS0`; `pertiskd` also redirects stdio to `/dev/ttyS0`. Host: `qm terminal <vmid>`.

IPs appear in the **network** panel.

Disable with `--no-dashboard` if you need raw scrolling serial logs only.

Or look up the VM MAC on the Proxmox host / DHCP server:

```bash
# MAC from Hardware → Network Device, e.g. BC:24:11:F2:B6:53
ip neigh | grep -i bc:24:11:f2:b6:53
# or your router's DHCP lease table for that MAC
```

To force a known address, bake static config into the image (`dhcp: false` + `addresses` / `gateway` in the seed YAML) and rebuild.

### If the VM does not boot

**Console shows `Start PXE over IPv6` / `PXE over IPv4`:** UEFI found **no bootable disk** and fell through to network boot. Almost always: missing `scsi0`, **`scsi0` size=1M** while the real 8G import sits under `unusedN`, failed qcow2 import (common on ZFS **without** `PROXMOX_SSH`), or Secure Boot rejecting unsigned systemd-boot.

```bash
# On the PVE host (or via SSH) — inspect (scsi0 must be ~8G, not 1M):
qm config <vmid> | grep -E '^(scsi0|unused|efidisk|boot|bios):'
pvesm list local-zfs | grep vm-<vmid>

# Preferred: set SSH and re-attach the cloud image
export PROXMOX_SSH=root@10.1.1.195   # your PVE
./scripts/proxmox-reattach-disk.sh <vmid> out/pertisk-cloud-amd64.qcow2
./scripts/proxmox-fix-boot.sh <vmid>
```

**`TASK ERROR: connection timed out`** on Console is often a **VNC proxy** failure — check Serial instead, or SSH: `qm terminal <vmid>`.

If `qm config` shows **no `scsi0`** (only `efidisk0` / `boot: order=net0`), the OS disk was lost — re-attach:

```bash
PROXMOX_SSH=root@10.1.1.195 ./scripts/proxmox-reattach-disk.sh 9100 out/pertisk-cloud-amd64.qcow2
```

Other boot tips:

| Issue | Fix |
|-------|-----|
| PXE over IPv4/IPv6 | Attach/import `scsi0`; boot order `scsi0` only; `proxmox-fix-boot.sh` |
| `scsi0` size=1M + unused 8G | Upload script picked wrong unused — `fix-boot.sh` or reattach largest unused |
| Secure Boot / Microsoft keys | `efidisk` with `pre-enrolled-keys=0` (script above recreates it) |
| Wrong boot disk | Options → Boot Order → `scsi0` first (disable Network) |
| No EFI disk | Hardware → Add EFI Disk |
| Still UEFI shell | Confirm GPT has ESP; re-import cloud image |
| ZFS import without SSH | Set `PROXMOX_SSH=root@pve` — API upload to `local-zfs` often leaves no disk |

## 4. Talos-shaped cluster (1 CP or HA)

Same qcow2 for control-plane and workers. Prefer VMIDs **210+** so lab VM 200 is untouched.

### 4a. Create VMs

```bash
make cloud ARCH=amd64   # or ARCH=arm64
# Uses ./proxmox.sh for PROXMOX_* if not already exported (does not run its exec).
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
# → 210 pertisk-cp-1, 211 pertisk-wk-1, 212 pertisk-wk-2

# arm64 guests on Proxmox:
./scripts/proxmox-create-cluster-vms.sh --arch arm64 --cp-vmid 210 --workers 2

# HA (3 CP + 2 workers): VMIDs 210–212 CP, 213–214 workers
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --controlplanes 3 --workers 2 --no-lab-up

# Sizing (passed through to upload):
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2 --memory 8192 --cores 4
# Or different CP vs worker:
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2 \
  --cp-memory 8192 --cp-cores 4 --worker-memory 4096 --worker-cores 2
# Disk GiB per role (lab-up builds separate *-50g / *-75g qcow2; import size = Proxmox size):
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2 \
  --cp-disk out/pertisk-cloud-amd64-50g.qcow2 \
  --worker-disk out/pertisk-cloud-amd64-75g.qcow2 --no-lab-up
```

### 4a-auto. One-shot lab (build → VMs → IPs → cluster → CNI)

```bash
# Needs ./proxmox.sh + ideally PROXMOX_SSH=root@<pve> for MAC→ARP IP lookup.
# Optional: --subnet 10.1.1.0/24 for nmap ping-sweep fallback.
make lab-up ARCH=amd64
# arm64:
make lab-up ARCH=arm64
# or skip rebuild / reuse VMs:
./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cp-vmid 210 --workers 2 --cni cilium
# HA (pick a free L2 IP for kube-vip):
./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.200 --workers 2 --cni cilium
# Opt-in dual-stack (needs SLAAC/ULA on the bridge or static v6; optional --vip6):
./scripts/proxmox-lab-up.sh --controlplanes 3 --vip 10.1.1.210 --vip6 'fd00:1::210' \
  --dual-stack --workers 2 --cni cilium
# CP vs worker sizing (builds role-sized qcow2s; Proxmox Hardware shows 50G / 75G):
./scripts/proxmox-lab-up.sh \
  --cp-memory 4096 --cp-cores 2 \
  --worker-memory 8192 --worker-cores 4 \
  --cp-disk-gb 50 --worker-disk-gb 75 --cni cilium
# or: --cni calico | --cni flannel
# install an example app after CNI:
APPS=examples/apps/nginx.yaml ./scripts/proxmox-lab-up.sh --skip-build --cni cilium
```

**IPv4 vs dual-stack:** default labs stay IPv4-only (`networkMode: ipv4`, Cilium `ipv6.enabled=false`). `--dual-stack` sets `networkMode: dual-stack` with Talos-shaped CIDRs (`10.244.0.0/16` + `2001:db8:10:0::/56` pods, `10.96.0.0/12` + `2001:db8:96:1::/112` services), enables guest IPv6, dual-stack apiserver `--service-cluster-ip-range`, kubelet `--node-ip=v4,v6`, and Cilium `ipam.mode=kubernetes` + `ipv6.enabled=true` (Node.PodCIDR). Lab bridges often have no RA — guests then get a stable node ULA derived from DHCPv4 (`10.1.1.173` → `fd00:a:1:1::ad/64`). When the LAN also offers SLAAC, the GUA wins and the synthetic ULA is dropped so kubectl InternalIP and the serial dashboard eth0 match. Rebuild the cloud image when changing EPHEMERAL/kubelet dual-stack behavior. Flannel/Calico dual-stack is out of scope for lab-up.

CNI choices (pick one): Cilium (default, kubeProxyReplacement), Calico (VXLAN + kube-proxy), Flannel (VXLAN + kube-proxy). See [examples/cni/README.md](../examples/cni/README.md) and [cilium.md](../examples/cni/cilium.md) / [calico.md](../examples/cni/calico.md).

`lab-up` / bootstrap finalize always install **basic addons**: CoreDNS (`kube-dns` **10.96.0.10**) and [metrics-server](../examples/addons/metrics-server.yaml). Optional reflector is installed by lab-up unless `--skip-addons`. With Cilium, CoreDNS stays Pending while nodes carry `node.cilium.io/agent-not-ready` (until agents are Running).

After DNS/addons, lab-up can install [optional reflector](../examples/addons/README.md). Skip reflector with `--skip-addons` (metrics-server still applied).

### 4b. gen config → apply → bootstrap → join

```bash
make pertiskctl
# Single CP: use the CP guest IP (Serial / DHCP):
./out/bin/pertiskctl gen config lab-ha https://<CP_IP>:6443 -o ./out/cluster

./out/bin/pertiskctl -e <CP_IP>:50000 apply -f ./out/cluster/controlplane.yaml
./out/bin/pertiskctl -e <CP_IP>:50000 bootstrap
./out/bin/pertiskctl -e <CP_IP>:50000 kubeconfig -f ./out/cluster/admin.conf
./out/bin/pertiskctl -e <CP_IP>:50000 join-config -f ./out/cluster/worker.yaml

# HA: endpoint = VIP; bootstrap CP1, then join CP2/CP3:
./out/bin/pertiskctl gen config lab-ha https://<VIP>:6443 -o ./out/cluster --controlplanes 3
# Dual-stack HA (Talos-shaped pod/service CIDRs; optional IPv6 VIP → certSANs + kube-vip ND):
./out/bin/pertiskctl gen config lab-ha https://10.1.1.210:6443 -o ./out/cluster \
  --controlplanes 3 --dual-stack [--vip6 '2405:9800:…:210']
./out/bin/pertiskctl -e <CP1>:50000 apply -f ./out/cluster/controlplane.yaml
./out/bin/pertiskctl -e <CP1>:50000 bootstrap
./out/bin/pertiskctl -e <CP1>:50000 get-join-config --controlplane --controlplane-index 2 \
  -o ./out/cluster/controlplane-2.yaml
# edit hostname → lab-ha-cp-2, then:
./out/bin/pertiskctl -e <CP2>:50000 apply -f ./out/cluster/controlplane-2.yaml
./out/bin/pertiskctl -e <CP2>:50000 join-controlplane --etcd-endpoints https://<CP1>:2379
# (repeat for CP3; kube-vip static pod owns the VIP)

# Prefer node InternalIP IPv6 = SLAAC GUA (2405:9800:…) when the LAN sends RAs.
# Apply waits ~8s for GUA before falling back to synthetic ULA (fd00:a:1:1::xx).
# If a node still shows ULA, wait for RA then re-apply the same YAML (restarts kubelet).

# Bootstrap finalizes (once apiserver is up): bootstrap-token Secret, node-join
# RBAC, CP control-plane label/taint, CoreDNS, and metrics-server.
# Wait ~1–2 min if the node is still unlabeled.

# Per worker (edit hostname in a copy of worker.yaml):
./out/bin/pertiskctl -e <WK_IP>:50000 apply -f ./out/cluster/worker.yaml

# CNI — Cilium dual-stack (see examples/cni/cilium.md); k8sServiceHost = VIP when HA
# Or lab-up: --skip-build --skip-vms --dual-stack --cni cilium --vip 10.1.1.210
kubectl --kubeconfig ./out/cluster/admin.conf get nodes -o wide
# Expect: InternalIP v4 + InternalIP 2405:9800:… (or fd00:… if no RA)

# Flannel (IPv4-only labs):
kubectl --kubeconfig ./out/cluster/admin.conf apply -f examples/cni/kube-flannel.yaml
# Basic addons are already applied by bootstrap; re-apply if needed:
kubectl --kubeconfig ./out/cluster/admin.conf apply -f examples/dns/coredns.yaml
kubectl --kubeconfig ./out/cluster/admin.conf apply -f examples/addons/metrics-server.yaml
# optional reflector:
kubectl apply --kubeconfig ./out/cluster/admin.conf \
  -f https://github.com/emberstack/kubernetes-reflector/releases/latest/download/reflector.yaml
kubectl --kubeconfig ./out/cluster/admin.conf get nodes
```

**Control-plane images:** static pods pull `registry.k8s.io/pause`, `etcd`, `kube-apiserver`, `kube-controller-manager`, `kube-scheduler` (default tag from `pertiskctl gen config -k`, currently `v1.36.3`). HA also pulls `ghcr.io/kube-vip/kube-vip` when `cluster.endpoint` is a VIP. The guest needs outbound HTTPS to that registry **and** a system CA bundle (`/etc/ssl/certs/ca-certificates.crt` is embedded in the image). Corporate TLS interception requires injecting your proxy CA. See [COMPATIBILITY.md](./COMPATIBILITY.md). Keep the **embedded kubelet** (`make fetch-runtime`) on the same minor as `-k`.

**HA VIP:** pick an IPv4 address that is **free on the L2** (not used by DHCP or another host). kube-vip announces it with gratuitous ARP, which needs the `af_packet` module in the guest image (`make fetch-kernel` / rebuild cloud image). A busy VIP or a guest without `af_packet` leaves `:6443` unreachable off-node even while CP apiservers are healthy on their node IPs.

### 4c. Worker-only join (existing external CP)

You can still join Pertisk workers to a non-Pertisk control plane with `examples/worker-join.yaml` + `pertiskctl apply`.

## 5. Networking notes

| Item | Recommendation |
|------|----------------|
| Guest NIC | virtio on `vmbr0` (or VLAN bridge); image loads `virtio_net` module at boot |
| Guest IP | DHCP (default seed) or static in machine config |
| Management API | `:50000` (mTLS); firewall carefully |
| Metrics | `:50001` (optional bearer token) |
| CNI | Prefer `cni: none` + Cilium, Calico, or Flannel on multi-node Proxmox |
| Loopback | `lo` + `127.0.0.1/8` brought up by `pertisk-net` (required for containerd CRI) |

Ensure the worker can reach the API server on `:6443` and that Node/Pod networks are routable as your CNI expects.

## 6. Checklist

- [ ] Rotated any Proxmox password that was shared in chat
- [ ] API token in env, not in repo
- [ ] VM is **UEFI** (OVMF), not SeaBIOS
- [ ] Image built with **runtime embedded** for kubelet/containerd
- [ ] CP bootstrapped (`pertiskctl bootstrap`); workers have `ca` + token
- [ ] CNI applied; `kubectl get nodes` shows Ready

## Related

- Cloud image layout: [image/cloud/README.md](../image/cloud/README.md)
- Examples: `examples/controlplane.yaml`, `examples/worker-join.yaml`
- Multi-VM helper: `scripts/proxmox-create-cluster-vms.sh`
- Compatibility: [COMPATIBILITY.md](./COMPATIBILITY.md)



