---
page_title: "pertisk-kos_addon Resource - pertisk-kos"
subcategory: ""
  description: |-
    Install or update a Pertisk cluster add-on (NFS, cert-manager, Cilium LB, Ingress, KOS scaler, Kubernetes Dashboard).
---

# pertisk-kos_addon (Resource)

Install a catalog add-on on a **ready** cluster through pertisk-mgmt (`POST /api/clusters/{id}/addons/{name}/install`) and wait for the job.

Changing `config` or `secrets` re-runs install. **Destroy does not uninstall** — mgmt has no remove API; Terraform only drops state.

Depends on [`pertisk-kos_cluster`](cluster.md) being ready. `cilium-lb` is only in the catalog when cluster CNI is Cilium.

## Example Usage

```terraform
resource "pertisk-kos_addon" "nfs" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "nfs"

  config = {
    server = "10.1.1.150"
    path   = "/mnt/nfs_share"
  }
}

resource "pertisk-kos_addon" "certs" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "cert-manager"

  config = {
    provider = "cloudflare"
    email    = "ops@example.com"
    acme     = "production"
    domain   = "*.lab.example.com"
  }

  secrets = {
    api_token = var.cloudflare_api_token
  }
}

resource "pertisk-kos_addon" "lb" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "cilium-lb"

  config = {
    ipv4 = "10.1.1.50"
  }
}

resource "pertisk-kos_addon" "ingress" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "ingress"

  config = {
    image_tag  = "v0.1.83"
    admin_host = "admin.lab.example.com"
    tls_secret = "none"
  }
}

resource "pertisk-kos_addon" "scaler" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "pertisk-kos-scaler"

  config = {
    username   = "admin"
    min_size   = "2"
    max_size   = "10"
    image_tag  = "0.1.0"
  }

  secrets = {
    password = var.mgmt_password
  }
}

resource "pertisk-kos_addon" "dashboard" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "kubernetes-dashboard"
}
```

Recreate a cluster and restore the last add-on configs for that name (mgmt default):

```terraform
resource "pertisk-kos_cluster" "lab" {
  name         = "tf-lab"
  provider_id  = pertisk-kos_provider.pve.id
  reuse_addons = true
  # addon_preset = "tf-lab" # optional; defaults to name
  # …
}
```

## Argument Reference

### Required

* `cluster_id` - (String) Cluster UUID. Forces new resource.
* `addon` - (String) `nfs` | `cert-manager` | `cilium-lb` | `ingress` | `pertisk-kos-scaler` | `pertisk-cd` | `kubernetes-dashboard`. Forces new resource.

### Optional

* `config` - (Map of String) Non-secret fields for the add-on.
  * **nfs:** `server`, `path`
  * **cert-manager:** `provider` (`cloudflare`), `email`, `acme` (`production`|`staging`), `domain`
  * **cilium-lb:** `ipv4`, optional `ipv6` (required on dual-stack)
  * **ingress:** `image_tag`, optional `admin_host`, `tls_secret`, `registry_user`
  * **pertisk-kos-scaler:** `username`, `min_size`, `max_size`, optional `image_tag`, `storage_class`, `mgmt_url`
  * **pertisk-cd:** `admin_email`, optional `admin_name`, `image_tag`, `host`, `tls_secret`, `git_url`, `git_group`
  * **kubernetes-dashboard:** `namespace` (default `pertisk-dashboard`), `image_tag`, `username`, optional `host`, `tls_secret` (`none` for HTTP only)
* `secrets` - (Map of String, Sensitive) `api_token` (cert-manager); `admin_password` / `registry_password` (ingress); `password` (pertisk-kos-scaler and kubernetes-dashboard); `database_url` / `admin_password` / `git_token` (pertisk-cd).
* `timeout_minutes` - (Number) Job wait timeout (default `20`).

## Attribute Reference

* `id` - (String) `cluster_id/addon`.
* `status` - (String) Mgmt add-on status (`installed`, `error`, …).

## Import

```shell
terraform import pertisk-kos_addon.nfs <cluster_id>/nfs
```
