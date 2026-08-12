resource "pertisk_provider" "pve" {
  name         = "tf-proxmox"
  kind         = "proxmox"
  url          = var.pve_url
  token_id     = var.pve_token_id
  token_secret = var.pve_token_secret
  node         = var.pve_node
  storage      = var.pve_storage
  bridge       = var.pve_bridge
  insecure     = var.pve_insecure
}

resource "pertisk_cluster" "lab" {
  name          = var.cluster_name
  provider_id   = pertisk_provider.pve.id
  controlplanes = var.controlplanes
  workers       = var.workers
  cni           = var.cni
  cp_vmid       = var.cp_vmid
  k8s_version   = var.k8s_version
}
