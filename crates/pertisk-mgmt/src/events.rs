//! Dashboard / UI event bus (SSE fan-out).

use serde::Serialize;
use tokio::sync::broadcast;

/// Max buffered events per subscriber lag window.
const CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize)]
pub struct MgmtEvent {
    /// `job` | `cluster` | `hello`
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub ts: i64,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<MgmtEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MgmtEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, event: MgmtEvent) {
        // No subscribers is fine (UI not open).
        let _ = self.tx.send(event);
    }

    pub fn job(
        &self,
        cluster_id: Option<&str>,
        job_id: &str,
        job_kind: Option<&str>,
        status: &str,
    ) {
        self.publish(MgmtEvent {
            kind: "job".into(),
            cluster_id: cluster_id.map(str::to_string),
            job_id: Some(job_id.to_string()),
            job_kind: job_kind.map(str::to_string),
            status: Some(status.to_string()),
            ts: chrono::Utc::now().timestamp(),
        });
    }

    pub fn cluster(&self, cluster_id: &str, status: &str) {
        self.publish(MgmtEvent {
            kind: "cluster".into(),
            cluster_id: Some(cluster_id.to_string()),
            job_id: None,
            job_kind: None,
            status: Some(status.to_string()),
            ts: chrono::Utc::now().timestamp(),
        });
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
