# NFS storage addon (lab)

Use an **external NFS server** (mgmt host or NAS) plus
[nfs-subdir-external-provisioner](https://github.com/kubernetes-sigs/nfs-subdir-external-provisioner)
for dynamic `ReadWriteMany` PVs.

## Prerequisites

1. **Guest image with NFS client** — modules `nfs`/`nfsv3`/`nfsv4` + `mount.nfs`
   ([image/extensions/nfs-client](../../../image/extensions/nfs-client/)).
   Existing lab VMs without that image will fail with:

   ```text
   mount … failed: No such device
   ```

   Rebuild cloud images and recreate/roll nodes after upgrading Pertisk.

2. **NFS server** exporting a path reachable from every node (example below).

## Lab: NFS server on mgmt (`10.1.1.150`)

```bash
# On the mgmt host (Alma/RHEL)
sudo dnf install -y nfs-utils
sudo mkdir -p /mnt/nfs_share
sudo chmod 777 /mnt/nfs_share
echo '/mnt/nfs_share *(rw,sync,no_subtree_check,no_root_squash)' | sudo tee /etc/exports
sudo exportfs -ra
sudo systemctl enable --now nfs-server
showmount -e localhost
```

Firewall: allow TCP/UDP **2049** (and rpcbind **111** if needed) from the lab subnet.

## Install provisioner

```bash
# Until images ship boot-time NFS load (netfs→fscache→nfs), apply this first:
kubectl apply -f examples/addons/nfs/pertisk-nfs-modules-ds.yaml

export NFS_SERVER=10.1.1.150
export NFS_PATH=/mnt/nfs_share
envsubst < examples/addons/nfs/nfs-subdir-external-provisioner.yaml \
  | kubectl apply -f -

kubectl get sc,pods -n nfs-provisioner
kubectl get pods -n kube-system -l app=pertisk-nfs-modules
```

Create a test PVC:

```bash
kubectl apply -f examples/addons/nfs/test-pvc.yaml
kubectl get pvc,pv
```
