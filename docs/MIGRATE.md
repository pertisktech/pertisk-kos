# Migrating to Pertisk KOS

There is **no** one-click “convert old cluster → Pertisk” path. Choose by goal: move **workloads**, join **workers**, or **DR** an existing Pertisk control plane.

## Quick matrix

| Goal | Supported? | How |
|------|------------|-----|
| New Pertisk cluster, move apps | Yes (manual) | Create cluster → re-apply manifests / GitOps / Velero → cut DNS |
| Keep old CP, add Pertisk workers | Partial | [`examples/worker-join.yaml`](../examples/worker-join.yaml) + `pertiskctl apply` |
| In-place convert foreign OS → Pertisk via OS `manifest.json` | **No** | New guests from Pertisk cloud image (A/B is Pertisk→Pertisk only) |
| Restore Pertisk etcd from snapshot | Lab | `pertiskctl etcd snapshot` / `etcd restore --force` |
| Recover HA etcd with no leader | Lab | `pertiskctl etcd recover --force-new-cluster --force` |
| Register existing Pertisk host in mgmt | Yes | [Adopt](./MGMT.md#d2--adopt--join-tokens) (`POST …/nodes/adopt`) — not for kubeadm/Talos/etc. |

---

## 1. Online migrate (workloads / manifests)

**Yes for apps — not for the node OS.**

### Recommended production cutover

1. Build a Pertisk cloud image and create a new cluster (mgmt UI, lab-up, or `pertiskctl bootstrap`). See [PROXMOX.md](./PROXMOX.md) / [MGMT.md](./MGMT.md).
2. Export or re-apply Kubernetes objects from the old cluster (`kubectl get … -o yaml`, Helm, GitOps).
3. Point DNS / Ingress / LoadBalancer at the new VIP (or service IPs).
4. Drain and retire old nodes when stable.

OS A/B `manifest.json` / `manifest.sig` is only for **signed upgrades on nodes that are already Pertisk**. It does not convert a foreign OS.

### Hybrid: Pertisk workers on an external control plane

Join Pertisk workers to a non-Pertisk API server:

```bash
# Edit examples/worker-join.yaml — endpoint, token, and preferably cluster.ca
./out/bin/pertiskctl -e <PERTISK_NODE_IP>:50000 apply -f examples/worker-join.yaml
```

That does **not** turn the old control plane into Pertisk; it only adds Pertisk workers. Details: [PROXMOX.md §4c](./PROXMOX.md#4c-worker-only-join-existing-external-cp).

---

## 2. Offline backup and restore (etcd)

**Lab path for Pertisk control-plane DR** — not a general “import any cluster” tool.

### Backup (live)

```bash
./out/bin/pertiskctl -e <CP_IP>:50000 etcd snapshot
# Default: /var/lib/pertisk/etcd-snapshots/snapshot-<unix>.db
# Optional: etcd snapshot -o /path/to/snapshot.db
```

Copy the `.db` off the node for safekeeping.

### Restore (destructive)

```bash
./out/bin/pertiskctl -e <CP_IP>:50000 etcd restore \
  --file /path/to/snapshot.db \
  --force
# Optional: --member-name --initial-cluster --peer-url
```

- `--force` is required (wipes `/var/lib/etcd`).
- Stops the etcd static pod → `etcdutl snapshot restore` → re-enables the manifest.
- Prefer **single-CP** or a carefully planned HA recovery; multi-member restore needs care.
- Restores **API objects in etcd**, not PV data, external DBs, or the guest OS image.

### Recover when snapshot hangs (no etcd leader)

A live snapshot needs a leader. After DHCP reassigns control-plane IPs, members can lose quorum (`ID mismatch`) and `etcd snapshot` times out. Promote **one** surviving CP from the existing data dir (guest must include this RPC — rebuild/roll the OS image):

```bash
./out/bin/pertiskctl -e <CP_IP>:50000 etcd recover --force-new-cluster --force
```

- `--force` is required. Does **not** wipe `/var/lib/etcd`.
- Patches the etcd static pod with `--force-new-cluster`, waits until healthy, then strips the flag.
- Point kubeconfig at that CP (or wait for kube-vip). Extra CPs still run the old membership — reset and re-join them; do not run recover on more than one member.

There is **no** supported product flow for “take a kubeadm/k3s etcd dump and restore it onto a fresh Pertisk CP.”

---

## Practical pick

| Situation | Approach |
|-----------|----------|
| Production cutover | **New Pertisk cluster** + online **manifest / GitOps** migrate + DNS cutover |
| Temporary capacity on old CP | Pertisk workers **join** (`worker-join.yaml`) |
| Pertisk disaster recovery | Regular **etcd snapshot**; **restore --force** on a CP; **recover --force-new-cluster** if there is no leader |
| Old OS → Pertisk OS | **New VMs** from Pertisk image (or A/B only if already Pertisk) |

## Related

- [OS.md](./OS.md) — disk layout, A/B upgrade
- [PACKAGE.md](./PACKAGE.md) — kernel / package pins
- [MGMT.md](./MGMT.md) — adopt, join tokens, OS packages
- [PROXMOX.md](./PROXMOX.md) — bootstrap and external worker join
