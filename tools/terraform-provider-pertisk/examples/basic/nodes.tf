# Scale out: extra worker VM (Proxmox only). Set extra_worker = false to skip.
resource "pertisk_node" "extra_worker" {
  count = var.extra_worker ? 1 : 0

  cluster_id = pertisk_cluster.lab.id
  role       = "worker"
  mode       = "create"
}

# Optional add-ons (NFS / cert-manager / cilium-lb / ingress). Uncomment after the cluster is ready.
# resource "pertisk_addon" "nfs" {
#   cluster_id = pertisk_cluster.lab.id
#   addon      = "nfs"
#   config = {
#     server = "10.1.1.150"
#     path   = "/mnt/nfs_share"
#   }
# }

