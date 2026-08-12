---
page_title: "pertisk_node Resource - pertisk"
subcategory: ""
description: |-
  Add a VM node or adopt an existing host into a Pertisk cluster.
---

# pertisk_node (Resource)

Add a worker or control-plane after cluster create. Prefer this over changing `pertisk_cluster.workers` / `controlplanes` (those are create-time only).

`mode=create` provisions a new VM (**Proxmox only**). `mode=adopt` joins an existing host by Machine API IP.

## Example Usage

```terraform
resource "pertisk_node" "extra_worker" {
  cluster_id = pertisk_cluster.lab.id
  role       = "worker"
  mode       = "create"

  # Optional hardware overrides (create only):
  # memory  = 8192
  # cores   = 4
  # disk_gb = 75
}

# Adopt an existing Pertisk guest:
# resource "pertisk_node" "bare" {
#   cluster_id = pertisk_cluster.lab.id
#   role       = "worker"
#   mode       = "adopt"
#   ip         = "10.1.1.50"
#   source     = "baremetal"
# }
```

## Argument Reference

### Required

* `cluster_id` - (String) Cluster UUID. Forces new resource.
* `role` - (String) `controlplane` | `worker`. Forces new resource.

### Optional

* `mode` - (String) `create` (default) | `adopt`. Forces new resource.
* `ip` - (String) Required for `mode=adopt` (Machine API IPv4). Computed after create/join.
* `name` - (String) Optional hostname for adopt; otherwise mgmt assigns `{cluster}-cp-N` / `{cluster}-wk-N`.
* `source` - (String) For adopt: `adopted` | `baremetal`. After create, API may report `proxmox` | `vsphere`.
* `memory` - (Number) Optional memory MB override (`mode=create`). Forces new resource.
* `cores` - (Number) Optional vCPU override. Forces new resource.
* `disk_gb` - (Number) Optional disk GiB override. Forces new resource.
* `timeout_minutes` - (Number) Job wait timeout (default `45`).

## Attribute Reference

* `id` - (String) Node UUID.
* `vmid` - (Number) Proxmox VMID when applicable.
* `status` - (String) Node status from mgmt.
* `ip` - (String) Node IPv4 when known.
* `source` - (String) Provenance from API.

## Import

```shell
terraform import pertisk_node.extra_worker <cluster_id>/<node_id>
```
