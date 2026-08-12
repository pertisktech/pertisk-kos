---
page_title: "pertisk_provider Resource - pertisk"
subcategory: ""
description: |-
  Register a Proxmox or vSphere hypervisor with pertisk-mgmt.
---

# pertisk_provider (Resource)

Register a hypervisor connection used by [`pertisk_cluster`](cluster.md).

Deleting this resource requires an **admin** mgmt user.

## Example Usage

```terraform
resource "pertisk_provider" "pve" {
  name         = "lab-proxmox"
  kind         = "proxmox"
  url          = "https://10.1.1.10:8006"
  token_id     = "root@pam!pertisk"
  token_secret = var.pve_token_secret
  node         = "pve"
  storage      = "local-lvm"
  bridge       = "vmbr0"
  insecure     = true
}
```

## Argument Reference

### Required

* `name` - (String) Display name in mgmt.
* `url` - (String) Hypervisor API URL.
* `token_id` - (String) API token id (Proxmox `user@realm!token`) or vSphere equivalent.
* `token_secret` - (String, Sensitive) API token secret.
* `node` - (String) Proxmox node name (or vSphere cluster/host per mgmt).
* `storage` - (String) Storage for cloud disks.
* `bridge` - (String) Network bridge (default often `vmbr0`).

### Optional

* `kind` - (String) `proxmox` | `vsphere` (default `proxmox`).
* `insecure` - (Boolean) Skip TLS verify (default `false`).

## Attribute Reference

* `id` - (String) Provider UUID in pertisk-mgmt.

## Import

```shell
terraform import pertisk_provider.pve a2fb8554-f6b9-433d-9f6e-e3d738eff677
```
