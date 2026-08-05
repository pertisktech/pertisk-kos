# Extension: nfs-client

Enables Kubernetes volumes that use the in-tree `kubernetes.io/nfs` plugin
(and CSI NFS drivers that call `mount -t nfs`).

## Symptom without this extension

```text
MountVolume.SetUp failed … mount: mounting 10.x.x.x:/export on … failed: No such device
```

`No such device` (ENODEV) means the guest kernel has no NFS filesystem —
not a bad export path. Fix = ship + load NFS modules and `mount.nfs`.

## Modules (`modules.txt`)

Dependency order is handled by `fetch-kernel.sh`’s `copy_module` (follows
`depends=`). Roots we request:

- `sunrpc`, `lockd`, `grace`
- `nfs`, `nfsv3`, `nfsv4` (and `nfsv2` for legacy)
- `auth_rpcgss` (common dep for NFSv4)

## Userspace

Alpine `nfs-utils`: `/sbin/mount.nfs`, `mount.nfs4`, `umount.nfs`, `umount.nfs4`.

BusyBox `mount -t nfs` looks for `/sbin/mount.nfs`.

## NFS server (external)

Do **not** run `nfs-server` inside every worker by default. Lab pattern:

- Export on mgmt / dedicated host (e.g. `10.1.1.150:/mnt/nfs_share`)
- Install [nfs-subdir-external-provisioner](../../../examples/addons/nfs/) in the cluster

See [examples/addons/nfs/README.md](../../../examples/addons/nfs/README.md).
