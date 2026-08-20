# Cilium (Helm) — use with Pertisk workers set to `cluster.cni: none`.
#
# Pertisk already mounts cgroup2 + bpffs and marks `/sys` + `/sys/fs/bpf` +
# `/var` rshared (after EPHEMERAL bind). Cilium hostPath Bidirectional on
# `/var/run/netns` needs that — without a rebuilt image you get:
#   path "/var/run/netns" is mounted on "/var" but it is not a shared or slave mount
# Workaround (also applied by `proxmox-lab-up.sh`): patch the DaemonSet volume
# `cilium-netns` to `/run/netns` (already rshared). Newer images bind `/run`
# over `/var/run` so the default path works.
#
# Install on Pertisk: do **not** let Cilium remount bpf/cgroup (its mount-bpf-fs
# init with Bidirectional propagation has broken host `/proc` on Pertisk →
# containerd "stat /proc/.../ns/pid" → nodes NotReady + kubectl logs 401).
#
# Pertisk has no kube-proxy — enable kubeProxyReplacement (+ bpf.masquerade).
# Also set k8sServiceHost/Port to the real apiserver (ClusterIP unreachable
# until Cilium is up). For HA labs use the kube-vip address, not a single CP IP:
#   --set k8sServiceHost=$VIP
#
# Kernel: Alpine linux-virt builds nf_tables/vxlan/iptables/xfrm as **modules**.
# Without `xfrm_user`, Cilium CrashLoops with `protocol not supported` (netlink
# handle opens NETLINK_XFRM). Boot must load `x_tables` **before** any `xt_*` /
# `ip_tables` (otherwise unknown-symbol loads and Cilium spams tunnel iptables
# errors: Extension udp/comment/CT/socket missing). Also pack `xt_CT`,
# `xt_TPROXY`, `nf_tproxy_*`. Rebuild: `PERTISK_FORCE_KERNEL=1 ./image/fetch-kernel.sh`
# then `make cloud` / lab-up.
#
# Cilium 1.20 image defaults `iptables` → **nft**, while Pertisk uses
# **iptables-legacy** on the host. `lab-up` wraps the agent entrypoint to
# retarget iptables* → xtables-legacy-multi. Do **not** set
# `installIptablesRules=false` with Hubble/L7 unless you also enable BPF TProxy
# (`enable-bpf-tproxy`) — agent fatals: L7 proxy requires iptables or BPF TProxy.
#
# On the management host (known-good IPv4-only):
#
#   export IP=10.1.1.210   # VIP (HA) or CP advertise IP
#   export KUBECONFIG=./out/cluster/admin.conf
#   helm repo add cilium https://helm.cilium.io/
#   helm upgrade --install cilium cilium/cilium \
#     --namespace cilium --create-namespace \
#     --set ipam.mode=kubernetes \
#     --set kubeProxyReplacement=true \
#     --set 'securityContext.capabilities.ciliumAgent={CHOWN,KILL,NET_ADMIN,NET_RAW,IPC_LOCK,SYS_ADMIN,SYS_RESOURCE,DAC_OVERRIDE,FOWNER,SETGID,SETUID}' \
#     --set 'securityContext.capabilities.cleanCiliumState={NET_ADMIN,SYS_ADMIN,SYS_RESOURCE}' \
#     --set cgroup.autoMount.enabled=false \
#     --set ipv6.enabled=false \
#     --set cgroup.hostRoot=/sys/fs/cgroup \
#     --set bpf.autoMount.enabled=false \
#     --set k8sServiceHost=$IP \
#     --set k8sServicePort=6443 \
#     --set l2announcements.enabled=true \
#     --set bpf.masquerade=true \
#     --set hubble.enabled=true \
#     --set hubble.relay.enabled=true \
#     --set hubble.ui.enabled=true \
#     --set prometheus.enabled=true \
#     --set ipam.operator.clusterPoolIPv4MaskSize=24 \
#     --set hubble.relay.hostNetwork=true \
#     --set hubble.relay.dnsPolicy=ClusterFirstWithHostNet
#
# Dual-stack (cluster must be gen'd with `pertiskctl --dual-stack` so apiserver
# / controller-manager have v4,v6 CIDRs; Cilium uses kubernetes IPAM + Node.PodCIDR):
#
#   helm upgrade --install cilium cilium/cilium \
#     --namespace cilium --create-namespace \
#     --set ipam.mode=kubernetes \
#     --set kubeProxyReplacement=true \
#     --set 'securityContext.capabilities.ciliumAgent={CHOWN,KILL,NET_ADMIN,NET_RAW,IPC_LOCK,SYS_ADMIN,SYS_RESOURCE,DAC_OVERRIDE,FOWNER,SETGID,SETUID}' \
#     --set 'securityContext.capabilities.cleanCiliumState={NET_ADMIN,SYS_ADMIN,SYS_RESOURCE}' \
#     --set cgroup.autoMount.enabled=false \
#     --set ipv6.enabled=true \
#     --set enableIPv6Masquerade=true \
#     --set cgroup.hostRoot=/sys/fs/cgroup \
#     --set bpf.autoMount.enabled=false \
#     --set k8sServiceHost=$IP \
#     --set k8sServicePort=6443 \
#     --set l2announcements.enabled=true \
#     --set bpf.masquerade=true \
#     --set hubble.enabled=true \
#     --set hubble.relay.enabled=true \
#     --set hubble.ui.enabled=true \
#     --set prometheus.enabled=true \
#     --set ipam.operator.clusterPoolIPv4MaskSize=24 \
#     --set ipam.operator.clusterPoolIPv6MaskSize=112 \
#     --set hubble.relay.hostNetwork=true \
#     --set hubble.relay.dnsPolicy=ClusterFirstWithHostNet
#
#   # Required until image binds /run over /var/run (lab-up does this):
#   kubectl -n cilium patch ds cilium --type=json \
#     -p '[{"op":"replace","path":"/spec/template/spec/volumes/'"$(
#          kubectl -n cilium get ds cilium -o json \
#            | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(i for i,v in enumerate(d["spec"]["template"]["spec"]["volumes"]) if v["name"]=="cilium-netns"))'
#        )"'/hostPath/path","value":"/run/netns"}]'
#
#   # Also required (lab-up does this): wrap cilium-agent so iptables → legacy.
#
# Or: ./scripts/proxmox-lab-up.sh --skip-build --skip-vms --cni cilium
# Dual-stack: ./scripts/proxmox-lab-up.sh --dual-stack --cni cilium --vip 10.1.1.210
#
# LoadBalancer ELB IPs (L2 announcements): apply a pool after Cilium is up —
# examples/addons/cilium-lb/cilium-ip.yaml or UI → Add-ons → Cilium LoadBalancer.
#
# Notes:
# - Prefer `helm upgrade --install` (not bare `helm install`) so re-runs are idempotent.
# - Do not install Flannel or Calico together with Cilium.
# - Built-in bridge CNI (`cluster.cni: bridge`) must stay off so Cilium owns /etc/cni/net.d.
# - `ipam.mode=kubernetes` needs controller-manager `--allocate-node-cidrs` + matching
#   `--cluster-cidr` (dual-stack: `10.244.0.0/16,2001:db8:10:0::/56`).
# - Refresh kubeconfig after DHCP IP changes:
#     pertiskctl -e <CP_IP>:50000 kubeconfig -f ./out/cluster/admin.conf
#
# Check:
#   kubectl --kubeconfig ./out/cluster/admin.conf get nodes -o wide
#   kubectl --kubeconfig ./out/cluster/admin.conf -n cilium get pods -o wide
#   cilium status --wait
