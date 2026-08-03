//! Basic cluster addons applied during bootstrap finalize.

use anyhow::{Context, Result};
use tracing::info;

use crate::api::KubeClient;
use crate::manifests;

/// Embedded copy of `examples/addons/metrics-server.yaml` (with lab TLS flag).
pub const METRICS_SERVER_YAML: &str =
    include_str!("../../../examples/addons/metrics-server.yaml");

pub fn ensure_metrics_server(client: &KubeClient) -> Result<()> {
    manifests::apply_yaml_documents(client, METRICS_SERVER_YAML)
        .context("apply metrics-server manifests")?;
    info!("metrics-server manifests ensured (--kubelet-insecure-tls)");
    Ok(())
}

/// CoreDNS + metrics-server (always-on basic addons for a usable cluster).
pub fn ensure_basic_addons(client: &KubeClient) -> Result<()> {
    crate::coredns::ensure_coredns(client)?;
    ensure_metrics_server(client)?;
    Ok(())
}
