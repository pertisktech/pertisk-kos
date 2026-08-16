//! Outbound SMTP for password resets and Auth0 first-login notices.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sha2::{Digest, Sha256};

use crate::config::{Config, SmtpConfig, SmtpTlsMode};

pub const RESET_TOKEN_TTL_SECS: i64 = 3600;

/// SHA-256 hex of a raw reset token (stored in DB).
pub fn hash_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}

pub fn mail_enabled(cfg: &Config) -> bool {
    cfg.smtp.is_some()
}

/// Fire-and-forget send; logs errors and never panics the caller.
pub fn spawn_send(cfg: &Config, to: Vec<String>, subject: String, body: String) {
    let Some(smtp) = cfg.smtp.clone() else {
        tracing::debug!(%subject, "smtp not configured; skipping email");
        return;
    };
    if to.is_empty() {
        tracing::warn!(%subject, "no email recipients; skipping");
        return;
    }
    tokio::spawn(async move {
        match tokio::time::timeout(
            Duration::from_secs(15),
            send_email(&smtp, &to, &subject, &body),
        )
        .await
        {
            Ok(Ok(())) => tracing::info!(%subject, recipients = to.len(), "email sent"),
            Ok(Err(e)) => tracing::error!(error = %e, %subject, "email send failed"),
            Err(_) => tracing::error!(%subject, "email send timed out"),
        }
    });
}

pub async fn send_email(
    smtp: &SmtpConfig,
    to: &[String],
    subject: &str,
    body: &str,
) -> anyhow::Result<()> {
    let from: Mailbox = smtp
        .from
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid MGMT_SMTP_FROM: {e}"))?;
    let mut builder = Message::builder().from(from).subject(subject);
    for addr in to {
        let mb: Mailbox = addr
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid recipient {addr}: {e}"))?;
        builder = builder.to(mb);
    }
    let message = builder.body(body.to_string())?;

    let transport = build_transport(smtp)?;
    transport
        .send(message)
        .await
        .map_err(|e| anyhow::anyhow!("smtp send: {e}"))?;
    Ok(())
}

fn build_transport(
    smtp: &SmtpConfig,
) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
    let mut builder = match smtp.tls {
        SmtpTlsMode::Tls => {
            let tls = TlsParameters::new(smtp.host.clone())
                .map_err(|e| anyhow::anyhow!("smtp tls params: {e}"))?;
            AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)?
                .port(smtp.port)
                .tls(Tls::Wrapper(tls))
        }
        SmtpTlsMode::Starttls => {
            let tls = TlsParameters::new(smtp.host.clone())
                .map_err(|e| anyhow::anyhow!("smtp tls params: {e}"))?;
            AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)?
                .port(smtp.port)
                .tls(Tls::Required(tls))
        }
        SmtpTlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host)
            .port(smtp.port)
            .tls(Tls::None),
    };

    if let (Some(user), Some(pass)) = (&smtp.username, &smtp.password) {
        builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
    }

    Ok(builder.build())
}

pub fn password_reset_email(cfg: &Config, username: &str, raw_token: &str) -> (String, String) {
    let base = cfg.public_url.trim_end_matches('/');
    let link = format!("{base}/#/reset-password?token={raw_token}");
    let subject = "Pertisk KOS password reset".to_string();
    let body = format!(
        "Hello {username},\n\n\
A password reset was requested for your Pertisk KOS account.\n\n\
Reset link (valid for one hour, single use):\n{link}\n\n\
If you did not request this, you can ignore this email.\n"
    );
    (subject, body)
}

pub fn auth0_new_user_email(cfg: &Config, username: &str, role: &str, user_id: &str) -> (String, String) {
    let subject = "Pertisk KOS: new Auth0 user signed in".to_string();
    let body = format!(
        "A new Auth0 identity signed into Pertisk KOS for the first time.\n\n\
Username: {username}\n\
Role (from Auth0 claim): {role}\n\
User id: {user_id}\n\
Public URL: {}\n\n\
Access is controlled by Auth0 configuration and claims — Pertisk does not require local approval.\n",
        cfg.public_url
    );
    (subject, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, Config};
    use std::net::SocketAddr;
    use std::path::PathBuf;

    fn base_cfg() -> Config {
        Config {
            listen: "127.0.0.1:8080".parse::<SocketAddr>().unwrap(),
            db: PathBuf::from("./data/mgmt.db"),
            data_dir: PathBuf::from("./data"),
            lab_up: PathBuf::from("./scripts/proxmox-lab-up.sh"),
            pertiskctl: PathBuf::from("./out/bin/pertiskctl"),
            auth_mode: AuthMode::Local,
            admin_user: "admin".into(),
            admin_password: None,
            secret_key: vec![0u8; 32],
            jwt_ttl_secs: 3600,
            auth0_domain: None,
            auth0_client_id: None,
            auth0_client_secret: None,
            auth0_audience: None,
            public_url: "http://127.0.0.1:8080".into(),
            metrics_token: None,
            metrics_tls: None,
            images_dir: PathBuf::from("./data/images"),
            smtp: None,
            admin_emails: vec![],
        }
    }

    #[test]
    fn mail_disabled_without_smtp() {
        let cfg = base_cfg();
        assert!(!mail_enabled(&cfg));
    }

    #[test]
    fn hash_token_stable() {
        assert_eq!(hash_token("abc"), hash_token("abc"));
        assert_ne!(hash_token("abc"), hash_token("abd"));
    }

    #[test]
    fn reset_email_contains_token_link() {
        let cfg = base_cfg();
        let (subj, body) = password_reset_email(&cfg, "alice", "tok123");
        assert!(subj.contains("password reset"));
        assert!(body.contains("/#/reset-password?token=tok123"));
        assert!(body.contains("alice"));
    }
}
