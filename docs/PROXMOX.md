# Deploy Pertisk workers on Proxmox VE

Pertisk is a **worker OS**. Create the Kubernetes control plane elsewhere (e.g. a Debian VM with `kubeadm`), then join Pertisk VMs.

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

Or: `make cloud ARCH=amd64` (after fetch + embed boot/runtime as needed).

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

### Finding the guest IP

Pertisk has **no qemu-guest-agent** and no SSH, so Proxmox Summary will not show an IP.

After a successful DHCP lease, Serial logs a line like:

```text
DHCP configured interface=eth0 addresses=["10.1.1.50/24"]
```

Use **Console → xterm.js / Serial** (`console=ttyS0`).

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

## 4. Join a cluster

Either bake `cluster:` into the seed config before `build-cloud-image.sh`, or after boot apply via management API:

```yaml
# examples/worker-join.yaml (edit endpoint/token/ca/podCidr)
cluster:
  endpoint: https://<cp-ip>:6443
  token: <bootstrap-token>
  ca: |
    -----BEGIN CERTIFICATE-----
    ...
  cni: bridge          # or none + Flannel/Cilium
  podCidr: 10.244.1.0/24
```

```bash
# mTLS required in production
pertiskctl -e <worker-ip>:50000 apply -f examples/worker-join.yaml
```

On the control plane:

```bash
kubectl get nodes
# For Flannel/Cilium workers: see examples/cni/
```

## 5. Networking notes

| Item | Recommendation |
|------|----------------|
| Guest NIC | virtio on `vmbr0` (or VLAN bridge) |
| Guest IP | DHCP (default seed) or static in machine config |
| Management API | `:50000` (mTLS); firewall carefully |
| Metrics | `:50001` (optional bearer token) |
| CNI | Prefer `cni: none` + Cilium/Flannel on multi-node Proxmox |

Ensure the worker can reach the API server on `:6443` and that Node/Pod networks are routable as your CNI expects.

## 6. Checklist

- [ ] Rotated any Proxmox password that was shared in chat
- [ ] API token in env, not in repo
- [ ] VM is **UEFI** (OVMF), not SeaBIOS
- [ ] Image built with **runtime embedded** for kubelet/containerd
- [ ] Control plane exists; worker join config has `ca` + token
- [ ] `kubectl get nodes` shows Ready

## Related

- Cloud image layout: [image/cloud/README.md](../image/cloud/README.md)
- Join examples: `examples/worker-join.yaml`, `examples/worker-join-flannel.yaml`
- Compatibility: [COMPATIBILITY.md](./COMPATIBILITY.md)
