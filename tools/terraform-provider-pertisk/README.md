# terraform-provider-pertisk

Terraform provider for [pertisk-mgmt](../../docs/MGMT.md): register Proxmox / vSphere / Nutanix hypervisors and create, scale, upgrade, and destroy Pertisk Kubernetes clusters.

Address: `registry.terraform.io/pertisk-tech/pertisk`

## Features

| Feature | Resource / API | Notes |
|---------|----------------|-------|
| Auth | provider | Username/password login **or** Bearer `token`; `insecure` for lab TLS; env `PERTISK_*` |
| Register hypervisor | `pertisk_provider` | `kind` = `proxmox` \| `vsphere` \| `nutanix`; token + node/storage/bridge |
| Lookup hypervisor | data `pertisk_provider` | By `name` or `id` |
| Create / destroy cluster | `pertisk_cluster` | Waits for mgmt job; exports `status`, `endpoint`, `kubeconfig` |
| HA control planes | `pertisk_cluster` | `controlplanes > 1` + `vip` (kube-vip); optional `vip6` |
| Dual-stack | `pertisk_cluster` | `network_mode = "dual-stack"`; optional IPv6 pod/service CIDRs |
| CNI | `pertisk_cluster` | `cilium` (default), `flannel`, `calico`, `none` |
| VM sizing | `pertisk_cluster` | `cp_memory` / `cp_cores` / `cp_disk_gb` / `worker_memory` / `worker_cores` / `worker_disk_gb` |
| Base VMID | `pertisk_cluster` | `cp_vmid` — CP uses base, then +1… |
| K8s version | `pertisk_cluster` | Set at create; **change triggers in-place upgrade** (no replace) |
| Scale out / in | `pertisk_node` | `mode=create` (hypervisor VM) or `mode=adopt` (existing IP) |
| Node hardware overrides | `pertisk_node` | Optional `memory` / `cores` / `disk_gb` on create |
| Install cluster add-ons | `pertisk_addon` | `nfs`, `cert-manager`, `cilium-lb`, `ingress`; waits for install job |
| Reuse add-on configs | `pertisk_cluster` | `reuse_addons` (default true) + optional `addon_preset` |
| Import | cluster / provider / node / addon | Cluster & provider by UUID; node/addon as `cluster_id/…` |

### Create-time vs scale

- `controlplanes` and `workers` on `pertisk_cluster` are **initial size only**. Later HCL changes are ignored; live inventory from add/remove node is not synced back into those fields.
- Scale with `pertisk_node` (create or adopt).
- Changing sizing, network, CNI, VIP, or `cp_vmid` forces **replace**.

### Outputs (cluster)

- `id` — cluster UUID  
- `status` — e.g. `ready`  
- `endpoint` — API host (node IP or VIP)  
- `kubeconfig` — admin kubeconfig (sensitive)

## Quick start

```bash
cd tools/terraform-provider-pertisk
make install
cd examples/basic
cp terraform.tfvars.example terraform.tfvars   # edit secrets, VIP, sizing
terraform init
terraform apply
```

Minimal sketch:

```hcl
provider "pertisk" {
  url      = var.mgmt_url
  username = var.mgmt_user
  password = var.mgmt_password
  insecure = true
}

resource "pertisk_provider" "pve" {
  name         = "tf-proxmox"
  kind         = "proxmox"
  url          = var.pve_url
  token_id     = var.pve_token_id
  token_secret = var.pve_token_secret
  node         = var.pve_node
  storage      = var.pve_storage
  bridge       = var.pve_bridge
  insecure     = true
}

resource "pertisk_cluster" "lab" {
  name          = "tf-lab"
  provider_id   = pertisk_provider.pve.id
  controlplanes = 1
  workers       = 2
  cni           = "cilium"
  cp_vmid       = 310
  k8s_version   = "v1.36.3"

  cp_memory      = 4096
  cp_cores       = 2
  cp_disk_gb     = 50
  worker_memory  = 8192
  worker_cores   = 4
  worker_disk_gb = 75
}

# Optional scale-out
resource "pertisk_node" "extra_worker" {
  cluster_id = pertisk_cluster.lab.id
  role       = "worker"
  mode       = "create"
}

resource "pertisk_addon" "nfs" {
  cluster_id = pertisk_cluster.lab.id
  addon      = "nfs"
  config = {
    server = "10.1.1.150"
    path   = "/mnt/nfs_share"
  }
}
```

Nutanix AHV (`kind = "nutanix"`; see [NUTANIX.md](../../docs/NUTANIX.md)):

```hcl
resource "pertisk_provider" "ahv" {
  name         = "tf-ahv"
  kind         = "nutanix"
  url          = "https://10.1.1.111:9440"
  token_id     = "admin"
  token_secret = var.nutanix_password
  node         = "NTNX-Cluster"
  storage      = "SelfServiceContainer"
  bridge       = "homelab-subnet"
  insecure     = true
}
```

HA + dual-stack (see `examples/basic/terraform.tfvars.example`):

```hcl
resource "pertisk_cluster" "ha" {
  name          = "tf-lab-ha"
  provider_id   = pertisk_provider.pve.id
  controlplanes = 3
  workers       = 2
  network_mode  = "dual-stack"
  vip           = "10.1.1.210" # free L2, outside DHCP
  vip6          = "fd00:1::210"
  cni           = "cilium"
  cp_vmid       = 310
  # … sizing as above
}
```

Env: `PERTISK_URL`, `PERTISK_USERNAME`, `PERTISK_PASSWORD`, `PERTISK_TOKEN`, `PERTISK_INSECURE=1`.

**Notes**
- Deleting `pertisk_provider` needs an **admin** mgmt user.
- After `make install`, delete `.terraform.lock.hcl` if the local binary checksum changes.
- VIP must be free on L2 before HA create.

## Documentation

Registry-style docs (Example Usage, Argument Reference, Attribute Reference):

| Doc | Contents |
|-----|----------|
| [`docs/index.md`](./docs/index.md) | Provider |
| [`docs/resources/cluster.md`](./docs/resources/cluster.md) | `pertisk_cluster` |
| [`docs/resources/provider.md`](./docs/resources/provider.md) | `pertisk_provider` |
| [`docs/resources/node.md`](./docs/resources/node.md) | `pertisk_node` |
| [`docs/resources/addon.md`](./docs/resources/addon.md) | `pertisk_addon` |
| [`docs/data-sources/provider.md`](./docs/data-sources/provider.md) | Data source |

Example layouts:

- [`examples/basic/`](./examples/basic/) — full lab (HA / dual-stack / sizing / optional node)
- [`examples/resources/pertisk_cluster/`](./examples/resources/pertisk_cluster/) — 3-node docs sample

## Status

| Item | Status |
|------|--------|
| Provider auth | Done |
| `pertisk_provider` + data source | Done |
| `pertisk_cluster` (HA, dual-stack, sizing, upgrade) | Done |
| `pertisk_node` (create / adopt) | Done |
| `pertisk_addon` (install / update) | Done |
| HashiCorp docs | Done |
| Unit + acceptance tests | Done |
| CI + GoReleaser | Done |
| Terraform Registry listing | After first signed release |

## Tests

```bash
make test      # unit (always)
make testacc   # live mgmt + Proxmox; needs TF_ACC=1 + env (see below)
```

Security scan (from this directory so [`.trivyignore`](./.trivyignore) applies):

```bash
trivy fs . --scanners vuln
# GO-2026-5932 is ignored: Trivy flags all of x/crypto; only openpgp is affected
# and this provider does not use it (govulncheck ./... is clean).
```

Acceptance creates **1 CP + 2 workers**, checks sizing/`status=ready`/`kubeconfig`, then destroys:

```bash
export TF_ACC=1
export PERTISK_URL=https://ptkos.example
export PERTISK_USERNAME=admin
export PERTISK_PASSWORD=…
export PERTISK_INSECURE=1
export PERTISK_ACC_PVE_URL=https://10.1.1.10:8006
export PERTISK_ACC_PVE_TOKEN_ID='root@pam!token'
export PERTISK_ACC_PVE_TOKEN_SECRET=…
export PERTISK_ACC_PVE_NODE=pve
export PERTISK_ACC_PVE_STORAGE=local-lvm
# optional: PERTISK_ACC_PVE_BRIDGE=vmbr0 PERTISK_ACC_CP_VMID=410
make testacc
```

## Release

1. Secrets: `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`
2. `git tag terraform-provider-pertisk/v0.1.0 && git push origin terraform-provider-pertisk/v0.1.0`
3. Publish the draft GitHub Release, then [Terraform Registry](https://developer.hashicorp.com/terraform/registry/providers/publishing) for `pertisk-tech` / `pertisk`
