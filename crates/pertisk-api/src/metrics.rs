//! Prometheus text exposition for node health / boot metrics.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::state::SharedState;

static METRICS_SCRAPES: AtomicU64 = AtomicU64::new(0);

/// Bind and serve `GET /metrics` (Prometheus text format). Ignores path otherwise.
pub async fn serve_metrics(state: SharedState, listen: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    info!(%listen, "metrics endpoint listening");

    loop {
        let (mut sock, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            METRICS_SCRAPES.fetch_add(1, Ordering::Relaxed);
            let body = render_metrics(&state);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            if let Err(err) = sock.write_all(resp.as_bytes()).await {
                warn!(error = %err, "metrics write failed");
            }
        });
    }
}

fn render_metrics(state: &SharedState) -> String {
    let st = match state.lock() {
        Ok(g) => g.clone(),
        Err(_) => {
            return "# ERROR lock poisoned\n".into();
        }
    };

    let ready = if st.ready { 1 } else { 0 };
    let cd = if st.containerd == "up" { 1 } else { 0 };
    let kl = if st.kubelet == "up" { 1 } else { 0 };

    let (boot_ok, boot_attempts, active_slot) = match pertisk_update::BootMeta::load(&st.state_root)
    {
        Ok(m) => (
            if m.boot_ok { 1 } else { 0 },
            m.boot_attempts,
            m.active.to_string(),
        ),
        Err(_) => (0, 0, "unknown".into()),
    };

    let scrapes = METRICS_SCRAPES.load(Ordering::Relaxed);

    format!(
        r#"# HELP pertisk_node_ready 1 if the node management plane considers itself ready
# TYPE pertisk_node_ready gauge
pertisk_node_ready {ready}
# HELP pertisk_containerd_up 1 if containerd is running
# TYPE pertisk_containerd_up gauge
pertisk_containerd_up {cd}
# HELP pertisk_kubelet_up 1 if kubelet is running
# TYPE pertisk_kubelet_up gauge
pertisk_kubelet_up {kl}
# HELP pertisk_boot_ok 1 if current boot is marked good
# TYPE pertisk_boot_ok gauge
pertisk_boot_ok {boot_ok}
# HELP pertisk_boot_attempts Failed boot attempts on current slot
# TYPE pertisk_boot_attempts gauge
pertisk_boot_attempts {boot_attempts}
# HELP pertisk_active_slot Active A/B slot (0=A, 1=B)
# TYPE pertisk_active_slot gauge
pertisk_active_slot{{slot="{active_slot}"}} {slot_num}
# HELP pertisk_metrics_scrapes_total Metrics HTTP scrapes
# TYPE pertisk_metrics_scrapes_total counter
pertisk_metrics_scrapes_total {scrapes}
# HELP pertisk_info Node version info
# TYPE pertisk_info gauge
pertisk_info{{version="{version}",api="{api}",platform="{platform}"}} 1
"#,
        ready = ready,
        cd = cd,
        kl = kl,
        boot_ok = boot_ok,
        boot_attempts = boot_attempts,
        active_slot = active_slot,
        slot_num = if active_slot == "B" { 1 } else { 0 },
        scrapes = scrapes,
        version = st.version,
        api = st.api_version,
        platform = st.platform,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::shared;
    use tempfile::tempdir;

    #[test]
    fn renders_prometheus_text() {
        let dir = tempdir().unwrap();
        let st = shared(dir.path().to_path_buf(), dir.path().join("trust.pk"));
        let body = render_metrics(&st);
        assert!(body.contains("pertisk_node_ready"));
        assert!(body.contains("pertisk_info{"));
    }
}
