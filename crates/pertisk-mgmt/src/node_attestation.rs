//! Node TPM Quote enroll / verify against a stored AK public (mgmt trust store).

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;
use tokio::process::Command;

use crate::config::Config;
use crate::db;
use crate::error::{ApiResult, AppError};

#[derive(Debug, Serialize)]
pub struct AttestationOut {
    pub enrolled: bool,
    pub ak_enrolled_at: Option<String>,
    /// Truncated fingerprint of stored AK (sha256 hex, 16 chars).
    pub ak_fingerprint: Option<String>,
    pub ok: Option<bool>,
    pub message: String,
}

#[derive(Debug, Default)]
struct QuoteParse {
    available: bool,
    message: String,
    nonce_hex: String,
    quoted_b64: String,
    signature_b64: String,
    ak_public_b64: String,
    pcrs: Vec<(u32, String)>,
}

async fn run_quote(cfg: &Config, ip: &str) -> Result<QuoteParse, String> {
    let out = Command::new(&cfg.pertiskctl)
        .args(["-e", &format!("{ip}:50000"), "quote"])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("pertiskctl quote: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!(
            "pertiskctl quote failed: {} {}",
            stdout.trim(),
            stderr.trim()
        ));
    }
    Ok(parse_quote_output(&stdout))
}

fn parse_quote_output(stdout: &str) -> QuoteParse {
    let mut q = QuoteParse::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("available=") {
            let avail = rest.split_whitespace().next().unwrap_or("");
            q.available = avail == "true";
            if let Some(msg) = rest.find("—").or_else(|| rest.find("--")) {
                // "available=true slot=… — message"
                if let Some(m) = rest.get(msg + "—".len()..) {
                    q.message = m.trim().trim_start_matches('-').trim().to_string();
                }
            }
        } else if let Some(rest) = line.strip_prefix("nonce=") {
            q.nonce_hex = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("quoted_b64=") {
            q.quoted_b64 = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("signature_b64=") {
            q.signature_b64 = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = line.strip_prefix("ak_public_b64=") {
            q.ak_public_b64 = rest.split_whitespace().next().unwrap_or("").to_string();
        } else {
            // PCR table: "0      sha256   deadbeef…"
            let mut parts = line.split_whitespace();
            if let (Some(idx), Some(_algo), Some(digest)) =
                (parts.next(), parts.next(), parts.next())
            {
                if let Ok(i) = idx.parse::<u32>() {
                    if digest.len() >= 32 && digest.chars().all(|c| c.is_ascii_hexdigit()) {
                        q.pcrs.push((i, digest.to_string()));
                    }
                }
            }
        }
    }
    q
}

fn fingerprint_b64(ak_b64: &str) -> Option<String> {
    let raw = B64.decode(ak_b64.trim()).ok()?;
    use sha2::{Digest, Sha256};
    let dig = Sha256::digest(&raw);
    Some(hex::encode(&dig[..8]))
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("hex: {e}"))
        })
        .collect()
}

/// TOFU enroll: fetch Quote, store AK public in SQLite.
pub async fn enroll(cfg: &Config, pool: &sqlx::SqlitePool, node_id: &str, ip: &str) -> ApiResult<AttestationOut> {
    let quote = tokio::time::timeout(Duration::from_secs(60), run_quote(cfg, ip))
        .await
        .map_err(|_| AppError::bad("pertiskctl quote timed out"))?
        .map_err(AppError::bad)?;
    if !quote.available || quote.ak_public_b64.is_empty() {
        return Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ok: Some(false),
            message: if quote.message.is_empty() {
                "Quote unavailable (no TPM / persistent AK?)".into()
            } else {
                quote.message
            },
        });
    }
    let now = db::now_rfc3339();
    sqlx::query(
        "UPDATE nodes SET ak_public_b64 = ?, ak_enrolled_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&quote.ak_public_b64)
    .bind(&now)
    .bind(&now)
    .bind(node_id)
    .execute(pool)
    .await?;

    Ok(AttestationOut {
        enrolled: true,
        ak_enrolled_at: Some(now),
        ak_fingerprint: fingerprint_b64(&quote.ak_public_b64),
        ok: Some(true),
        message: format!(
            "enrolled AK from Quote ({} bytes)",
            B64.decode(&quote.ak_public_b64).map(|b| b.len()).unwrap_or(0)
        ),
    })
}

/// Verify a fresh Quote against the enrolled AK.
pub async fn verify(cfg: &Config, pool: &sqlx::SqlitePool, node_id: &str, ip: &str) -> ApiResult<AttestationOut> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT ak_public_b64, ak_enrolled_at FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    let Some((Some(stored_b64), enrolled_at)) = row else {
        return Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ok: Some(false),
            message: "no AK enrolled — POST …/attestation/enroll first".into(),
        });
    };

    let quote = tokio::time::timeout(Duration::from_secs(60), run_quote(cfg, ip))
        .await
        .map_err(|_| AppError::bad("pertiskctl quote timed out"))?
        .map_err(AppError::bad)?;
    if !quote.available {
        return Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ok: Some(false),
            message: quote.message,
        });
    }

    if quote.ak_public_b64 != stored_b64 {
        return Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ok: Some(false),
            message: "Quote AK does not match enrolled key (re-enroll after AK reset?)".into(),
        });
    }

    let quoted = B64
        .decode(quote.quoted_b64.trim())
        .map_err(|e| AppError::bad(format!("quoted_b64: {e}")))?;
    let signature = B64
        .decode(quote.signature_b64.trim())
        .map_err(|e| AppError::bad(format!("signature_b64: {e}")))?;
    let ak_public = B64
        .decode(stored_b64.trim())
        .map_err(|e| AppError::bad(format!("ak_public_b64: {e}")))?;
    let nonce = hex_decode(&quote.nonce_hex).map_err(AppError::bad)?;
    let pcrs: Vec<pertisk_tpm::PcrDigest> = quote
        .pcrs
        .into_iter()
        .map(|(index, digest_hex)| pertisk_tpm::PcrDigest { index, digest_hex })
        .collect();

    match pertisk_tpm::verify_quote(&quoted, &signature, &ak_public, &nonce, &pcrs) {
        Ok(()) => Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ok: Some(true),
            message: "verify=ok (signature + nonce + PCR digest)".into(),
        }),
        Err(e) => Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ok: Some(false),
            message: format!("verify failed: {e}"),
        }),
    }
}

pub async fn status(pool: &sqlx::SqlitePool, node_id: &str) -> ApiResult<AttestationOut> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT ak_public_b64, ak_enrolled_at FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((Some(ak), at)) if !ak.is_empty() => Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: at,
            ak_fingerprint: fingerprint_b64(&ak),
            ok: None,
            message: "AK enrolled".into(),
        }),
        _ => Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ok: None,
            message: "not enrolled".into(),
        }),
    }
}
