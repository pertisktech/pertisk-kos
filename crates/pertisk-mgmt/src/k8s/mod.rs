//! Per-cluster Kubernetes helpers via `kubectl` + stored kubeconfig.

mod kubectl;
mod kubelet_serving;
mod transform;

pub use kubectl::{
    helm_json, helm_output, helm_with_values, json_to_yaml, kubeconfig_tls_error, kubectl_apply_url,
    kubectl_apply_yaml, kubectl_json, kubectl_json_optional, kubectl_ok, kubectl_text,
    redact_secret_obj, refresh_kubeconfig_from_guest, resolve_cluster_kubeconfig,
    resolve_ready_kubeconfig, ResourceKind, WorkloadKind, MAX_APPLY_YAML_BYTES,
};
pub use kubelet_serving::{
    approve_pending_kubelet_serving_csrs, approve_pending_kubelet_serving_csrs_throttled,
    wait_kubelet_serving_cert,
};
pub use transform::{
    transform_configmap, transform_cronjob, transform_daemonset, transform_deployment,
    transform_event, transform_ingress, transform_job, transform_namespace, transform_node,
    transform_pod, transform_pvc, transform_resource, transform_secret, transform_service,
    transform_statefulset,
};
