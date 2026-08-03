# Calico — use with Pertisk `cluster.cni: none`

Install **after** workers join. Pertisk has no built-in kube-proxy; lab-up
installs [kube-proxy.yaml](./kube-proxy.yaml) first so ClusterIP (and Calico’s
in-cluster clients) work.

## lab-up

```bash
./scripts/proxmox-lab-up.sh --cni calico
# or reuse VMs:
./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cni calico
```

## Manual

```bash
CP_IP=<control-plane-ip>
kc=./out/cluster/admin.conf

# 1) kube-proxy (direct apiserver — ClusterIP not ready yet)
sed "s/__KUBERNETES_SERVICE_HOST__/${CP_IP}/g" examples/cni/kube-proxy.yaml \
  | kubectl --kubeconfig "$kc" apply -f -

# 2) Calico manifest, then pin VXLAN + Pertisk pod CIDR + API host
curl -fsSL https://raw.githubusercontent.com/projectcalico/calico/v3.29.3/manifests/calico.yaml \
  | kubectl --kubeconfig "$kc" apply -f -

kubectl --kubeconfig "$kc" -n kube-system set env ds/calico-node \
  CALICO_IPV4POOL_CIDR=10.244.0.0/16 \
  CALICO_IPV4POOL_IPIP=Never \
  CALICO_IPV4POOL_VXLAN=Always \
  KUBERNETES_SERVICE_HOST="${CP_IP}" \
  KUBERNETES_SERVICE_PORT=6443

kubectl --kubeconfig "$kc" -n kube-system rollout status ds/calico-node --timeout=5m
```

## Notes

- Do **not** combine with Flannel or Cilium (single owner of `/etc/cni/net.d`).
- Image must ship bridge/veth/vxlan/ipset/xt_* modules + host `iptables-legacy`
  (`PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh && make cloud`).
- Default upstream Calico uses IPIP; lab-up prefers **VXLAN** (same tunnel module
  path as Flannel/Cilium on linux-virt).
- Optional eBPF dataplane (replaces kube-proxy) is not wired yet — use iptables
  mode + kube-proxy on Pertisk for now.
