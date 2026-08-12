# Nutanix (AHV) provider

Pertisk mgmt can provision clusters on **Nutanix Prism Element** (AHV) via the REST API v2.0. Same idea as [Proxmox](./PROXMOX.md) and [vSphere](./VSPHERE.md): images live on the **mgmt** host; create uploads a qcow2 disk image over HTTPS and talks to Prism with the provider credentials only (no SSH / acli required).

## Requirements

- Prism Element (cluster VIP or CVM) on port **9440**.
- Storage container with free space for cloud images.
- AHV managed network / VLAN (lab default often `vlan.0` or similar).
- Self-signed TLS: enable **Insecure TLS** on the provider.
- Tools on the mgmt host: `curl`, `python3`, `jq` (optional `qemu-img` for virtual size).

Guest image: AHV uses virtio disk/NIC by default — the existing Pertisk cloud qcow2 (virtio modules) works without the ESXi-specific `mptspi` / `e1000e` rebuild.

## Provider fields

| UI field | Stored as | Example |
|---------|-----------|---------|
| URL | `url` | `https://10.1.1.50:9440` |
| Username | `token_id` | `admin` |
| Password | `token_secret_enc` | (encrypted) |
| Cluster / host | `node` | `NTNX-Cluster` |
| Storage container | `storage` | `SelfServiceContainer` |
| Network | `bridge` | `vlan.0` |
| Kind | `kind` | `nutanix` |

Aliases accepted by the API: `ahv`, `prism` → `nutanix`.

## VMID vs Nutanix UUID

**Base VMID** (default `210`) is Pertisk inventory only — same numbering as Proxmox (`210` = first CP, …). Prism assigns its own VM UUID. Match VMs by **name** (`{cluster}-cp-1`, …).

## Dashboard Public URL

On cluster create, mgmt sets `MGMT_PUBLIC_URL` into generated machine configs (same as Proxmox / vSphere).

## UI

1. **Providers → Add provider → Kind: Nutanix (AHV)**
2. Fill URL / user / password / cluster / storage container / network; keep Insecure TLS = Yes for lab certs.
3. **Test** — login, hosts, storage container, and network must succeed before Save.
4. **Clusters → Create** — pick the Nutanix provider. VM / node names are `{cluster}-cp-N` / `{cluster}-wk-N`.

## Scripts

```bash
export NUTANIX_URL=https://10.1.1.50:9440
export NUTANIX_USER=admin
export NUTANIX_PASSWORD='…'
export NUTANIX_STORAGE=SelfServiceContainer
export NUTANIX_NETWORK=vlan.0
export NUTANIX_INSECURE=1

# One VM
./scripts/nutanix-upload-vm.sh --vmid 9100 --name lab-9100 \
  --disk out/pertisk-cloud-amd64.qcow2 --memory 4096 --cores 2

# Cluster VMs only
./scripts/nutanix-create-cluster-vms.sh --cp-vmid 210 --controlplanes 1 --workers 1 --no-lab-up

# Full lab (VMs + bootstrap + CNI) — needs LAB_SUBNET / L2 for MAC→IP
LAB_SUBNET=10.1.1.0/24 ./scripts/nutanix-lab-up.sh --skip-build --cp-vmid 210 --workers 1
```

Upload flow: temporary HTTP server on the mgmt host (default **:18765**) → Prism **image_import_spec** pulls the qcow2 → clone disk into a UEFI AHV VM → power on.

Prism CVMs must reach the mgmt IP on that port. `No route to host` usually means **firewalld REJECT** — the script tries to open the port temporarily; for a permanent rule:

```bash
sudo firewall-cmd --permanent --add-port=18765/tcp && sudo firewall-cmd --reload
```

If Prism has an HTTP proxy, whitelist the mgmt IP. Override with `NUTANIX_HTTP_ADDR` / `NUTANIX_HTTP_PORT` if needed.

## Terraform

```hcl
resource "pertisk_provider" "ahv" {
  name         = "lab-ahv"
  kind         = "nutanix"
  url          = "https://10.1.1.50:9440"
  token_id     = "admin"
  token_secret = var.nutanix_password
  node         = "NTNX-Cluster"
  storage      = "SelfServiceContainer"
  bridge       = "vlan.0"
  insecure     = true
}
```

## Guest console (EFI stub)

AHV **VGA** freezes on:

```text
EFI stub: Loaded initrd from LINUX EFI...
```

That is **expected**: Pertisk redirects the console to **serial** (`ttyS0`), same as Proxmox. VGA will not advance.

1. Prism → VM → **Launch Console** → switch to **Serial Console** (you should see the Pertisk dashboard).
2. Or check lab-up / DHCP / `Machine API :50000`.

Recreate VMs after pulling a script that sets `vm_serial_ports` + `secure_boot: false` — older VMs may have no serial port, so stdio goes to a dead UART and it looks hung.
## Limits

- Adding nodes to an existing nutanix cluster from the UI is not wired yet — recreate with the desired CP/worker counts.
- Mgmt must share L2 with guests for MAC→IP discovery (`LAB_SUBNET`), same as ESXi lab-up.
