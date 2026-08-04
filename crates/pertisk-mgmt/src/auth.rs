use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::config::Config;
use crate::db;
use crate::error::{ApiResult, AppError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn can_mutate(self) -> bool {
        matches!(self, Role::Admin | Role::Operator)
    }

    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub username: String,
    pub role: Role,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
    pub role: Role,
}

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn issue_token(cfg: &Config, user: &AuthUser) -> ApiResult<String> {
    let now = Utc::now();
    let claims = Claims {
        sub: user.id.clone(),
        username: user.username.clone(),
        role: user.role,
        iat: now.timestamp(),
        exp: (now + Duration::seconds(cfg.jwt_ttl_secs)).timestamp(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&cfg.secret_key),
    )
    .map_err(|e| AppError::Anyhow(anyhow::anyhow!("jwt encode: {e}")))
}

pub fn decode_token(cfg: &Config, token: &str) -> ApiResult<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(&cfg.secret_key),
        &Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| AppError::Unauthorized)
}

pub async fn seed_admin(pool: &SqlitePool, cfg: &Config) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Ok(());
    }
    if !cfg.auth_mode.allows_local() {
        tracing::info!("no local users seeded (AUTH_MODE does not allow local)");
        return Ok(());
    }
    let password = cfg
        .admin_password
        .clone()
        .unwrap_or_else(|| {
            tracing::warn!("MGMT_ADMIN_PASSWORD unset; seeding admin with password 'admin'");
            "admin".into()
        });
    let id = Uuid::new_v4().to_string();
    let hash = hash_password(&password)?;
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?, ?, ?, 'admin', ?)",
    )
    .bind(&id)
    .bind(&cfg.admin_user)
    .bind(&hash)
    .bind(&now)
    .execute(pool)
    .await?;
    tracing::info!(user = %cfg.admin_user, "seeded admin user");
    Ok(())
}

pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> ApiResult<Option<(String, Option<String>, Role)>> {
    let row = sqlx::query_as::<_, (String, Option<String>, String)>(
        "SELECT id, password_hash, role FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, hash, role)| {
        (
            id,
            hash,
            Role::parse(&role).unwrap_or(Role::Viewer),
        )
    }))
}

pub async fn find_or_create_auth0_user(
    pool: &SqlitePool,
    sub: &str,
    username: &str,
    role: Role,
) -> ApiResult<AuthUser> {
    if let Some(row) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, username, role FROM users WHERE auth0_sub = ?",
    )
    .bind(sub)
    .fetch_optional(pool)
    .await?
    {
        return Ok(AuthUser {
            id: row.0,
            username: row.1,
            role: Role::parse(&row.2).unwrap_or(Role::Viewer),
        });
    }
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, auth0_sub, created_at) VALUES (?, ?, NULL, ?, ?, ?)",
    )
    .bind(&id)
    .bind(username)
    .bind(role.as_str())
    .bind(sub)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(AuthUser {
        id,
        username: username.to_string(),
        role,
    })
}

pub async fn audit(
    pool: &SqlitePool,
    user_id: Option<&str>,
    action: &str,
    resource: Option<&str>,
    detail: Option<&str>,
) {
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    let _ = sqlx::query(
        "INSERT INTO audit_log (id, user_id, action, resource, detail, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(action)
    .bind(resource)
    .bind(detail)
    .bind(&now)
    .execute(pool)
    .await;
}
