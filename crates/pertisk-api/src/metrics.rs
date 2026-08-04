//! Prometheus text exposition for node health / boot metrics.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::info;

use crate::state::SharedState;

static METRICS_SCRAPES: AtomicU64 = AtomicU64::new(0);

/// Bind and serve `GET /metrics` (Prometheus text format).
///
/// When `bearer_token` is `Some`, requests must include
/// `Authorization: Bearer <token>` (case-insensitive scheme).
pub async fn serve_metrics(
    state: SharedState,
    listen: SocketAddr,
    bearer_token: Option<String>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    if bearer_token.is_some() {
        info!(%listen, auth = "bearer", "metrics endpoint listening");
    } else {
        info!(%listen, auth = "none", "metrics endpoint listening");
    }

    loop {
        let (mut sock, _) = listener.accept().await?;
        let state = state.clone();
        let token = bearer_token.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);

            if !path_is_metrics(&req) {
                let _ = write_http(&mut sock, 404, "text/plain", b"not found\n").await;
                return;
            }

            if let Some(ref expected) = token {
                if !bearer_authorized(&req, expected) {
                    let _ = write_http(&mut sock, 401, "text/plain", b"unauthorized\n").await;
                    return;
                }
            }

            METRICS_SCRAPES.fetch_add(1, Ordering::Relaxed);
            let body = render_metrics(&state);
            let _ = write_http(&mut sock, 200, "text/plain; version=0.0.4", body.as_bytes()).await;
        });
    }
}

fn path_is_metrics(req: &str) -> bool {
    let line = req.lines().next().unwrap_or("");
    // GET /metrics HTTP/1.1  (also allow /metrics?…)
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    method.eq_ignore_ascii_case("GET") && (path == "/metrics" || path.starts_with("/metrics?"))
}

fn bearer_authorized(req: &str, expected: &str) -> bool {
    for line in req.lines() {
        let line = line.trim_end_matches('\r');
        let Some(rest) = line
            .strip_prefix("Authorization:")
            .or_else(|| line.strip_prefix("authorization:"))
        else {
            continue;
        };
        let rest = rest.trim();
        if let Some(tok) = rest
            .strip_prefix("Bearer ")
            .or_else(|| rest.strip_prefix("bearer "))
        {
            return tok.trim() == expected;
        }
    }
    false
}

async fn write_http(
    sock: &mut tokio::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(resp.as_bytes()).await?;
    sock.write_all(body).await?;
    Ok(())
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

    let mut body = format!(
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
    );
    crate::api_metrics::snapshot().render_prometheus(&mut body);
    body
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
        assert!(body.contains("pertisk_api_requests_total"));
        assert!(body.contains("pertisk_api_request_duration_seconds_sum"));
    }

    #[test]
    fn accepts_bearer_header() {
        let req = "GET /metrics HTTP/1.1\r\nAuthorization: Bearer s3cret\r\n\r\n";
        assert!(bearer_authorized(req, "s3cret"));
        assert!(!bearer_authorized(req, "nope"));
        assert!(!bearer_authorized(
            "GET /metrics HTTP/1.1\r\n\r\n",
            "s3cret"
        ));
    }

    #[test]
    fn metrics_path_only() {
        assert!(path_is_metrics("GET /metrics HTTP/1.1"));
        assert!(path_is_metrics("GET /metrics?foo=1 HTTP/1.1"));
        assert!(!path_is_metrics("GET / HTTP/1.1"));
        assert!(!path_is_metrics("POST /metrics HTTP/1.1"));
    }
}
