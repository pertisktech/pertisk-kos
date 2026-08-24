# Nutanix (AHV) provider

Pertisk mgmt can provision clusters on **Nutanix Prism Element** (AHV) via the REST API v2.0. Same idea as [Proxmox](./PROXMOX.md) and [vSphere](./VSPHERE.md): images live on the **mgmt** host; create uploads a qcow2 disk image over HTTPS and talks to Prism with the provider credentials only (no SSH / acli required).

Production install and **SSH matrix**: [DEPLOY.md](./DEPLOY.md).

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

Upload flow: temporary HTTP server on the mgmt host (default **:18765**, serialized with flock) → Prism **image_import_spec** pulls the qcow2 → clone disk into a UEFI AHV VM → power on.

Prism CVMs must reach the mgmt IP on that port. `No route to host` is usually **firewalld REJECT**. `pertisk-mgmt` is not root, so it cannot open the port at runtime — RPM postinstall adds a permanent rule when firewalld is running, or:

```bash
sudo firewall-cmd --permanent --add-port=18765/tcp && sudo firewall-cmd --reload
```

Do **not** rely on per-VM ports (`18765+VMID`); those fail with `No route to host`. If Prism has an HTTP proxy, whitelist the mgmt IP. Override with `NUTANIX_HTTP_ADDR` / `NUTANIX_HTTP_PORT` if needed.

On an **unmanaged** AHV network (this lab: `vlan.0`), upload skips the IPAM **netcfg** extra disk and the mgmt `:67` DHCP helper — guests use LAN DHCP. Netcfg is only for **managed IPAM** subnets.

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

## Guest console + networking

AHV **VGA** freezes on `EFI stub: Loaded initrd…` — that is **expected**. Pertisk uses **serial** (`ttyS0`); VGA will not advance.

1. Prism → VM → **Serial Console**. PE **ignores** `vm_serial_ports` on create — the upload script attaches serial afterward (v2 PUT / v3 / optional `NUTANIX_CVM_SSH` + `acli vm.serial_port_create … type=kServer index=0`). Power-cycle after attaching.
2. Default disk bus is **`pci`** (virtio-blk → `/dev/vda`). SCSI often hangs after EFI stub on AHV; override with `NUTANIX_DISK_BUS=scsi` if needed.
3. Put guest NICs on an **unmanaged** AHV network (same virtual switch / VLAN as mgmt). This lab has `vlan.0` (vs0, VLAN 0, no IPAM) next to `homelab-subnet` (vs0, VLAN 0, **managed IPAM**). Managed IPAM shows an address in Prism immediately but does **not** lease it to the guest and usually **does not flood DHCP** onto the wire — Serial stays `(no ipv4)`, mgmt `nc` is `No route to host`, and a DHCP helper on mgmt `:67` sees no DISCOVER. Set the provider **Network / bridge** to `vlan.0` (OpenWrt DHCP on 10.1.1.0/24), or Edit `homelab-subnet` and **enable DHCP** (expand the pool if “Free IPs in Pool” is tiny). Netcfg-disk inject is a fallback when you must stay on IPAM; pin UEFI boot via v3 to pci:0.
4. Quick check from mgmt: `nc -zv <ip> 50000`. Recreate VMs after changing the network (omit `--skip-vms`). Guest image rebuild is needed only for netcfg/`sr_mod`/DHCP padding inside pertiskd.

## Stable guest IPs

Proxmox keeps the same IPv4 across stop/start because `net0` MAC is pinned from VMID and the LAN DHCP server binds that MAC.

On AHV:

- Upload now pins a **deterministic MAC** (`52:54:…` from VMID + `NUTANIX_URL`) and writes it back if create omitted it.
- On a **managed IPAM** subnet, Prism often **releases** the address when the VM is powered off, then assigns a new one on the next power-on. Upload now sets `requested_ip_address` / `ip_endpoint_list` to that first address **before** power-off, so the reservation sticks. The IPAM netcfg disk and mgmt `:67` helper still inject/serve that same address to the guest.

If the **AHV cluster itself reboots** and DHCP/IPAM still hands out a new IPv4, the guest rebases etcd/apiserver onto the live address (certs + static pods). Workers keep the **issued** kubelet client cert across reboot (STATE snapshot) instead of re-bootstrapping with an expired join token. After power-on, kubelet retries until containerd’s CRI plugin is actually serving (the socket file appearing first used to leave `kubelet=absent` and the Node **NotReady**). Mgmt rediscovers the new IP from Prism (or a `:50000` scan) and updates kubeconfig when the endpoint was the old control-plane IP. Prefer unmanaged VLAN + DHCP static leases for HA so etcd peer URLs stay stable. After an OS roll that includes etcd heal, `*-cp-1` updates member peer URLs when a leader exists, or runs `--force-new-cluster` if the whole HA set lost quorum — see [MIGRATE.md](./MIGRATE.md).

## Nodes NotReady after VM shutdown

Guests still on **0.3.9** do not retry kubelet after the containerd CRI race. Power-off → power-on leaves the Node **issued** in the API and **NotReady**. Control planes often recover (bootstrap is slower than CRI init); workers do not.

That is fixed in `pertiskd` (keep issued kubelet certs, snapshot to STATE, retry kubelet until CRI is up). It is **not** on the VM until you ship a new initramfs:

```bash
make os-trust                              # once
make os-bundle VERSION=0.3.10 ARCH=amd64   # kernel + initramfs with the new pertiskd
```

Then **Cluster → Upgrade → OS A/B upgrade** (workers first). Recreating VMs from `make cloud` also works; that is a reinstall.

Until the new guest is rolling, either open the cluster in mgmt (it re-applies machine config when `kubelet=absent`) or:

```bash
./scripts/recover-not-ready-nodes.sh ~/.kube/ptkos/lab-ha-nutanix.yaml
```

## OS A/B upgrade hangs (`Unable to find valid boot device`)

Slot staging can succeed (`upgrade ok slot=B … reboot required`) while the job still fails.

Two separate problems:

1. **False “guest API up”.** `pertiskctl upgrade --reboot` returns while `:50000` is still the *old* process. Mgmt used to treat the first TCP connect as success (~2s), sleep 3s, then `mark-boot-good` hits `No route to host` after the reboot. The job now waits for the API to **drop**, then come back, and rediscovers the Prism/DHCP address (it often changes).
2. **AHV firmware.** The extra IPAM **netcfg** virtio disk (`pci:1`) can steal UEFI after a guest reboot. Serial shows `Unable to find valid boot device` — that is firmware, not systemd-boot. Pin boot to the OS disk (`pci:0` / `scsi:0`) and power-cycle. After ~60s down, the upgrade job does this itself.

Recover the VM that is already stuck (do **not** use `--repair-name`; that re-attaches netcfg):

```bash
# on the mgmt host, with Prism env from /etc/pertisk-mgmt.env or the provider
./scripts/nutanix-upload-vm.sh --pin-boot lab-ha-nutanix-wk-1
```

Prism Serial should then show systemd-boot. Guest IPv4 may differ from `10.1.1.85`. When `:50000` is up:

```bash
pertiskctl -e <new-ip>:50000 mark-boot-good
kubectl uncordon lab-ha-nutanix-wk-1
```

Until the new mgmt package is installed, `--pin-boot` is the recovery; later OS upgrades pin and power-cycle automatically.

**Existing VMs:** recreate, or repair in place (pins MAC + requested IP, re-attaches netcfg):

```bash
./scripts/nutanix-upload-vm.sh --repair-name lab-ha-cp-1 --vmid 210
```

Prefer an unmanaged VLAN + router DHCP static leases (same as Proxmox) when you can.

Recreate VMs after updating scripts. Guest image rebuild is needed for DHCP-before-STATE / partprobe timeout fixes inside `pertiskd`.

## Stale OS-IMAGE (`pertisk-kos 0.1.0`)

Proxmox always re-imports the qcow2 from mgmt. Older Nutanix create **reused** the last ACTIVE Prism `DISK_IMAGE` named `pertisk-cloud-{vmid}-…`, so a new cluster could boot a leftover guest. `kubectl get nodes -o wide` then shows `OS-IMAGE` `pertisk-kos 0.1.0` (the workspace Cargo default baked into that old initramfs).

Upload now names the Prism image with a qcow2 content hash and re-imports when the file on mgmt changes. Recreate the VMs (Cluster → Create, or `nutanix-upload-vm.sh` without `NUTANIX_IMAGE_NAME`). Force a re-import of the same file with `NUTANIX_FORCE_IMPORT=1`.

Create imports the hashed qcow2 **once**, then clones VMs in parallel (`PERTISK_VM_JOBS`, default 4). Mgmt still runs one exclusive cluster job (create/upgrade/delete) at a time; add-on installs run in parallel.

## Limits

- Scale-out via UI / Terraform `pertisk_node` (`mode=create`) uses `nutanix-add-node.sh` (same join path as Proxmox).
- Mgmt must share L2 with guests for MAC→IP discovery (`LAB_SUBNET`), same as ESXi lab-up (Prism IP fallback helps when AHV has learned the address).
- For Serial Console without working REST attach: `export NUTANIX_CVM_SSH=nutanix@<cvm-ip>` (SSH key, BatchMode).
