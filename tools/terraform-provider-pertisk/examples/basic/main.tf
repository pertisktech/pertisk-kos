terraform {
  required_providers {
    pertisk = {
      source  = "pertisk-tech/pertisk"
      version = "0.1.0"
    }
  }
}

variable "mgmt_url" {
  type = string
}

variable "mgmt_user" {
  type    = string
  default = "admin"
}

variable "mgmt_password" {
  type      = string
  sensitive = true
}

variable "pve_url" {
  type = string
}

variable "pve_token_id" {
  type = string
}

variable "pve_token_secret" {
  type      = string
  sensitive = true
}

variable "pve_node" {
  type = string
}

variable "pve_storage" {
  type = string
}

provider "pertisk" {
  url      = var.mgmt_url
  username = var.mgmt_user
  password = var.mgmt_password
  insecure = true
}

resource "pertisk_provider" "pve" {
  name         = "tf-proxmox"
  kind         = "proxmox"
  url          = var.pve_url
  token_id     = var.pve_token_id
  token_secret = var.pve_token_secret
  node         = var.pve_node
  storage      = var.pve_storage
  bridge       = "vmbr0"
  insecure     = true
}

resource "pertisk_cluster" "lab" {
  name          = "tf-lab"
  provider_id   = pertisk_provider.pve.id
  controlplanes = 1
  workers       = 1
  cni           = "cilium"
  cp_vmid       = 310
  k8s_version   = "v1.36.3"
}

# Scale out: extra worker VM (Proxmox only)
resource "pertisk_node" "extra_worker" {
  cluster_id = pertisk_cluster.lab.id
  role       = "worker"
  mode       = "create"
}

# Or adopt an existing host:
# resource "pertisk_node" "bare" {
#   cluster_id = pertisk_cluster.lab.id
#   role       = "worker"
#   mode       = "adopt"
#   ip         = "10.1.1.50"
#   source     = "baremetal"
# }

output "cluster_id" {
  value = pertisk_cluster.lab.id
}

output "cluster_status" {
  value = pertisk_cluster.lab.status
}

output "endpoint" {
  value = pertisk_cluster.lab.endpoint
}

output "kubeconfig" {
  value     = pertisk_cluster.lab.kubeconfig
  sensitive = true
}

output "extra_worker_id" {
  value = pertisk_node.extra_worker.id
}
