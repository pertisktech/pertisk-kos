---
page_title: "pertisk_provider Data Source - pertisk"
subcategory: ""
description: |-
  Look up an existing pertisk-mgmt provider by name or id.
---

# pertisk_provider (Data Source)

Look up a registered hypervisor provider.

## Example Usage

```terraform
data "pertisk_provider" "pve" {
  name = "lab-proxmox"
}

resource "pertisk_cluster" "lab" {
  name        = "tf-lab"
  provider_id = data.pertisk_provider.pve.id
  # …
}
```

## Argument Reference

* `id` - (Optional) Provider UUID. One of `id` or `name` is required.
* `name` - (Optional) Provider display name.

## Attribute Reference

* `id` - (String) Provider UUID.
* `name` - (String) Display name.
* `kind` - (String) `proxmox` | `vsphere`.
