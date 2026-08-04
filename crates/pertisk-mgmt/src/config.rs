use std::net::SocketAddr;
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
    pub public_url: String,
    /// Optional Bearer for scraping guest `:50001/metrics`.
    pub metrics_token: Option<String>,
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
        let auth_mode = AuthMode::parse(
            &std::env::var("AUTH_MODE").unwrap_or_else(|_| "local".into()),
        )?;
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
        let public_url = std::env::var("MGMT_PUBLIC_URL")
            .unwrap_or_else(|_| format!("http://{}", listen));
        let metrics_token = std::env::var("MGMT_METRICS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        let auth0_domain = std::env::var("AUTH0_DOMAIN").ok().filter(|s| !s.is_empty());
        let auth0_client_id = std::env::var("AUTH0_CLIENT_ID").ok().filter(|s| !s.is_empty());
        let auth0_client_secret =
            std::env::var("AUTH0_CLIENT_SECRET").ok().filter(|s| !s.is_empty());
        let auth0_audience = std::env::var("AUTH0_AUDIENCE").ok().filter(|s| !s.is_empty());

        if auth_mode.allows_auth0()
            && (auth0_domain.is_none() || auth0_client_id.is_none() || auth0_client_secret.is_none())
        {
            bail!("AUTH_MODE requires AUTH0_DOMAIN, AUTH0_CLIENT_ID, AUTH0_CLIENT_SECRET");
        }

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
        })
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    pub fn kubeconfigs_dir(&self) -> PathBuf {
        self.data_dir.join("kubeconfigs")
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

#[allow(dead_code)]
pub fn require_nonempty(name: &str, v: Option<String>) -> anyhow::Result<String> {
    v.filter(|s| !s.is_empty())
        .with_context(|| format!("{name} required"))
}
