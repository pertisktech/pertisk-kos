# Cilium (Helm) — use with Pertisk workers set to `cluster.cni: none`.
#
# Pertisk already mounts cgroup2 + bpffs and marks `/sys` + `/sys/fs/bpf` rshared.
# Install like Talos: do **not** let Cilium remount those (its mount-bpf-fs init
# with Bidirectional propagation has broken host `/proc` on Pertisk → containerd
# "stat /proc/.../ns/pid" → nodes NotReady + kubectl logs 401).
#
# On the management host:
#
#   helm repo add cilium https://helm.cilium.io/
#   helm upgrade --install cilium cilium/cilium --namespace cilium --create-namespace \
#     --set operator.replicas=1 \
#     --set ipam.mode=kubernetes \
#     --set bpf.autoMount.enabled=false \
#     --set cgroup.autoMount.enabled=false \
#     --set cgroup.hostRoot=/sys/fs/cgroup
#
# Optional Hubble:
#     --set hubble.relay.enabled=true --set hubble.ui.enabled=true
#
# Pertisk node config: same `cluster.cni: none` as Flannel join examples.
#
# Notes:
# - Do not install Flannel and Cilium together.
# - Built-in bridge CNI (`cluster.cni: bridge`) must stay off so Cilium owns /etc/cni/net.d.
# - Refresh kubeconfig after DHCP IP changes:
#     pertiskctl -e <CP_IP>:50000 kubeconfig -f ./out/cluster/admin.conf
# - If nodes show Ready=False "container runtime is down", rebuild/redeploy the
#   image (proc heal + no rshared `/`) and reinstall Cilium with the flags above.
#
# Check:
#   kubectl --kubeconfig ./out/cluster/admin.conf get nodes -o wide
#   kubectl --kubeconfig ./out/cluster/admin.conf -n cilium get pods -o wide
