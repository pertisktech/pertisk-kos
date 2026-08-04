cluster: my-ui-cluster
endpoint: https://10.1.1.210:6443
token: zbris9.g4izb3loczo3otmy

1) Apply controlplane.yaml to the CP Machine API (:50000)
2) pertiskctl bootstrap -e <cp-ip>:50000
3) pertiskctl kubeconfig -e <cp-ip>:50000 -f admin.conf
4) pertiskctl join-config -e <cp-ip>:50000 -f worker.yaml
5) Apply worker.yaml to each worker (unique hostname); install CNI
Bootstrap also creates the join token Secret, node-join RBAC, and
labels the CP node-role.kubernetes.io/control-plane= (kubeadm-shaped).
