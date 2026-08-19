//! Prometheus text exposition for node health / boot / host resource metrics.

use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Context;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

use crate::server::TlsPaths;
use crate::state::SharedState;

static METRICS_SCRAPES: AtomicU64 = AtomicU64::new(0);

/// Bind and serve `GET /metrics` (Prometheus text format).
///
/// When `tls` is `Some`, connections are TLS with **required client certificates**
/// (same CA as the management API). When `bearer_token` is `Some`, requests must
/// also include `Authorization: Bearer <token>` (case-insensitive scheme).
pub async fn serve_metrics(
    state: SharedState,
    listen: SocketAddr,
    bearer_token: Option<String>,
    tls: Option<TlsPaths>,
) -> anyhow::Result<()> {
    let acceptor = match tls.as_ref() {
        Some(paths) => Some(TlsAcceptor::from(Arc::new(build_metrics_tls(paths)?))),
        None => None,
    };

    let listener = TcpListener::bind(listen).await?;
    let auth = match (acceptor.is_some(), bearer_token.is_some()) {
        (true, true) => "mtls+bearer",
        (true, false) => "mtls",
        (false, true) => "bearer",
        (false, false) => "none",
    };
    info!(%listen, auth, "metrics endpoint listening");

    loop {
        let (sock, _) = listener.accept().await?;
        let state = state.clone();
        let token = bearer_token.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            if let Some(acceptor) = acceptor {
                match acceptor.accept(sock).await {
                    Ok(tls_sock) => handle_metrics_conn(tls_sock, state, token).await,
                    Err(err) => {
                        warn!(error = %err, "metrics TLS handshake failed");
                    }
                }
            } else {
                handle_metrics_conn(sock, state, token).await;
            }
        });
    }
}

async fn handle_metrics_conn<S>(mut sock: S, state: SharedState, token: Option<String>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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
}

/// Build a rustls server config that requires client certificates signed by `tls.ca_cert`.
pub fn build_metrics_tls(tls: &TlsPaths) -> anyhow::Result<ServerConfig> {
    let certs = load_certs(&tls.server_cert)?;
    let key = load_private_key(&tls.server_key)?;
    let mut roots = RootCertStore::empty();
    for cert in load_certs(&tls.ca_cert)? {
        roots
            .add(cert)
            .map_err(|e| anyhow::anyhow!("invalid metrics CA: {e}"))?;
    }
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| anyhow::anyhow!("metrics client verifier: {e}"))?;
    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .context("metrics TLS identity")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

fn load_certs(path: &std::path::Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut reader = Cursor::new(data);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse certs {}", path.display()))?;
    if certs.is_empty() {
        anyhow::bail!("no certificates in {}", path.display());
    }
    Ok(certs)
}

fn load_private_key(path: &std::path::Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut reader = Cursor::new(&data);
    if let Some(key) = rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("parse key {}", path.display()))?
    {
        return Ok(key);
    }
    anyhow::bail!("no private key in {}", path.display());
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

async fn write_http<S>(
    sock: &mut S,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
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

pub(crate) fn render_metrics(state: &SharedState) -> String {
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
    crate::host_metrics::HostSnapshot::collect().render_prometheus(&mut body);
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::shared;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
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
        #[cfg(target_os = "linux")]
        {
            assert!(
                body.contains("pertisk_cpu_seconds_total")
                    || body.contains("pertisk_memory_total_bytes"),
                "linux host metrics should appear in /metrics"
            );
        }
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

    #[test]
    fn builds_metrics_tls_config() {
        let dir = tempdir().unwrap();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "pertisk-test-ca");
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_ca_cert_pem(
            &ca_cert.pem(),
            KeyPair::from_pem(&ca_key.serialize_pem()).unwrap(),
        )
        .unwrap();

        let server_key = KeyPair::generate().unwrap();
        let mut server_params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        server_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "pertiskd");
        let server_cert = server_params.signed_by(&server_key, &issuer).unwrap();

        let ca_path = dir.path().join("ca.crt");
        let cert_path = dir.path().join("server.crt");
        let key_path = dir.path().join("server.key");
        std::fs::write(&ca_path, ca_cert.pem()).unwrap();
        std::fs::write(&cert_path, server_cert.pem()).unwrap();
        std::fs::write(&key_path, server_key.serialize_pem()).unwrap();

        let cfg = build_metrics_tls(&TlsPaths {
            ca_cert: ca_path,
            server_cert: cert_path,
            server_key: key_path,
        })
        .expect("build_metrics_tls");
        assert!(!cfg.alpn_protocols.is_empty());
    }
}
