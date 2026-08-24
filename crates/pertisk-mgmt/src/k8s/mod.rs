//! Per-cluster Kubernetes helpers via `kubectl` + stored kubeconfig.

mod kubectl;
mod transform;

pub use kubectl::{
    helm_output, kubeconfig_tls_error, kubectl_apply_url, kubectl_apply_yaml, kubectl_json,
    kubectl_json_optional, kubectl_ok, refresh_kubeconfig_from_guest, resolve_cluster_kubeconfig,
    resolve_ready_kubeconfig, WorkloadKind,
};
pub use transform::{
    transform_cronjob, transform_daemonset, transform_deployment, transform_job,
    transform_namespace, transform_pod, transform_statefulset,
};
