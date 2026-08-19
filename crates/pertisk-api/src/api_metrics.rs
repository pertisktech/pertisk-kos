//! Process-wide Machine API request counters for Prometheus exposition.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http::Request;
use tower::{Layer, Service};

static DURATION_SUM_NS: AtomicU64 = AtomicU64::new(0);
static DURATION_COUNT: AtomicU64 = AtomicU64::new(0);
static BY_METHOD: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Record one completed Machine API RPC.
pub fn record(method: &str, elapsed: Duration) {
    let ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
    DURATION_SUM_NS.fetch_add(ns, Ordering::Relaxed);
    DURATION_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut map) = BY_METHOD.lock() {
        *map.entry(method.to_string()).or_insert(0) += 1;
    }
}

/// Snapshot for Prometheus text rendering.
pub fn snapshot() -> ApiMetricsSnapshot {
    let by_method = BY_METHOD.lock().map(|m| m.clone()).unwrap_or_default();
    ApiMetricsSnapshot {
        by_method,
        duration_sum_seconds: DURATION_SUM_NS.load(Ordering::Relaxed) as f64 / 1e9,
        duration_count: DURATION_COUNT.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Clone)]
pub struct ApiMetricsSnapshot {
    pub by_method: HashMap<String, u64>,
    pub duration_sum_seconds: f64,
    pub duration_count: u64,
}

impl ApiMetricsSnapshot {
    /// Append Prometheus counter lines (no trailing blank).
    pub fn render_prometheus(&self, out: &mut String) {
        out.push_str(
            "# HELP pertisk_api_requests_total Total Machine API gRPC requests\n\
             # TYPE pertisk_api_requests_total counter\n",
        );
        if self.by_method.is_empty() {
            out.push_str("pertisk_api_requests_total{method=\"\"} 0\n");
        } else {
            let mut keys: Vec<_> = self.by_method.keys().cloned().collect();
            keys.sort();
            for method in keys {
                let n = self.by_method.get(&method).copied().unwrap_or(0);
                let safe = sanitize_label(&method);
                out.push_str(&format!(
                    "pertisk_api_requests_total{{method=\"{safe}\"}} {n}\n"
                ));
            }
        }
        out.push_str(
            "# HELP pertisk_api_request_duration_seconds_sum Cumulative Machine API RPC duration\n\
             # TYPE pertisk_api_request_duration_seconds_sum counter\n",
        );
        out.push_str(&format!(
            "pertisk_api_request_duration_seconds_sum {}\n",
            self.duration_sum_seconds
        ));
        out.push_str(
            "# HELP pertisk_api_request_duration_seconds_count Machine API RPCs timed\n\
             # TYPE pertisk_api_request_duration_seconds_count counter\n",
        );
        out.push_str(&format!(
            "pertisk_api_request_duration_seconds_count {}\n",
            self.duration_count
        ));
    }
}

fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' | '\\' | '\n' => '_',
            c => c,
        })
        .collect()
}

/// Extract RPC method name from a gRPC HTTP/2 path (`/pkg.Service/Method`).
pub fn rpc_method_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

/// Tower layer that times every inbound gRPC request.
#[derive(Clone, Default)]
pub struct ApiMetricsLayer;

impl<S> Layer<S> for ApiMetricsLayer {
    type Service = ApiMetricsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ApiMetricsService { inner }
    }
}

#[derive(Clone)]
pub struct ApiMetricsService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for ApiMetricsService<S>
where
    S: Service<Request<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let method = rpc_method_from_path(req.uri().path());
        let start = Instant::now();
        // Clone so we don't hold &mut self across the async boundary (tower clone pattern).
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let result = inner.call(req).await;
            record(&method, start.elapsed());
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_method_name() {
        assert_eq!(
            rpc_method_from_path("/pertisk.machine.v1alpha1.MachineService/Health"),
            "Health"
        );
        assert_eq!(rpc_method_from_path("/Foo"), "Foo");
    }

    #[test]
    fn renders_counters() {
        record("Health", Duration::from_millis(5));
        let snap = snapshot();
        let mut out = String::new();
        snap.render_prometheus(&mut out);
        assert!(out.contains("pertisk_api_requests_total{method=\"Health\"}"));
        assert!(out.contains("pertisk_api_request_duration_seconds_sum"));
        assert!(out.contains("pertisk_api_request_duration_seconds_count"));
    }
}
