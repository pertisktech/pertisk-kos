//! Node TPM Quote enroll / verify against a stored AK public (mgmt trust store).
//!
//! Also records the manufacturer EK certificate fingerprint when the Quote
//! includes one (TCG NV). Full CA chain verify runs on the node when
//! `PERTISK_TPM_EK_CAS` is set; mgmt stores the fingerprint for TOFU match.

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
    /// Truncated EK cert fingerprint when enrolled (sha256 hex, 16 chars).
    pub ek_fingerprint: Option<String>,
    pub ek_chain_status: Option<String>,
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
    ek_fingerprint: String,
    ek_chain_status: String,
    ek_chain_message: String,
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
        } else if let Some(rest) = line.strip_prefix("ek_nv=") {
            // ek_nv=0x… chain=ok fingerprint=deadbeef…
            for part in rest.split_whitespace() {
                if let Some(v) = part.strip_prefix("chain=") {
                    q.ek_chain_status = v.to_string();
                } else if let Some(v) = part.strip_prefix("fingerprint=") {
                    if v != "—" {
                        q.ek_fingerprint = v.to_string();
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("ek_chain_message=") {
            q.ek_chain_message = rest.trim().to_string();
        } else {
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

fn ek_fp_short(full: &str) -> Option<String> {
    let s = full.trim();
    if s.is_empty() || s == "—" {
        return None;
    }
    Some(s.chars().take(16).collect())
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("hex: {e}")))
        .collect()
}

/// TOFU enroll: fetch Quote, store AK public (+ EK fingerprint when present).
pub async fn enroll(
    cfg: &Config,
    pool: &sqlx::SqlitePool,
    node_id: &str,
    ip: &str,
) -> ApiResult<AttestationOut> {
    let quote = tokio::time::timeout(Duration::from_secs(60), run_quote(cfg, ip))
        .await
        .map_err(|_| AppError::bad("pertiskctl quote timed out"))?
        .map_err(AppError::bad)?;
    if !quote.available || quote.ak_public_b64.is_empty() {
        return Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ek_fingerprint: None,
            ek_chain_status: None,
            ok: Some(false),
            message: if quote.message.is_empty() {
                "Quote unavailable (no TPM / persistent AK?)".into()
            } else {
                quote.message
            },
        });
    }
    let now = db::now_rfc3339();
    let ek_fp = if quote.ek_fingerprint.is_empty() {
        None
    } else {
        Some(quote.ek_fingerprint.clone())
    };
    sqlx::query(
        "UPDATE nodes SET ak_public_b64 = ?, ak_enrolled_at = ?, ek_fingerprint = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&quote.ak_public_b64)
    .bind(&now)
    .bind(&ek_fp)
    .bind(&now)
    .bind(node_id)
    .execute(pool)
    .await?;

    let mut message = format!(
        "enrolled AK from Quote ({} bytes)",
        B64.decode(&quote.ak_public_b64)
            .map(|b| b.len())
            .unwrap_or(0)
    );
    if let Some(ref fp) = ek_fp {
        message.push_str(&format!(
            "; EK fingerprint {}… (chain={})",
            fp.chars().take(16).collect::<String>(),
            if quote.ek_chain_status.is_empty() {
                "—"
            } else {
                &quote.ek_chain_status
            }
        ));
    }

    Ok(AttestationOut {
        enrolled: true,
        ak_enrolled_at: Some(now),
        ak_fingerprint: fingerprint_b64(&quote.ak_public_b64),
        ek_fingerprint: ek_fp_short(ek_fp.as_deref().unwrap_or("")),
        ek_chain_status: if quote.ek_chain_status.is_empty() {
            None
        } else {
            Some(quote.ek_chain_status)
        },
        ok: Some(true),
        message,
    })
}

/// Verify a fresh Quote against the enrolled AK (+ EK fingerprint when stored).
pub async fn verify(
    cfg: &Config,
    pool: &sqlx::SqlitePool,
    node_id: &str,
    ip: &str,
) -> ApiResult<AttestationOut> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT ak_public_b64, ak_enrolled_at, ek_fingerprint FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    let Some((Some(stored_b64), enrolled_at, stored_ek)) = row else {
        return Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ek_fingerprint: None,
            ek_chain_status: None,
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
            ek_fingerprint: stored_ek.as_deref().and_then(ek_fp_short),
            ek_chain_status: None,
            ok: Some(false),
            message: quote.message,
        });
    }

    if quote.ak_public_b64 != stored_b64 {
        return Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ek_fingerprint: stored_ek.as_deref().and_then(ek_fp_short),
            ek_chain_status: if quote.ek_chain_status.is_empty() {
                None
            } else {
                Some(quote.ek_chain_status)
            },
            ok: Some(false),
            message: "Quote AK does not match enrolled key (re-enroll after AK reset?)".into(),
        });
    }

    if let Some(ref want_ek) = stored_ek {
        if !want_ek.is_empty() {
            if quote.ek_fingerprint.is_empty() {
                return Ok(AttestationOut {
                    enrolled: true,
                    ak_enrolled_at: enrolled_at,
                    ak_fingerprint: fingerprint_b64(&stored_b64),
                    ek_fingerprint: ek_fp_short(want_ek),
                    ek_chain_status: None,
                    ok: Some(false),
                    message: "enrolled EK fingerprint present but Quote has no EK cert".into(),
                });
            }
            if quote.ek_fingerprint != *want_ek {
                return Ok(AttestationOut {
                    enrolled: true,
                    ak_enrolled_at: enrolled_at,
                    ak_fingerprint: fingerprint_b64(&stored_b64),
                    ek_fingerprint: ek_fp_short(want_ek),
                    ek_chain_status: Some(quote.ek_chain_status),
                    ok: Some(false),
                    message: "Quote EK fingerprint does not match enrolled EK".into(),
                });
            }
        }
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
        Ok(()) => {
            let mut message = "verify=ok (signature + nonce + PCR digest)".to_string();
            if !quote.ek_fingerprint.is_empty() {
                message.push_str(&format!(
                    "; EK ok chain={}",
                    if quote.ek_chain_status.is_empty() {
                        "—"
                    } else {
                        &quote.ek_chain_status
                    }
                ));
            }
            Ok(AttestationOut {
                enrolled: true,
                ak_enrolled_at: enrolled_at,
                ak_fingerprint: fingerprint_b64(&stored_b64),
                ek_fingerprint: ek_fp_short(&quote.ek_fingerprint)
                    .or_else(|| stored_ek.as_deref().and_then(ek_fp_short)),
                ek_chain_status: if quote.ek_chain_status.is_empty() {
                    None
                } else {
                    Some(quote.ek_chain_status)
                },
                ok: Some(true),
                message,
            })
        }
        Err(e) => Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: enrolled_at,
            ak_fingerprint: fingerprint_b64(&stored_b64),
            ek_fingerprint: stored_ek.as_deref().and_then(ek_fp_short),
            ek_chain_status: if quote.ek_chain_status.is_empty() {
                None
            } else {
                Some(quote.ek_chain_status)
            },
            ok: Some(false),
            message: format!("verify failed: {e}"),
        }),
    }
}

pub async fn status(pool: &sqlx::SqlitePool, node_id: &str) -> ApiResult<AttestationOut> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT ak_public_b64, ak_enrolled_at, ek_fingerprint FROM nodes WHERE id = ?",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((Some(ak), at, ek)) if !ak.is_empty() => Ok(AttestationOut {
            enrolled: true,
            ak_enrolled_at: at,
            ak_fingerprint: fingerprint_b64(&ak),
            ek_fingerprint: ek.as_deref().and_then(ek_fp_short),
            ek_chain_status: None,
            ok: None,
            message: if ek.as_deref().is_some_and(|e| !e.is_empty()) {
                "AK + EK enrolled".into()
            } else {
                "AK enrolled".into()
            },
        }),
        _ => Ok(AttestationOut {
            enrolled: false,
            ak_enrolled_at: None,
            ak_fingerprint: None,
            ek_fingerprint: None,
            ek_chain_status: None,
            ok: None,
            message: "not enrolled".into(),
        }),
    }
}
