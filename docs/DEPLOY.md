# Production deploy

Install **pertisk-mgmt** on a Linux host, register a hypervisor, then create clusters from the UI (or Terraform). Guests are immutable: **there is no SSH into Pertisk nodes**.

Related: [MGMT.md](./MGMT.md) (packages / env), [PROXMOX.md](./PROXMOX.md), [NUTANIX.md](./NUTANIX.md), [VSPHERE.md](./VSPHERE.md).

## SSH matrix

| Hop | Proxmox (amd64) | Proxmox (arm64) | Nutanix AHV | vSphere (ESXi) |
|-----|-----------------|-----------------|-------------|----------------|
| **Build laptop → mgmt host** | Yes — copy DEB/RPM + qcow2 | Yes | Yes | Yes |
| **Mgmt → hypervisor API** | Yes — token (`:8006`) | Yes — token | Yes — user/password (`:9440`) | Yes — user/password (`/sdk` SOAP) |
| **Mgmt → hypervisor SSH** | **Not required** (`PROXMOX_NO_SSH=1`) | **Required** (`PROXMOX_SSH=root@<pve>` for `qm create --arch aarch64`) | **Not required**. Optional `NUTANIX_CVM_SSH` only to attach serial if REST fails | **Not required** (never used) |
| **Anyone → Pertisk guest** | **Never** — Machine API `:50000` / serial console | Never | Never | Never |

**L2:** mgmt must share the guest VLAN (or set `LAB_SUBNET`) so MAC→IP discovery works. If mgmt is routed-only, Proxmox can fall back to SSH ARP on the PVE bridge; ESXi and AHV cannot — put mgmt on the same L2.

**Optional Proxmox SSH** (amd64): ZFS `qm importdisk` if API import fails, and offline EPHEMERAL grow. Production default is API-only.

---

## 0. Build artifacts (once)

On a machine with Docker + Rust toolchain:

```bash
make cloud ARCH=amd64 VERSION=0.3.0          # → out/pertisk-cloud-amd64*.qcow2
# make cloud ARCH=arm64 VERSION=0.3.0        # if you need arm64 guests
make mgmt-rpm VERSION=0.3.0                  # amd64 DEB+RPM → out/pkg/
# make mgmt-pkg VERSION=0.3.0                # amd64+arm64 DEB+RPM (release)
```

Keep `VERSION` in sync on image and RPM.

---

## 1. Install pertisk-mgmt

SSH **to the mgmt host** (Alma / Rocky / RHEL, or Debian / Ubuntu):

```bash
MGMT=user@mgmt.example.com
# RPM (x86_64 or aarch64)
scp out/pkg/pertisk-mgmt-0.3.0-1.x86_64.rpm "$MGMT:/tmp/"
# DEB: scp out/pkg/pertisk-mgmt_0.3.0-1_amd64.deb "$MGMT:/tmp/"
scp out/pertisk-cloud-amd64*.qcow2 "$MGMT:/tmp/"

ssh "$MGMT" 'sudo bash -s' <<'EOF'
set -euo pipefail
rpm -Uvh /tmp/pertisk-mgmt-*.rpm
# apt-get install -y /tmp/pertisk-mgmt_*.deb
mkdir -p /var/lib/pertisk-mgmt/images
mv /tmp/pertisk-cloud-amd64*.qcow2 /var/lib/pertisk-mgmt/images/
chown -R pertisk-mgmt:pertisk-mgmt /var/lib/pertisk-mgmt/images
EOF
```

Or one-shot from the repo:

```bash
./scripts/deploy-mgmt-lab.sh --mgmt user@mgmt.example.com --version 0.3.0
```

Edit `/etc/pertisk-mgmt/pertisk-mgmt.env` **before** first start in production:

| Key | Production |
|-----|------------|
| `MGMT_SECRET_KEY` | `openssl rand -hex 32` — stable across upgrades or provider secrets will not decrypt |
| `MGMT_ADMIN_PASSWORD` | Strong password (seeded once) |
| `MGMT_PUBLIC_URL` | Public HTTPS URL (OIDC callback + serial `mgmt_url`). Never leave `http://0.0.0.0:8080` |
| `PROXMOX_NO_SSH` | `1` unless you need arm64 / ZFS SSH import |
| `LAB_SUBNET` | Guest VLAN CIDR (e.g. `10.0.0.0/24`) |
| `LAB_GATEWAY` | Optional guest default route override for AHV IPAM netcfg (auto: Prism subnet, else mgmt default route) |
| `AUTH_MODE` | `local`, `auth0`, or `both` |

```bash
sudo systemctl enable --now pertisk-mgmt
```

Put TLS in front (Caddy / nginx / load balancer) and set `MGMT_PUBLIC_URL=https://mgmt.example.com`.

---

## 2. Register a provider (UI)

**Providers → Add** → Test → Save. Credentials stay encrypted in `mgmt.db` (`MGMT_SECRET_KEY`).

Then **Clusters → Create**: pick the provider, image arch, CP/worker counts, VIP (HA), CNI.

---

## 3. Proxmox

**Need SSH to PVE?** No for amd64 API import. Yes for arm64 guests.

1. Datacenter → Permissions → API Tokens. Token needs VM.Allocate, Datastore.AllocateSpace, SDN.Use.
2. Storage **local** (directory) must allow **Import** content (upload hop). VM disks can still be `local-zfs` / `local-lvm`.
3. UI provider: URL `https://pve.example.com:8006`, token id `user@pam!pertisk`, token secret, node name, storage, bridge (`vmbr0`).
4. Create cluster. Disks upload from mgmt → Proxmox API → import-from.

Arm64 extra:

```bash
# on mgmt, after installing the pertisk-mgmt SSH key on each PVE:
# PROXMOX_SSH=root@pve.example.com
# PROXMOX_NO_SSH=0
```

CLI equivalent: `./scripts/proxmox-lab-up.sh` with `PROXMOX_URL` / token env (see [PROXMOX.md](./PROXMOX.md)).

---

## 4. Nutanix AHV

**Need SSH to Prism/CVM?** No for create / add-node. Optional `NUTANIX_CVM_SSH` only if serial-port attach via REST fails.

1. Prism Element `:9440`, storage container, managed network.
2. UI provider: URL, Prism user/password, cluster name, container, network. Insecure TLS for lab certs.
3. Mgmt host must listen on **:18765** (or `NUTANIX_HTTP_PORT`) so Prism can **pull** the qcow2. Open firewalld if needed.
4. Create cluster. Match VMs by name `{cluster}-cp-N`.

Serial: Prism VGA often stays on the EFI stub — use **Serial Console** (`ttyS0`). See [NUTANIX.md](./NUTANIX.md).

---

## 5. vSphere (standalone ESXi)

**Need SSH to ESXi?** No. SOAP `/sdk` only (not vCenter folders/DRS).

1. Build a guest image that includes **LSI Logic (`mptspi`) + e1000e/vmxnet3** (not virtio-only). See [VSPHERE.md](./VSPHERE.md).
2. `qemu-img` on mgmt (`dnf install -y qemu-img`) avoids Docker convert.
3. UI provider: ESXi URL, username/password, host, datastore, portgroup (`VM Network`).
4. Create cluster. CPU/memory resize **powers the VM off** (no CPU hot-plug for `otherLinux64Guest`).

Mgmt must be L2-adjacent for DHCP/ARP IP discovery.

---

## 6. After the cluster is Ready

- Download kubeconfig from the cluster page (or Terraform).
- CNI: Cilium (lab-up default), Calico, or Flannel — `cluster.cni: none` when a DaemonSet owns networking.
- Optional: [examples/observability](../examples/observability/) on the **mgmt** host (not on guests).
- Harden: rotate `MGMT_SECRET_KEY` only with a planned re-encrypt; change default Grafana/admin passwords; [HARDENING.md](./HARDENING.md).

Terraform: [tools/terraform-provider-pertisk](../tools/terraform-provider-pertisk/README.md).
