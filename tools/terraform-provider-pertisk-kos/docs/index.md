---
page_title: "Provider: Pertisk KOS"
description: |-
  The Pertisk KOS provider manages Kubernetes clusters on Proxmox, vSphere, Nutanix, or Pertisk VMs via pertisk-mgmt.
---

# Pertisk KOS Provider

The Pertisk KOS provider talks to [pertisk-mgmt](https://github.com/pertisk-tech/pertisk-kos) to register hypervisors and create/destroy Pertisk Kubernetes clusters (cloud images on Proxmox, vSphere, Nutanix, or Pertisk VMs).

## Example Usage

```terraform
terraform {
  required_providers {
    pertisk-kos = {
      source  = "pertisk-tech/pertisk-kos"
      version = "~> 0.1"
    }
  }
}

provider "pertisk-kos" {
  url      = "https://ptkos.example"
  username = "admin"
  password = var.mgmt_password
  insecure = true # lab self-signed TLS
}

# Register Proxmox, then create a 3-node cluster (1 CP + 2 workers).
resource "pertisk-kos_provider" "pve" {
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

resource "pertisk-kos_cluster" "lab" {
  name          = "tf-lab"
  provider_id   = pertisk-kos_provider.pve.id
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

## HA + dual-stack example

```terraform
resource "pertisk-kos_cluster" "ha" {
  name          = "tf-lab-ha"
  provider_id   = pertisk-kos_provider.pve.id
  controlplanes = 3
  workers       = 2
  network_mode  = "dual-stack"
  vip           = "10.1.1.210"   # free L2 address outside DHCP
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

The following arguments are supported in the provider block:

* `url` - (Optional) Base URL of pertisk-mgmt. Env: `PERTISK_URL`.
* `username` - (Optional) Local auth username. Env: `PERTISK_USERNAME`. Ignored when `token` is set.
* `password` - (Optional, Sensitive) Local auth password. Env: `PERTISK_PASSWORD`.
* `token` - (Optional, Sensitive) Bearer JWT. Env: `PERTISK_TOKEN`. If set, login is skipped.
* `insecure` - (Optional) Skip TLS verify for mgmt. Env: `PERTISK_INSECURE=1`.

## Schema

### Optional

- `url` (String)
- `username` (String)
- `password` (String, Sensitive)
- `token` (String, Sensitive)
- `insecure` (Boolean)
