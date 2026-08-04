use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "static/"]
#[prefix = ""]
pub struct Assets;

pub async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if path.is_empty() || path == "index.html" {
        return serve("index.html");
    }

    if Assets::get(path).is_some() {
        return serve(path);
    }

    // SPA fallback
    serve("index.html")
}

fn serve(path: &str) -> Response {
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(content.data.to_vec()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => {
            // Placeholder when UI not built yet
            let html = r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/><title>Pertisk Mgmt</title>
<style>body{font-family:system-ui;background:#0f1419;color:#e7ecf3;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0}
.card{max-width:32rem;padding:2rem;border:1px solid #2a3544;border-radius:12px;background:#1a2332}
code{background:#0f1419;padding:0.2em 0.4em;border-radius:4px}</style></head>
<body><div class="card"><h1>Pertisk Management</h1>
<p>API is up. Build the UI with <code>make mgmt-ui</code> then <code>make mgmt</code>.</p>
<p><a href="/api/health" style="color:#5b9fd4">/api/health</a></p></div></body></html>"#;
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}
