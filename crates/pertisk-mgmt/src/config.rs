use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;

use anyhow::{bail, Context};

/// Runtime configuration for pertisk-mgmt.
#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub db: PathBuf,
    pub data_dir: PathBuf,
    pub lab_up: PathBuf,
    pub pertiskctl: PathBuf,
    /// local | auth0 | both
    pub auth_mode: AuthMode,
    pub admin_user: String,
    pub admin_password: Option<String>,
    /// 32-byte key as hex (64 chars) or any string (hashed to 32 bytes).
    pub secret_key: Vec<u8>,
    pub jwt_ttl_secs: i64,
    pub auth0_domain: Option<String>,
    pub auth0_client_id: Option<String>,
    pub auth0_client_secret: Option<String>,
    #[allow(dead_code)]
    pub auth0_audience: Option<String>,
    /// Reachable base URL for OIDC + guest serial dashboard (`machine.dashboard.mgmt_url`).
    /// Never `http://0.0.0.0:…` — that is not a client-reachable address.
    pub public_url: String,
    /// Optional Bearer for scraping guest `:50001/metrics`.
    pub metrics_token: Option<String>,
    /// Optional mTLS client material for scraping guest metrics over HTTPS.
    /// When set (all three), scrapes use `https://{ip}:50001/metrics`.
    pub metrics_tls: Option<MetricsTls>,
    /// Directory of prebuilt cloud qcow2 images (lab-up --skip-build).
    pub images_dir: PathBuf,
    /// Optional SMTP for password reset and Auth0 first-login notices.
    pub smtp: Option<SmtpConfig>,
    /// Recipients for Auth0 first-login / admin notices (comma-separated env).
    pub admin_emails: Vec<String>,
}

/// SMTP relay settings (env-only). Absent when host/from are unset.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub from: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls: SmtpTlsMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTlsMode {
    /// Plain SMTP (lab / internal only).
    None,
    /// STARTTLS after connect (typical submission port 587).
    Starttls,
    /// Implicit TLS (typical port 465).
    Tls,
}

impl SmtpTlsMode {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" | "off" | "plain" => Ok(Self::None),
            "starttls" | "opportunistic" => Ok(Self::Starttls),
            "tls" | "ssl" | "wrapper" => Ok(Self::Tls),
            other => bail!("invalid MGMT_SMTP_TLS={other}; use none|starttls|tls"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Starttls => "starttls",
            Self::Tls => "tls",
        }
    }
}

/// Client PEMs for scraping guest metrics when nodes enable metrics mTLS.
#[derive(Debug, Clone)]
pub struct MetricsTls {
    pub ca_cert: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Local,
    Auth0,
    Both,
}

impl AuthMode {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "auth0" => Ok(Self::Auth0),
            "both" => Ok(Self::Both),
            other => bail!("invalid AUTH_MODE={other}; use local|auth0|both"),
        }
    }

    pub fn allows_local(self) -> bool {
        matches!(self, Self::Local | Self::Both)
    }

    pub fn allows_auth0(self) -> bool {
        matches!(self, Self::Auth0 | Self::Both)
    }
}

impl Config {
    pub fn from_env(
        listen: SocketAddr,
        db: PathBuf,
        data_dir: PathBuf,
        lab_up: PathBuf,
        pertiskctl: PathBuf,
    ) -> anyhow::Result<Self> {
        let auth_mode =
            AuthMode::parse(&std::env::var("AUTH_MODE").unwrap_or_else(|_| "local".into()))?;
        let admin_user = std::env::var("MGMT_ADMIN_USER").unwrap_or_else(|_| "admin".into());
        let admin_password = std::env::var("MGMT_ADMIN_PASSWORD").ok();
        let secret_raw = std::env::var("MGMT_SECRET_KEY").unwrap_or_else(|_| {
            tracing::warn!("MGMT_SECRET_KEY unset; using insecure lab default");
            "pertisk-mgmt-dev-secret-change-me".into()
        });
        let secret_key = derive_key(&secret_raw);
        let jwt_ttl_secs = std::env::var("MGMT_JWT_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(86400);
        let public_url = resolve_public_url(listen);
        let metrics_token = std::env::var("MGMT_METRICS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        let metrics_tls = resolve_metrics_tls()?;
        let images_dir = std::env::var("PERTISK_IMAGES_DIR")
            .or_else(|_| std::env::var("MGMT_IMAGES_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("images"));

        let auth0_domain = std::env::var("AUTH0_DOMAIN").ok().filter(|s| !s.is_empty());
        let auth0_client_id = std::env::var("AUTH0_CLIENT_ID")
            .ok()
            .filter(|s| !s.is_empty());
        let auth0_client_secret = std::env::var("AUTH0_CLIENT_SECRET")
            .ok()
            .filter(|s| !s.is_empty());
        let auth0_audience = std::env::var("AUTH0_AUDIENCE")
            .ok()
            .filter(|s| !s.is_empty());

        if auth_mode.allows_auth0()
            && (auth0_domain.is_none()
                || auth0_client_id.is_none()
                || auth0_client_secret.is_none())
        {
            bail!("AUTH_MODE requires AUTH0_DOMAIN, AUTH0_CLIENT_ID, AUTH0_CLIENT_SECRET");
        }

        let smtp = resolve_smtp()?;
        let admin_emails = std::env::var("MGMT_ADMIN_EMAILS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            listen,
            db,
            data_dir,
            lab_up,
            pertiskctl,
            auth_mode,
            admin_user,
            admin_password,
            secret_key,
            jwt_ttl_secs,
            auth0_domain,
            auth0_client_id,
            auth0_client_secret,
            auth0_audience,
            public_url,
            metrics_token,
            metrics_tls,
            images_dir,
            smtp,
            admin_emails,
        })
    }

    pub fn smtp_configured(&self) -> bool {
        self.smtp.is_some()
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    pub fn kubeconfigs_dir(&self) -> PathBuf {
        self.data_dir.join("kubeconfigs")
    }

    pub fn os_bundles_dir(&self) -> PathBuf {
        self.data_dir.join("os-bundles")
    }

    /// Catalog of signed OS A/B packages (version + arch), reused across clusters.
    pub fn os_packages_dir(&self) -> PathBuf {
        self.data_dir.join("os-packages")
    }

    /// Public OS trust key used to seed `STATE/secrets/os-trust.pk` when missing.
    pub fn os_trust_pk(&self) -> PathBuf {
        std::env::var("MGMT_OS_TRUST_PK")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("secrets/os-trust.pk"))
    }
}

fn derive_key(raw: &str) -> Vec<u8> {
    if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(raw).unwrap_or_else(|_| hash_key(raw));
    }
    hash_key(raw)
}

fn hash_key(raw: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    h.finalize().to_vec()
}

fn resolve_smtp() -> anyhow::Result<Option<SmtpConfig>> {
    let host = std::env::var("MGMT_SMTP_HOST")
        .ok()
        .filter(|s| !s.is_empty());
    let from = std::env::var("MGMT_SMTP_FROM")
        .or_else(|_| std::env::var("MGMT_SMTP_SENDER"))
        .ok()
        .filter(|s| !s.is_empty());
    match (host, from) {
        (None, None) => Ok(None),
        (Some(host), Some(from)) => {
            let port = std::env::var("MGMT_SMTP_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(587);
            let tls = SmtpTlsMode::parse(
                &std::env::var("MGMT_SMTP_TLS").unwrap_or_else(|_| "starttls".into()),
            )?;
            let username = std::env::var("MGMT_SMTP_USER")
                .or_else(|_| std::env::var("MGMT_SMTP_USERNAME"))
                .ok()
                .filter(|s| !s.is_empty());
            let password = std::env::var("MGMT_SMTP_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty());
            if username.is_some() != password.is_some() {
                bail!(
                    "incomplete SMTP auth; set both MGMT_SMTP_USER (or MGMT_SMTP_USERNAME) and MGMT_SMTP_PASSWORD"
                );
            }
            if username.is_none() {
                tracing::warn!(
                    host = %host,
                    "SMTP configured without credentials; many relays (e.g. Gmail) will reject sends"
                );
            }
            Ok(Some(SmtpConfig {
                host,
                port,
                from,
                username,
                password,
                tls,
            }))
        }
        _ => bail!(
            "incomplete SMTP env; set both MGMT_SMTP_HOST and MGMT_SMTP_FROM (or MGMT_SMTP_SENDER)"
        ),
    }
}

fn resolve_metrics_tls() -> anyhow::Result<Option<MetricsTls>> {
    let ca = std::env::var("MGMT_METRICS_TLS_CA")
        .ok()
        .filter(|s| !s.is_empty());
    let cert = std::env::var("MGMT_METRICS_TLS_CERT")
        .ok()
        .filter(|s| !s.is_empty());
    let key = std::env::var("MGMT_METRICS_TLS_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    match (ca, cert, key) {
        (Some(ca), Some(cert), Some(key)) => {
            let tls = MetricsTls {
                ca_cert: PathBuf::from(ca),
                client_cert: PathBuf::from(cert),
                client_key: PathBuf::from(key),
            };
            for (label, path) in [
                ("MGMT_METRICS_TLS_CA", &tls.ca_cert),
                ("MGMT_METRICS_TLS_CERT", &tls.client_cert),
                ("MGMT_METRICS_TLS_KEY", &tls.client_key),
            ] {
                if !path.is_file() {
                    bail!("{label}={} is not a readable file", path.display());
                }
            }
            Ok(Some(tls))
        }
        (None, None, None) => Ok(None),
        _ => bail!(
            "incomplete metrics TLS env; set all of MGMT_METRICS_TLS_CA, MGMT_METRICS_TLS_CERT, MGMT_METRICS_TLS_KEY"
        ),
    }
}

/// Prefer explicit `MGMT_PUBLIC_URL`. Never default to `http://0.0.0.0:…`
/// (listen wildcard is not reachable from guests / browsers).
fn resolve_public_url(listen: SocketAddr) -> String {
    if let Ok(raw) = std::env::var("MGMT_PUBLIC_URL") {
        let trimmed = raw.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            if public_url_host_unusable(&trimmed) {
                tracing::warn!(
                    url = %trimmed,
                    "MGMT_PUBLIC_URL host is not client-reachable; detecting LAN IP instead"
                );
            } else {
                return trimmed;
            }
        }
    }

    let port = listen.port();
    match listen.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => {
            return format!("http://{ip}:{port}");
        }
        IpAddr::V6(ip) if !ip.is_unspecified() && !ip.is_loopback() => {
            return format!("http://[{ip}]:{port}");
        }
        _ => {}
    }

    if let Some(ip) = detect_primary_ipv4() {
        let url = format!("http://{ip}:{port}");
        tracing::info!(
            %url,
            "MGMT_PUBLIC_URL unset; using detected LAN address for guest dashboard / OIDC"
        );
        return url;
    }

    tracing::warn!(
        "MGMT_PUBLIC_URL unset and no LAN IP detected — guest serial will not show a mgmt link. \
         Set MGMT_PUBLIC_URL=http://<mgmt-lan-ip>:{port} in /etc/pertisk-mgmt/pertisk-mgmt.env"
    );
    String::new()
}

/// True when the URL host cannot be used by guests (wildcard / empty).
pub fn public_url_host_unusable(url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return true;
    };
    matches!(host.as_str(), "" | "0.0.0.0" | "::" | "[::]")
}

fn url_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or("");
    if hostport.is_empty() {
        return None;
    }
    if let Some(h) = hostport.strip_prefix('[') {
        // [v6]:port
        return h.split(']').next().map(|s| s.to_string());
    }
    Some(hostport.split(':').next().unwrap_or(hostport).to_string())
}

/// Best-effort primary IPv4 via UDP "connect" (no packets sent).
fn detect_primary_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

#[allow(dead_code)]
pub fn require_nonempty(name: &str, v: Option<String>) -> anyhow::Result<String> {
    v.filter(|s| !s.is_empty())
        .with_context(|| format!("{name} required"))
}
