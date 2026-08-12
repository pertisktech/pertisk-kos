# Three-node cluster example used by docs and local smoke checks.
# See docs/resources/cluster.md

terraform {
  required_providers {
    pertisk = {
      source = "pertisk-tech/pertisk"
    }
  }
}

provider "pertisk" {
  url      = var.mgmt_url
  username = var.mgmt_user
  password = var.mgmt_password
  insecure = var.mgmt_insecure
}

variable "mgmt_url" { type = string }
variable "mgmt_user" { type = string }
variable "mgmt_password" {
  type      = string
  sensitive = true
}
variable "mgmt_insecure" {
  type    = bool
  default = true
}

variable "pve_url" { type = string }
variable "pve_token_id" { type = string }
variable "pve_token_secret" {
  type      = string
  sensitive = true
}
variable "pve_node" { type = string }
variable "pve_storage" { type = string }
variable "pve_bridge" {
  type    = string
  default = "vmbr0"
}

resource "pertisk_provider" "pve" {
  name         = "docs-proxmox"
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
  name          = "tf-docs"
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
