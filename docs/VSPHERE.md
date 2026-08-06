# vSphere (ESXi) provider

Pertisk mgmt can provision clusters on **standalone ESXi** (HostAgent) via the SOAP vim25 API. This is the same idea as the [Proxmox provider](./PROXMOX.md): images live on the **mgmt** host; create uploads disks over HTTPS and talks to the hypervisor with the provider credentials only (no SSH required).

## Requirements

- ESXi 7/8 Host Client / HostAgent (not vCenter resource pools/folders UI).
- Datastore with free space for cloud images (converted to flat VMDK).
- Portgroup / standard network (lab default: `VM Network`).
- Self-signed TLS: enable **Insecure TLS** on the provider.
- Tools on the mgmt host: `curl`, `python3`, and **`qemu-img`** (`dnf install -y qemu-img`). Without host `qemu-img`, the upload script falls back to Docker/`alpine`, which needs a Docker-visible temp dir (`VSPHERE_TMPDIR`, default `/var/tmp`).

REST `/api/session` is often unavailable behind ESXi’s envoy proxy — Pertisk uses **`/sdk` SOAP** only.

## Guest image requirement (ESXi)

Pertisk cloud images historically only shipped **virtio** disk/NIC modules (Proxmox/QEMU). ESXi VMs use **LSI Logic Parallel** (`mptspi`) + **e1000e** / **vmxnet3**.

Without those modules the guest never gets a disk/NIC. Separately, linux-virt builds **framebuffer as modules** (`CONFIG_FB=m`), so the Host Client console often **stays on the last EFI line forever** even when the guest is healthy:

```text
EFI stub: Loaded initrd from LINUX EFI...
```

Do **not** use that line alone as a failure signal — wait for DHCP / Machine API (`:50000`) / lab-up, or open serial (`telnet <esxi> 23000+vmid` if the firewall allows it). Images that ship `simpledrm` + `vmwgfx` make the VGA console advance after `pertiskd` loads modules.

Rebuild and redeploy the cloud image after pulling a tree that includes `mptspi` / `e1000e` / `vmxnet3` / `vmwgfx` in `image/fetch-kernel.sh` + `pertiskd` boot module load, then **recreate** the VMs (old disks keep the old initramfs):

```bash
./image/fetch-kernel.sh          # refreshes out/modules-amd64 (mptspi/e1000e/vmwgfx)
PERTISK_EMBED_BOOT=1 ./image/build-initramfs.sh
make cloud ARCH=amd64            # or your usual stage-images path
# copy new qcow2 to mgmt images dir, recreate the cluster
```

## Provider fields

| UI field | Stored as | Example |
|---------|-----------|---------|
| URL | `url` | `https://10.1.1.20` |
| Username | `token_id` | `root` |
| Password | `token_secret_enc` | (encrypted) |
| Host | `node` | `localhost.lan` |
| Datastore | `storage` | `datastore1` |
| Network | `bridge` | `VM Network` |
| Kind | `kind` | `vsphere` |

## UI

1. **Providers → Add provider → Kind: vSphere (ESXi)**
2. Fill URL / user / password / host / datastore / network; keep Insecure TLS = Yes for lab certs.
3. **Test** — login, host, datastore, and network must succeed before Save.
4. **Clusters → Create** — pick the ESXi provider. VM / node names are `{cluster}-cp-N` / `{cluster}-wk-N` (same as Proxmox).

## Scripts

```bash
export VSPHERE_URL=https://10.1.1.20
export VSPHERE_USER=root
export VSPHERE_PASSWORD='…'
export VSPHERE_DATASTORE=datastore1
export VSPHERE_NETWORK='VM Network'
export VSPHERE_INSECURE=1

# One VM
./scripts/vsphere-upload-vm.sh --vmid 9100 --name lab-9100 \
  --disk out/pertisk-cloud-amd64.qcow2 --memory 4096 --cores 2

# Cluster VMs only
./scripts/vsphere-create-cluster-vms.sh --cp-vmid 210 --controlplanes 1 --workers 1 --no-lab-up

# Full lab (VMs + bootstrap + CNI) — needs LAB_SUBNET / L2 for MAC→IP
LAB_SUBNET=10.1.1.0/24 ./scripts/vsphere-lab-up.sh --skip-build --cp-vmid 210 --workers 1
```

Upload flow: `qemu-img convert` qcow2 → **streamOptimized VMDK** (upload ≈ used data, not full virtual size) → datastore browser PUT → `CopyVirtualDisk` to thin VMFS → `CreateVM_Task` (UEFI, LSI Logic, e1000e) → **host autostart** (`ReconfigureAutostart`) → power on.

New VMs are registered with ESXi **Autostart** (`startAction=powerOn`) and host defaults `enabled=true`, so they power on after an ESXi reboot. Start order uses the numeric VMID (lower first: CP before workers).

## IP discovery

Same as Proxmox without SSH: mgmt must share **L2** with guests (`LAB_SUBNET=10.1.1.0/24`). Lab-up reads the NIC MAC from ESXi and matches the local ARP table / parallel `:50000` scan.

## Limits

- Standalone ESXi only (no vCenter folders / DRS).
- Adding nodes to an existing vsphere cluster from the UI is not wired yet — recreate with the desired CP/worker counts.
- Do not commit live passwords; use the Providers UI or env vars.
