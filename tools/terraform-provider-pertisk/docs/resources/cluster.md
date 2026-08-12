---
page_title: "pertisk_cluster Resource - pertisk"
subcategory: ""
description: |-
  Create and manage a Pertisk Kubernetes cluster on an existing Proxmox/vSphere provider.
---

# pertisk_cluster (Resource)

Create a Pertisk Kubernetes cluster via pertisk-mgmt. Guests are cloud images imported on the registered hypervisor (Proxmox or vSphere).

`controlplanes` and `workers` are **create-time only**. Scale later with [`pertisk_node`](node.md). Changing VM sizing (`cp_*` / `worker_*`), network, or CNI forces **replace**.

## Example Usage

### Three-node lab (1 control-plane + 2 workers)

```terraform
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
```

### HA dual-stack with kube-vip

```terraform
resource "pertisk_cluster" "ha" {
  name          = "tf-lab-ha"
  provider_id   = pertisk_provider.pve.id
  controlplanes = 3
  workers       = 2
  network_mode  = "dual-stack"
  vip           = "10.1.1.210"
  vip6          = "fd00:1::210"
  cni           = "cilium"
  cp_vmid       = 310

  cp_memory      = 4096
  cp_cores       = 2
  cp_disk_gb     = 50
  worker_memory  = 8192
  worker_cores   = 4
  worker_disk_gb = 75
}
```

## Argument Reference

The following arguments are supported:

### Required

* `name` - (String) Cluster name (VM prefix: `{name}-cp-N` / `{name}-wk-N`). Forces new resource.
* `provider_id` - (String) Existing mgmt provider UUID (`pertisk_provider` id). Forces new resource.

### Optional

* `controlplanes` - (Number) Initial control-plane count at create (default `1`). Create-time only; later HCL drift is ignored. Use `pertisk_node` to scale. Forces new when changed before create.
* `workers` - (Number) Initial worker count at create (default `1`). Create-time only. Scale with `pertisk_node`.
* `network_mode` - (String) `ipv4` | `ipv6` | `dual-stack` (default `ipv4`). Forces new resource.
* `vip` - (String) IPv4 kube-vip. **Required** when `controlplanes > 1` on ipv4/dual-stack. Must be a free L2 address outside the DHCP pool. Forces new resource.
* `vip6` - (String) Optional IPv6 kube-vip. Forces new resource.
* `cni` - (String) CNI plugin (default `cilium`). Forces new resource.
* `k8s_version` - (String) Kubernetes version (default `v1.36.3`). Changing this triggers an in-place upgrade job (no replace).
* `cp_vmid` - (Number) Base Proxmox VMID (default `210`). First CP uses this, then +1. Forces new resource.
* `cp_memory` - (Number) Control-plane memory in MB (default `4096`). Forces new resource.
* `cp_cores` - (Number) Control-plane vCPUs (default `2`). Forces new resource.
* `cp_disk_gb` - (Number) Control-plane disk GiB (default `50`). Forces new resource.
* `worker_memory` - (Number) Worker memory in MB (default `8192`). Forces new resource.
* `worker_cores` - (Number) Worker vCPUs (default `4`). Forces new resource.
* `worker_disk_gb` - (Number) Worker disk GiB (default `75`). Forces new resource.
* `max_pods` - (Number) kubelet maxPods (default `250`). Forces new resource.
* `arch` - (String) Guest arch `amd64` | `arm64`. Omit for provider default. Forces new resource.
* `pod_subnet` - (String) IPv4 pod CIDR (default `10.244.0.0/16`). Forces new resource.
* `service_subnet` - (String) IPv4 service CIDR (default `10.96.0.0/12`). Forces new resource.
* `pod_subnet_ipv6` - (String) IPv6 pod CIDR. When omitted on dual-stack, mgmt applies its default. Forces new resource.
* `service_subnet_ipv6` - (String) IPv6 service CIDR. When omitted on dual-stack, mgmt applies its default. Forces new resource.
* `timeout_minutes` - (Number) How long to wait for create/delete/upgrade jobs (default `45`).

## Attribute Reference

In addition to the arguments above, the following attributes are exported:

* `id` - (String) Cluster UUID.
* `status` - (String) Cluster status from mgmt (`ready`, `pending`, `error`, `deleting`, …).
* `endpoint` - (String) API server endpoint host when available (node IP or VIP).
* `kubeconfig` - (String, Sensitive) Admin kubeconfig YAML once `status` is `ready`.

## Import

Import is supported using the cluster UUID:

```shell
terraform import pertisk_cluster.lab 5826cf0e-9c33-4f15-b71f-252ca1110443
```
