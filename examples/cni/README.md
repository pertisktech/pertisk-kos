# Cluster CNI (Pertisk)

Use `cluster.cni: none` on control-plane + workers, then install **one** of:

| CNI | lab-up | Service proxy | Docs |
|-----|--------|---------------|------|
| **Cilium** | `--cni cilium` (default) | kubeProxyReplacement (no kube-proxy) | [cilium.md](./cilium.md) |
| **Calico** | `--cni calico` | kube-proxy iptables | [calico.md](./calico.md) |
| **Flannel** | `--cni flannel` | kube-proxy iptables | [kube-flannel.yaml](./kube-flannel.yaml) |
| none | `--cni none` | — | bring your own |

```bash
./scripts/proxmox-lab-up.sh --cni cilium
./scripts/proxmox-lab-up.sh --cni calico
./scripts/proxmox-lab-up.sh --cni flannel
```

## Image requirements (all three)

Rebuild so modules + host iptables are embedded:

```bash
PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh
make cloud ARCH=amd64
```

Shared needs: `bridge` / `br_netfilter` / `veth`, `vxlan`, netfilter `xt_*`, host
`/usr/sbin/iptables` (legacy). Calico also uses `ipip`/`ip_set` (shipped; lab-up
defaults Calico to VXLAN).

## Rules

- Only one CNI DaemonSet may own `/etc/cni/net.d`.
- Do not enable Pertisk `cluster.cni: bridge` together with these.
- Flannel/Calico need [kube-proxy.yaml](./kube-proxy.yaml); Cilium must **not**.
