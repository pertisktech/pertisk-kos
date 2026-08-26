//! Per-cluster Kubernetes helpers via `kubectl` + stored kubeconfig.

mod kubectl;
mod kubelet_serving;
mod transform;

pub use kubectl::{
    helm_output, kubeconfig_tls_error, kubectl_apply_url, kubectl_apply_yaml, kubectl_json,
    kubectl_json_optional, kubectl_ok, refresh_kubeconfig_from_guest, resolve_cluster_kubeconfig,
    resolve_ready_kubeconfig, WorkloadKind,
};
pub use kubelet_serving::{
    approve_pending_kubelet_serving_csrs, approve_pending_kubelet_serving_csrs_throttled,
    wait_kubelet_serving_cert,
};
pub use transform::{
    transform_cronjob, transform_daemonset, transform_deployment, transform_job,
    transform_namespace, transform_pod, transform_statefulset,
};
