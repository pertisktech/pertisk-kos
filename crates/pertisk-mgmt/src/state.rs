use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub cfg: Config,
    pub pool: SqlitePool,
    pub job_notify: Notify,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(cfg: Config, pool: SqlitePool) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("http client");
        Self {
            inner: Arc::new(Inner {
                cfg,
                pool,
                job_notify: Notify::new(),
                http,
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
        self.inner.job_notify.notify_one();
    }
}
