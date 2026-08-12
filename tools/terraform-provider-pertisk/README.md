# terraform-provider-pertisk

Terraform provider for [pertisk-mgmt](../../docs/MGMT.md): register hypervisors and create/destroy Pertisk clusters.

## Status (v0)

| Item | Status |
|------|--------|
| Provider auth (login / token) | Done |
| Data source `pertisk_provider` | Done |
| Resource `pertisk_provider` | Done |
| Resource `pertisk_cluster` | Done (create/delete + upgrade via `k8s_version`) |
| Resource `pertisk_node` | Done (create VM / adopt / remove) |
| CI (GitHub Actions) | Done |
| GoReleaser + Registry manifest | Done |
| Terraform Registry listing | Manual (after first signed release) |

## Local install

```bash
cd tools/terraform-provider-pertisk
make install
```

## Example

```hcl
terraform {
  required_providers {
    pertisk = {
      source  = "pertisk-tech/pertisk"
      version = "0.1.0"
    }
  }
}

provider "pertisk" {
  url      = "https://ptkos.example"
  username = var.mgmt_user
  password = var.mgmt_password
  insecure = true
}

resource "pertisk_provider" "pve" {
  name         = "lab-proxmox"
  kind         = "proxmox"
  url          = "https://pve:8006"
  token_id     = "root@pam!pertisk"
  token_secret = var.pve_token_secret
  node         = "pve"
  storage      = "local-lvm"
  insecure     = true
}

resource "pertisk_cluster" "lab" {
  name          = "tf-lab"
  provider_id   = pertisk_provider.pve.id
  controlplanes = 1
  workers       = 1
  k8s_version   = "v1.36.3"
  cp_vmid       = 310
}

resource "pertisk_node" "w2" {
  cluster_id = pertisk_cluster.lab.id
  role       = "worker"
  mode       = "create"
}

output "kubeconfig" {
  value     = pertisk_cluster.lab.kubeconfig
  sensitive = true
}
```

Env: `PERTISK_URL`, `PERTISK_USERNAME`, `PERTISK_PASSWORD`, `PERTISK_TOKEN`, `PERTISK_INSECURE=1`.

**Notes**
- Deleting `pertisk_provider` needs an **admin** mgmt user.
- Cluster `controlplanes` / `workers` are create-time only; scale with `pertisk_node`.
- `pertisk_node` mode=`create` is Proxmox only.
- Import node: `terraform import pertisk_node.w2 <cluster_id>/<node_id>`

## Release (GitHub + Terraform Registry)

1. **Repo secrets** (Settings → Secrets):
   - `GPG_PRIVATE_KEY` — armored private key used to sign `SHA256SUMS`
   - `GPG_PASSPHRASE` — key passphrase (if any)

2. **Tag and push** (monorepo prefix):

```bash
git tag terraform-provider-pertisk/v0.1.0
git push origin terraform-provider-pertisk/v0.1.0
```

3. Workflow [release-terraform-provider-pertisk.yml](../../.github/workflows/release-terraform-provider-pertisk.yml) runs GoReleaser and opens a **draft** GitHub Release with zips, `SHA256SUMS`, signature, and `_manifest.json`.

4. Publish the draft release, then [publish to the Terraform Registry](https://developer.hashicorp.com/terraform/registry/providers/publishing) for namespace `pertisk-tech` / name `pertisk` (GPG public key must be registered).

Local unsigned snapshot (no publish):

```bash
make snapshot   # needs goreleaser on PATH
```

CI on PRs touching this tree: [.github/workflows/terraform-provider-pertisk.yml](../../.github/workflows/terraform-provider-pertisk.yml).
