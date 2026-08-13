# Cluster addons

## Basic (always on init)

Applied by **pertisk bootstrap finalize** (and re-applied by `proxmox-lab-up.sh`):

| Addon | Manifest | Purpose |
|-------|----------|---------|
| CoreDNS | [examples/dns/coredns.yaml](../dns/coredns.yaml) | Cluster DNS (`kube-dns` **10.96.0.10**) |
| Metrics Server | [metrics-server.yaml](./metrics-server.yaml) | `kubectl top` / HPA (`--kubelet-insecure-tls` for lab) |

```bash
# Manual re-apply
kubectl apply -f examples/dns/coredns.yaml
kubectl apply -f examples/addons/metrics-server.yaml
```

Pods stay Pending until a CNI is Ready (and, with Cilium, until
`node.cilium.io/agent-not-ready` is cleared).

## Optional (lab-up)

[emberstack/kubernetes-reflector](https://github.com/emberstack/kubernetes-reflector) — mirrors
Secrets/ConfigMaps across namespaces. Installed by lab-up unless `--skip-addons`:

```bash
kubectl apply -f https://github.com/emberstack/kubernetes-reflector/releases/latest/download/reflector.yaml
```

## Storage (NFS)

Dynamic RWX volumes via external NFS + nfs-subdir-external-provisioner:

→ [nfs/README.md](./nfs/README.md)

Requires guest **nfs-client** image extension
([image/extensions/nfs-client](../../image/extensions/nfs-client/)).
Without it, mounts fail with `No such device`.

## Observability (host metrics)

Node OS metrics (CPU, RAM, network, disk I/O) are on `pertiskd` `:50001/metrics` — not this addon. See [../observability/README.md](../observability/README.md). metrics-server stays for `kubectl top` / HPA.