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
  network_mode  = var.network_mode
  vip           = var.vip
  vip6          = var.vip6
  cni           = var.cni
  cp_vmid       = var.cp_vmid
  k8s_version   = var.k8s_version

  pod_subnet     = var.pod_subnet
  service_subnet = var.service_subnet
  # Dual-stack: set in tfvars or leave null for mgmt defaults
  # (2001:db8:10:0::/56 pods, 2001:db8:96:1::/112 services).
  pod_subnet_ipv6     = var.pod_subnet_ipv6
  service_subnet_ipv6 = var.service_subnet_ipv6
}
