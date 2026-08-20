# Cilium LoadBalancer IP pool (L2 ELB)

Cilium kubeProxyReplacement can allocate `Service type: LoadBalancer` IPs from a
pool and announce them with L2 (same idea as MetalLB ARP/NDP).

Requires cluster **CNI = cilium** and Helm install with `l2announcements.enabled=true`
(lab-up default). See [examples/cni/cilium.md](../../cni/cilium.md).

## Management UI

Cluster → **Add-ons** → **Cilium LoadBalancer** (only listed when CNI is Cilium):

| Field | When |
|-------|------|
| ELB IPv4 | IPv4 and dual-stack clusters |
| ELB IPv6 | Dual-stack (and IPv6-only). Hidden on IPv4-only |

Bare IPs are stored as `/32` or `/128`.

## Manual apply

```bash
# Edit IPs, then:
kubectl apply -f examples/addons/cilium-lb/cilium-ip.yaml

kubectl get ciliumloadbalancerippool,ciliuml2announcementpolicy
```

Use a **free** L2 address on the node network (not the VIP or a node IP). Dual-stack
pools include both families in `spec.blocks`.
