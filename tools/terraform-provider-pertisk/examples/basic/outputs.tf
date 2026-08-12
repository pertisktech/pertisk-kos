output "provider_id" {
  description = "Registered Proxmox provider UUID"
  value       = pertisk_provider.pve.id
}

output "cluster_id" {
  description = "Cluster UUID"
  value       = pertisk_cluster.lab.id
}

output "cluster_status" {
  description = "Cluster status from mgmt"
  value       = pertisk_cluster.lab.status
}

output "endpoint" {
  description = "Kubernetes API endpoint when ready"
  value       = pertisk_cluster.lab.endpoint
}

output "kubeconfig" {
  description = "Admin kubeconfig YAML"
  value       = pertisk_cluster.lab.kubeconfig
  sensitive   = true
}

output "extra_worker_id" {
  description = "Extra worker node UUID (if extra_worker = true)"
  value       = try(pertisk_node.extra_worker[0].id, null)
}
