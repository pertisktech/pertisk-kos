//! Minimal sync Kubernetes API client (admin cert → local apiserver).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};

pub struct KubeClient {
    config: Arc<ClientConfig>,
    host: String,
    port: u16,
}

impl KubeClient {
    /// Connect to local apiserver with admin client certs (PEM).
    pub fn local(ca_pem: &str, client_crt: &str, client_key: &str) -> Result<Self> {
        let mut roots = RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
            roots
                .add(cert.context("parse cluster CA")?)
                .context("add cluster CA")?;
        }
        if roots.is_empty() {
            bail!("no certificates in cluster CA PEM");
        }

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut client_crt.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .context("parse client certificate")?;
        let key = rustls_pemfile::private_key(&mut client_key.as_bytes())
            .context("parse client key")?
            .context("no private key in client PEM")?;

        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(certs, key)
            .context("build rustls client config")?;

        Ok(Self {
            config: Arc::new(config),
            host: "127.0.0.1".into(),
            port: 6443,
        })
    }

    pub fn request(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Option<&str>,
    ) -> Result<(u16, String)> {
        let server_name = ServerName::try_from(self.host.clone()).context("server name")?;
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .with_context(|| format!("connect {}:{}", self.host, self.port))?;
        tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(30)))?;

        let conn = rustls::ClientConnection::new(self.config.clone(), server_name)
            .context("rustls connection")?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);

        let body_bytes = body.unwrap_or("").as_bytes();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nAccept: application/json\r\nUser-Agent: pertisk-bootstrap/0.1\r\n",
            host = self.host,
            port = self.port,
        );
        if let Some(ct) = content_type {
            req.push_str(&format!("Content-Type: {ct}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body_bytes.len()));
        tls.write_all(req.as_bytes())?;
        if !body_bytes.is_empty() {
            tls.write_all(body_bytes)?;
        }
        tls.flush()?;

        let mut raw = Vec::new();
        tls.read_to_end(&mut raw).ok();
        let text = String::from_utf8_lossy(&raw);
        let (status, resp_body) = parse_http_response(&text)?;
        Ok((status, resp_body))
    }

    pub fn get(&self, path: &str) -> Result<(u16, String)> {
        self.request("GET", path, None, None)
    }

    pub fn post_json(&self, path: &str, body: &str) -> Result<(u16, String)> {
        self.request("POST", path, Some("application/json"), Some(body))
    }

    pub fn patch_strategic(&self, path: &str, body: &str) -> Result<(u16, String)> {
        self.request(
            "PATCH",
            path,
            Some("application/strategic-merge-patch+json"),
            Some(body),
        )
    }

    pub fn patch_merge(&self, path: &str, body: &str) -> Result<(u16, String)> {
        self.request(
            "PATCH",
            path,
            Some("application/merge-patch+json"),
            Some(body),
        )
    }
}

fn parse_http_response(raw: &str) -> Result<(u16, String)> {
    let header_end = raw
        .find("\r\n\r\n")
        .or_else(|| raw.find("\n\n"))
        .context("incomplete HTTP response")?;
    let (head, body) = raw.split_at(header_end);
    let body = body
        .trim_start_matches("\r\n\r\n")
        .trim_start_matches("\n\n");
    let status_line = head.lines().next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .context("HTTP status")?
        .parse()
        .context("HTTP status number")?;
    Ok((status, body.to_string()))
}

/// Load admin client PEMs from a rendered kubeconfig (embedded base64 data).
pub fn credentials_from_kubeconfig(kc: &str) -> Result<(String, String, String)> {
    #[derive(serde::Deserialize)]
    struct Kc {
        clusters: Vec<ClusterEntry>,
        users: Vec<UserEntry>,
    }
    #[derive(serde::Deserialize)]
    struct ClusterEntry {
        cluster: Cluster,
    }
    #[derive(serde::Deserialize)]
    struct Cluster {
        #[serde(rename = "certificate-authority-data")]
        ca_data: String,
    }
    #[derive(serde::Deserialize)]
    struct UserEntry {
        user: User,
    }
    #[derive(serde::Deserialize)]
    struct User {
        #[serde(rename = "client-certificate-data")]
        cert_data: String,
        #[serde(rename = "client-key-data")]
        key_data: String,
    }

    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let parsed: Kc = serde_yaml::from_str(kc).context("parse admin kubeconfig")?;
    let ca = parsed
        .clusters
        .first()
        .context("kubeconfig missing cluster")?;
    let user = parsed.users.first().context("kubeconfig missing user")?;
    let ca_pem = String::from_utf8(B64.decode(ca.cluster.ca_data.trim())?)?;
    let cert_pem = String::from_utf8(B64.decode(user.user.cert_data.trim())?)?;
    let key_pem = String::from_utf8(B64.decode(user.user.key_data.trim())?)?;
    Ok((ca_pem, cert_pem, key_pem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_line() {
        let (s, b) = parse_http_response("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").unwrap();
        assert_eq!(s, 200);
        assert_eq!(b, "ok");
    }
}
