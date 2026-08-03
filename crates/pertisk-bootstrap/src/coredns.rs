//! Apply kubeadm-shaped CoreDNS manifests after bootstrap.

use anyhow::{Context, Result};
use tracing::info;

use crate::api::KubeClient;
use crate::manifests;

/// Embedded copy of `examples/dns/coredns.yaml`.
pub const COREDNS_YAML: &str = include_str!("../../../examples/dns/coredns.yaml");

pub fn ensure_coredns(client: &KubeClient) -> Result<()> {
    manifests::apply_yaml_documents(client, COREDNS_YAML).context("apply CoreDNS manifests")?;
    info!("CoreDNS manifests ensured (kube-dns Service 10.96.0.10)");
    Ok(())
}
