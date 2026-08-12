variable "mgmt_url" {
  type        = string
  description = "Base URL of pertisk-mgmt (e.g. https://ptkos.example)"
}

variable "mgmt_user" {
  type        = string
  description = "Local mgmt username"
  default     = "admin"
}

variable "mgmt_password" {
  type        = string
  description = "Local mgmt password"
  sensitive   = true
}

variable "mgmt_insecure" {
  type        = bool
  description = "Skip TLS verify for mgmt (lab self-signed)"
  default     = true
}

variable "pve_url" {
  type        = string
  description = "Proxmox API URL (e.g. https://pve:8006)"
}

variable "pve_token_id" {
  type        = string
  description = "Proxmox API token id (user@realm!token)"
}

variable "pve_token_secret" {
  type        = string
  description = "Proxmox API token secret"
  sensitive   = true
}

variable "pve_node" {
  type        = string
  description = "Proxmox node name"
}

variable "pve_storage" {
  type        = string
  description = "Proxmox storage for cloud disks"
}

variable "pve_bridge" {
  type        = string
  description = "Proxmox bridge / Linux bridge"
  default     = "vmbr0"
}

variable "pve_insecure" {
  type        = bool
  description = "Skip TLS verify for Proxmox"
  default     = true
}

variable "cluster_name" {
  type        = string
  description = "Pertisk cluster name (VM prefix)"
  default     = "tf-lab"
}

variable "controlplanes" {
  type        = number
  description = "Initial control-plane count"
  default     = 1
}

variable "workers" {
  type        = number
  description = "Initial worker count"
  default     = 1
}

variable "cp_vmid" {
  type        = number
  description = "Base VMID for the cluster"
  default     = 310
}

variable "k8s_version" {
  type        = string
  description = "Kubernetes version (change triggers upgrade)"
  default     = "v1.36.3"
}

variable "cni" {
  type        = string
  description = "CNI plugin"
  default     = "cilium"
}

variable "extra_worker" {
  type        = bool
  description = "Also create one extra worker via pertisk_node"
  default     = true
}
