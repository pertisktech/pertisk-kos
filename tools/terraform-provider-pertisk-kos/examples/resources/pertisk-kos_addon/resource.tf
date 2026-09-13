resource "pertisk-kos_addon" "nfs" {
  cluster_id = pertisk-kos_cluster.lab.id
  addon      = "nfs"

  config = {
    server = "10.1.1.150"
    path   = "/mnt/nfs_share"
  }
}
