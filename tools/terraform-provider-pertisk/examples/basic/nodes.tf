# Scale out: extra worker VM (Proxmox only). Set extra_worker = false to skip.
resource "pertisk_node" "extra_worker" {
  count = var.extra_worker ? 1 : 0

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
