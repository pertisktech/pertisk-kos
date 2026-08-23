use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::config::{Config, MetricsTls};
use crate::events::EventBus;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub cfg: Config,
    pub pool: SqlitePool,
    pub job_notify: Notify,
    pub events: EventBus,
    pub http: reqwest::Client,
    /// Dedicated client for guest `:50001/metrics` (HTTP or mTLS HTTPS).
    pub metrics_http: reqwest::Client,
}

impl AppState {
    pub fn new(cfg: Config, pool: SqlitePool) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("http client");
        let metrics_http = if cfg.metrics_tls.is_some() {
            build_metrics_http_client(cfg.metrics_tls.as_ref()).expect("metrics mTLS HTTP client")
        } else {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("metrics http client")
        };
        Self {
            inner: Arc::new(Inner {
                cfg,
                pool,
                job_notify: Notify::new(),
                events: EventBus::new(),
                http,
                metrics_http,
            }),
        }
    }

    pub fn cfg(&self) -> &Config {
        &self.inner.cfg
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.inner.pool
    }

    pub fn notify_jobs(&self) {
        self.inner.job_notify.notify_waiters();
    }

    pub fn emit_job(
        &self,
        cluster_id: Option<&str>,
        job_id: &str,
        job_kind: Option<&str>,
        status: &str,
    ) {
        self.inner.events.job(cluster_id, job_id, job_kind, status);
    }

    pub fn emit_cluster(&self, cluster_id: &str, status: &str) {
        self.inner.events.cluster(cluster_id, status);
    }
}

fn build_metrics_http_client(tls: Option<&MetricsTls>) -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60));
    let Some(tls) = tls else {
        return Ok(b.build()?);
    };

    let ca_pem = std::fs::read(&tls.ca_cert)?;
    let cert_pem = std::fs::read(&tls.client_cert)?;
    let key_pem = std::fs::read(&tls.client_key)?;

    let ca = reqwest::Certificate::from_pem(&ca_pem)?;
    let identity = reqwest::Identity::from_pkcs8_pem(&cert_pem, &key_pem)?;

    // Guests are scraped by IP; lab server certs typically lack those SANs.
    b = b
        .add_root_certificate(ca)
        .identity(identity)
        .danger_accept_invalid_hostnames(true);

    Ok(b.build()?)
}
