# Cilium (Helm) — use with Pertisk workers set to `cluster.cni: none`.
#
# On the control-plane / management host:
#
#   helm repo add cilium https://helm.cilium.io/
#   helm install cilium cilium/cilium --namespace kube-system \
#     --set operator.replicas=1 \
#     --set ipam.mode=kubernetes
#
# Pertisk node config: examples/worker-join-flannel.yaml (same `cni: none`).
#
# Notes:
# - Do not install Flannel and Cilium together.
# - Built-in bridge CNI (`cluster.cni: bridge`) must stay off so Cilium owns /etc/cni/net.d.
# - Ensure kubelet has CNI bin/conf dirs at /opt/cni/bin and /etc/cni/net.d (Pertisk default).
