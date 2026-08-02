# Deploy Pertisk on Proxmox VE

Pertisk KOS is a **Talos-shaped** Kubernetes OS: the **same cloud image** runs as `controlplane` or `worker` (role comes from machine config). Create/join clusters with `pertiskctl` (see [Talos-shaped cluster](#talos-shaped-cluster-1-cp--workers) below).

Do **not** put Proxmox root passwords in git, chat, or scripts. Use an **API token**.

## 1. Build a cloud disk

On your build machine (Docker required):

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

Or: `make cloud ARCH=amd64` (fetches/embeds boot + containerd/kubelet, then builds qcow2).

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
```

The script:

1. Uploads the qcow2 to the datastore as an importable disk
2. Creates a **q35** VM with **OVMF (UEFI)** and virtio NIC/disk
3. Attaches the imported disk and starts the VM (unless `--no-start`)

Manual UI alternative: upload `pertisk-cloud-amd64.qcow2` to storage → Create VM (UEFI, q35) → attach disk → start.

### Boot menu

Cloud images set systemd-boot `timeout 0` so there is **no countdown menu**. With `vga=serial0`, that menu’s text is usually garbled on Serial; the VM boots straight into the kernel.

### Console dashboard + finding the guest IP

Pertisk has **no qemu-guest-agent** and no SSH, so Proxmox Summary will not show an IP.

After boot, `pertiskd` shows a Serial TUI (ptkube-dashboard style): **node** → full-width **network** (`iface  state  IP/prefix` via getifaddrs / `ip -br addr`) → **mem**|**services** → **logs** (hand-painted ASCII frame). Size is **pinned 80×24** (override with `PERTISK_DASHBOARD_COLS` / `PERTISK_DASHBOARD_ROWS`) so lines do not wrap; cursor is hidden (`CSI ?25l`); ~2s refresh.

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

**`TASK ERROR: connection timed out`** on Console is often a **VNC proxy** failure — check Serial instead, or SSH: `qm terminal <vmid>`.

If `qm config` shows **no `scsi0`** (only `efidisk0` / `boot: order=net0`), the OS disk was lost — re-attach:

```bash
PROXMOX_SSH=root@10.1.1.197 ./scripts/proxmox-reattach-disk.sh 9100 out/pertisk-cloud-amd64.qcow2
```

Other boot tips:

| Issue | Fix |
|-------|-----|
| Secure Boot / Microsoft keys | `efidisk` with `pre-enrolled-keys=0` (script above recreates it) |
| Wrong boot disk | Options → Boot Order → `scsi0` first |
| No EFI disk | Hardware → Add EFI Disk |
| Still UEFI shell | Confirm GPT has ESP; re-import cloud image |

## 4. Talos-shaped cluster (1 CP + workers)

Same qcow2 for control-plane and workers. Prefer VMIDs **210+** so lab VM 200 is untouched.

### 4a. Create VMs

```bash
make cloud ARCH=amd64   # embed boot + runtime as usual
# Uses ./proxmox.sh for PROXMOX_* if not already exported (does not run its exec).
./scripts/proxmox-create-cluster-vms.sh --cp-vmid 210 --workers 2
# → 210 pertisk-cp-1, 211 pertisk-wk-1, 212 pertisk-wk-2
```

### 4b. gen config → apply → bootstrap → join

```bash
make pertiskctl
# Use the CP guest IP (Serial / DHCP), not the VIP until HA exists:
./out/bin/pertiskctl gen config lab-ha https://<CP_IP>:6443 -o ./out/cluster

./out/bin/pertiskctl -e <CP_IP>:50000 apply -f ./out/cluster/controlplane.yaml
./out/bin/pertiskctl -e <CP_IP>:50000 bootstrap
./out/bin/pertiskctl -e <CP_IP>:50000 kubeconfig -f ./out/cluster/admin.conf
./out/bin/pertiskctl -e <CP_IP>:50000 join-config -f ./out/cluster/worker.yaml

# After apiserver is up, create the bootstrap token Secret (written on the CP):
kubectl --kubeconfig ./out/cluster/admin.conf apply \
  -f <(ssh …)  # or copy /var/lib/pertisk/kubernetes/bootstrap-token-secret.yaml from the CP
# Lab: use Console to note the path; apply via kubectl once you can reach :6443

# Per worker (edit hostname in a copy of worker.yaml):
./out/bin/pertiskctl -e <WK_IP>:50000 apply -f ./out/cluster/worker.yaml

kubectl --kubeconfig ./out/cluster/admin.conf apply -f examples/cni/kube-flannel.yaml
kubectl --kubeconfig ./out/cluster/admin.conf get nodes
```

**Control-plane images:** static pods pull `registry.k8s.io/pause`, `etcd`, `kube-apiserver`, `kube-controller-manager`, `kube-scheduler` (default tag `v1.32.5`). The guest needs outbound HTTPS to that registry **and** a system CA bundle (`/etc/ssl/certs/ca-certificates.crt` is embedded in the image). Corporate TLS interception requires injecting your proxy CA. See [COMPATIBILITY.md](./COMPATIBILITY.md).

### 4c. Worker-only join (existing external CP)

You can still join Pertisk workers to a non-Pertisk control plane with `examples/worker-join.yaml` + `pertiskctl apply`.

## 5. Networking notes

| Item | Recommendation |
|------|----------------|
| Guest NIC | virtio on `vmbr0` (or VLAN bridge); image loads `virtio_net` module at boot |
| Guest IP | DHCP (default seed) or static in machine config |
| Management API | `:50000` (mTLS); firewall carefully |
| Metrics | `:50001` (optional bearer token) |
| CNI | Prefer `cni: none` + Cilium/Flannel on multi-node Proxmox |
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
