# Pertisk VMs provider

Pertisk mgmt can provision clusters on **[pertisk-vms](https://github.com/pertisk-tech/pertisk-vms)** (pertiskd QEMU/KVM) via REST `/v1`. Same idea as [Proxmox](./PROXMOX.md), [vSphere](./VSPHERE.md), and [Nutanix](./NUTANIX.md): images live on the **mgmt** host; create streams a qcow2 over HTTPS and talks to the daemon with the provider credentials only (**no SSH**).

Production install and **SSH matrix**: [DEPLOY.md](./DEPLOY.md).

## Requirements

- pertiskd HTTP **:7480** or HTTPS **:7443** (lab often uses a self-signed cert).
- Linux host with **QEMU + OVMF** (the pertisk-vms Linux default — required for KOS UEFI cloud images).
- Storage backend `replica` (or `rbd` when Ceph is configured).
- Network name or Linux bridge (lab default `vmbr0`).
- Tools on the mgmt host: `curl`, `jq`.

Guest image: virtio disk/NIC — the existing Pertisk cloud qcow2 works. **amd64 and arm64** are both allowed when `GET /v1/host` reports that arch.

## Provider fields

| UI field | Stored as | Example |
|---------|-----------|---------|
| URL | `url` | `https://10.1.1.80:7443` |
| Username | `token_id` | `admin` |
| Password | `token_secret_enc` | from `/etc/pertisk/admin` |
| Node | `node` | `n1` (cluster member `name`) |
| Storage | `storage` | `replica` |
| Network | `bridge` | `vmbr0` |
| Kind | `kind` | `pertisk-vms` |

Aliases accepted by the API: `pertisk-vm`, `pertiskvms`, `vms` → `pertisk-vms`.

Script env: `PERTISK_VMS_URL`, `PERTISK_VMS_USER`, `PERTISK_VMS_PASSWORD`, `PERTISK_VMS_NODE`, `PERTISK_VMS_STORAGE`, `PERTISK_VMS_NETWORK`, `PERTISK_VMS_INSECURE=1`. Optional static IPs: `PERTISK_VMS_STATIC_IPS` / `PERTISK_VMS_STATIC_GATEWAY`.

## VMID vs name

**Base VMID** (default `210`) is the numeric pertisk-vms VM `id` and KOS `cp_vmid` — same numbering as Proxmox (`210` = first CP, …). Match guests by **name** (`{cluster}-cp-1`, `{cluster}-wk-N`) as well.

## UI

1. **Providers → Add provider → Kind: Pertisk VMs**
2. Fill URL / user / password / node / storage / network; keep Insecure TLS = Yes for lab certs.
3. **Test** — health, login, `GET /v1/host`, cluster members, and network must succeed before Save.
4. **Clusters → Create** — pick the Pertisk VMs provider. VM / node names are `{cluster}-cp-N` / `{cluster}-wk-N`.

## Scripts

```bash
export PERTISK_VMS_URL=https://10.1.1.80:7443
export PERTISK_VMS_USER=admin
export PERTISK_VMS_PASSWORD='…'
export PERTISK_VMS_STORAGE=replica
export PERTISK_VMS_NETWORK=vmbr0
export PERTISK_VMS_INSECURE=1

# One VM
./scripts/pertisk-vms-upload-vm.sh --vmid 9100 --name lab-9100 \
  --disk out/pertisk-cloud-amd64.qcow2 --memory 4096 --cores 2

# Cluster VMs only
./scripts/pertisk-vms-create-cluster-vms.sh --cp-vmid 210 --controlplanes 1 --workers 1 --no-lab-up

# Full lab (VMs + bootstrap + CNI) — needs LAB_SUBNET / L2 for MAC→IP
LAB_SUBNET=10.1.1.0/24 ./scripts/pertisk-vms-lab-up.sh --skip-build --cp-vmid 210 --workers 1
```

Upload flow:

1. Login `POST /v1/login` → Bearer token.
2. `--import-only` streams the qcow2 once to `POST /v1/volumes/import?name=kos-cloud-{arch}&format=qcow2` (8 GiB stream, not the 64 MiB blob PUT).
3. Clone the template volume per node, optional resize, `POST /v1/vms`, attach disk + NIC (create a bridged network on `vmbr0` if missing), then start.

Guests on a bridged LAN use **DHCP** (same unmanaged path as Nutanix). Optional `--ip` / `PERTISK_VMS_STATIC_IPS` sets `AttachNicRequest.ip`.

Mgmt must share **L2** with guests (`LAB_SUBNET`) so MAC→IP discovery works. There is **no SSH** to the hypervisor.

## Terraform

```hcl
resource "pertisk_provider" "vms" {
  name         = "lab-vms"
  kind         = "pertisk-vms"
  url          = "https://10.1.1.80:7443"
  token_id     = "admin"
  token_secret = var.pertisk_vms_password
  node         = "n1"
  storage      = "replica"
  bridge       = "vmbr0"
  insecure     = true
}
```
