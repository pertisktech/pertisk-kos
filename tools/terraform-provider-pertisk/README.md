# terraform-provider-pertisk

Terraform provider for [pertisk-mgmt](../../docs/MGMT.md): register hypervisors and create/destroy Pertisk clusters.

## Status

| Item | Status |
|------|--------|
| Provider auth (login / token) | Done |
| Resource `pertisk_provider` | Done |
| Resource `pertisk_cluster` | Done (create/delete + upgrade via `k8s_version`) |
| Resource `pertisk_node` | Done (create VM / adopt / remove) |
| Data source `pertisk_provider` | Done |
| HashiCorp docs (`docs/`) | Done |
| Acceptance tests (`TF_ACC=1`) | Done |
| CI + GoReleaser | Done |

## Documentation

Registry-style docs (Example Usage, Argument Reference, Attribute Reference):

- [`docs/index.md`](./docs/index.md) — provider
- [`docs/resources/cluster.md`](./docs/resources/cluster.md) — **`pertisk_cluster`**
- [`docs/resources/provider.md`](./docs/resources/provider.md)
- [`docs/resources/node.md`](./docs/resources/node.md)
- [`docs/data-sources/provider.md`](./docs/data-sources/provider.md)

## Local install

```bash
cd tools/terraform-provider-pertisk
make install
```

## Example (3-node / HA)

Split layout under [`examples/basic`](./examples/basic/):

```bash
make install
cd examples/basic
cp terraform.tfvars.example terraform.tfvars   # edit secrets + VIP
terraform init
terraform apply
```

Hardware sizing (`cp_memory` / `cp_cores` / `cp_disk_gb` / `worker_*`) and HA dual-stack (`network_mode`, `vip`, `vip6`) are in `terraform.tfvars.example`.

Env overrides: `PERTISK_URL`, `PERTISK_USERNAME`, `PERTISK_PASSWORD`, `PERTISK_TOKEN`, `PERTISK_INSECURE=1`.

**Notes**
- Deleting `pertisk_provider` needs an **admin** mgmt user.
- Cluster `controlplanes` / `workers` are create-time only; scale with `pertisk_node`.
- Sizing and network attrs force **replace**.
- Import node: `terraform import pertisk_node.w2 <cluster_id>/<node_id>`

## Tests

Unit (always):

```bash
make test
```

Acceptance (live mgmt + Proxmox — creates a 1 CP + 2 worker cluster, then destroys):

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

## Release (GitHub + Terraform Registry)

1. Repo secrets: `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE`
2. Tag: `git tag terraform-provider-pertisk/v0.1.0 && git push origin terraform-provider-pertisk/v0.1.0`
3. Publish the draft GitHub Release, then register on the [Terraform Registry](https://developer.hashicorp.com/terraform/registry/providers/publishing) (`pertisk-tech` / `pertisk`).
