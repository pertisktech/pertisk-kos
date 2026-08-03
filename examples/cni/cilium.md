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
# - Pertisk PID 1 mounts `/` + `/sys` + `/sys/fs/bpf` as **rshared** and mounts
#   bpffs on `/sys/fs/bpf`. Without that, Cilium fails with:
#     path "/sys/fs/bpf" is mounted on "/sys" but it is not a shared mount
#   Rebuild/redeploy the image after that change; remounting from outside a
#   production guest (no shell) is not practical.
#
# Check after install:
#   kubectl -n cilium get pods -o wide
#   cilium status   # if cilium CLI is installed on the management host
