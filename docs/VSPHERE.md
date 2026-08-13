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

## VMID vs ESXi MoRef

**Base VMID** (default `210`) is Pertisk inventory only — same numbering as Proxmox (`210` = first CP, `211` = second, …). ESXi assigns its own MoRef when the VM is created (`31`, `32`, …). Host Client URLs look like `https://esxi/ui/#/host/vms/31`; that **31 is not** the Base VMID. Match VMs by **name** (`{cluster}-cp-1`, …).

## Dashboard Public URL

On cluster create, mgmt sets `MGMT_PUBLIC_URL` (Settings → Public URL) into generated machine configs:

```yaml
machine:
  dashboard:
    mgmt_url: https://ptkos.apps.thaidevops.co
```

Shown on the guest serial console. Same for Proxmox and vSphere lab-up / add-node.

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

New VMs are registered with ESXi **Autostart** (`startAction=powerOn`, `startOrder=-1`) and host defaults `enabled=true`, so they power on after an ESXi host reboot. Recreating a cluster replaces MoRefs — re-run autostart sync if VMs were created before this was wired:

```bash
VSPHERE_INSECURE=1 ./scripts/vsphere-enable-autostart.sh --prefix lab-ha-vsphere
```

Host Client: **Manage → System → Autostart** (must show Enabled + each VM listed).

## IP discovery

Same as Proxmox without SSH: mgmt must share **L2** with guests (`LAB_SUBNET=10.1.1.0/24`). Lab-up reads the NIC MAC from ESXi and matches the local ARP table / parallel `:50000` scan.

## Limits

- Standalone ESXi only (no vCenter folders / DRS).
- Scale-out via UI / Terraform `pertisk_node` (`mode=create`) uses `vsphere-add-node.sh` (upload + join, same as Proxmox / Nutanix).
- Hardware resize (CPU/memory) **powers the VM off**, applies `ReconfigVM`, then powers on. ESXi rejects live `numCPUs` changes unless CPU hot-plug is enabled for the guest OS; Pertisk VMs use `otherLinux64Guest`, which typically does not. Disk grow is online when the VM stays powered on.
- Do not commit live passwords; use the Providers UI or env vars.
