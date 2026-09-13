---
page_title: "pertisk-kos_provider Data Source - pertisk-kos"
subcategory: ""
description: |-
  Look up an existing pertisk-mgmt provider by name or id.
---

# pertisk-kos_provider (Data Source)

Look up a registered hypervisor provider.

## Example Usage

```terraform
data "pertisk-kos_provider" "pve" {
  name = "lab-proxmox"
}

resource "pertisk-kos_cluster" "lab" {
  name        = "tf-lab"
  provider_id = data.pertisk-kos_provider.pve.id
  # …
}
```

## Argument Reference

* `id` - (Optional) Provider UUID. One of `id` or `name` is required.
* `name` - (Optional) Provider display name.

## Attribute Reference

* `id` - (String) Provider UUID.
* `name` - (String) Display name.
* `kind` - (String) `proxmox` | `vsphere` | `nutanix` | `pertisk-vms`.
