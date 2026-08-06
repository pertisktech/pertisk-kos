//! Per-cluster Kubernetes helpers via `kubectl` + stored kubeconfig.

mod kubectl;
mod transform;

pub use kubectl::{
    kubectl_json, kubectl_ok, resolve_cluster_kubeconfig, resolve_ready_kubeconfig, WorkloadKind,
};
pub use transform::{
    transform_cronjob, transform_daemonset, transform_deployment, transform_job,
    transform_namespace, transform_pod, transform_statefulset,
};
