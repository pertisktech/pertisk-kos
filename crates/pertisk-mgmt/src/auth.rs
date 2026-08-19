use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::config::Config;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::mail::{self, RESET_TOKEN_TTL_SECS};

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
    /// `local` or `auth0` — used so Sign out can end the Auth0 SSO session.
    #[serde(default = "default_auth_provider")]
    pub provider: String,
    pub exp: i64,
    pub iat: i64,
}

fn default_auth_provider() -> String {
    "local".into()
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub provider: String,
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub role: Role,
    pub password_hash: Option<String>,
    pub auth0_sub: Option<String>,
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: Option<String>,
}

impl UserRecord {
    pub fn source(&self) -> &'static str {
        if self.auth0_sub.is_some() && self.password_hash.is_none() {
            "auth0"
        } else if self.auth0_sub.is_some() {
            "both"
        } else {
            "local"
        }
    }

    pub fn is_local(&self) -> bool {
        self.auth0_sub.is_none() || self.password_hash.is_some()
    }

    pub fn is_auth0_only(&self) -> bool {
        self.auth0_sub.is_some() && self.password_hash.is_none()
    }
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
        provider: if user.provider.is_empty() {
            "local".into()
        } else {
            user.provider.clone()
        },
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
    let password = cfg.admin_password.clone().unwrap_or_else(|| {
        tracing::warn!("MGMT_ADMIN_PASSWORD unset; seeding admin with password 'admin'");
        "admin".into()
    });
    let id = Uuid::new_v4().to_string();
    let hash = hash_password(&password)?;
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, email, disabled, created_at, updated_at) \
         VALUES (?, ?, ?, 'admin', NULL, 0, ?, ?)",
    )
    .bind(&id)
    .bind(&cfg.admin_user)
    .bind(&hash)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    tracing::info!(user = %cfg.admin_user, "seeded admin user");
    Ok(())
}

pub async fn find_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> ApiResult<Option<(String, Option<String>, Role, bool)>> {
    let row = sqlx::query_as::<_, (String, Option<String>, String, i64)>(
        "SELECT id, password_hash, role, COALESCE(disabled, 0) FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, hash, role, disabled)| {
        (
            id,
            hash,
            Role::parse(&role).unwrap_or(Role::Viewer),
            disabled != 0,
        )
    }))
}

pub async fn get_user(pool: &SqlitePool, id: &str) -> ApiResult<Option<UserRecord>> {
    let row = sqlx::query_as::<_, (
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        i64,
        String,
        Option<String>,
    )>(
        "SELECT id, username, email, role, password_hash, auth0_sub, COALESCE(disabled, 0), created_at, updated_at \
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(
            id,
            username,
            email,
            role,
            password_hash,
            auth0_sub,
            disabled,
            created_at,
            updated_at,
        )| {
            UserRecord {
                id,
                username,
                email,
                role: Role::parse(&role).unwrap_or(Role::Viewer),
                password_hash,
                auth0_sub,
                disabled: disabled != 0,
                created_at,
                updated_at,
            }
        },
    ))
}

pub async fn list_users(pool: &SqlitePool) -> ApiResult<Vec<UserRecord>> {
    let rows = sqlx::query_as::<_, (
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        i64,
        String,
        Option<String>,
    )>(
        "SELECT id, username, email, role, password_hash, auth0_sub, COALESCE(disabled, 0), created_at, updated_at \
         FROM users ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                username,
                email,
                role,
                password_hash,
                auth0_sub,
                disabled,
                created_at,
                updated_at,
            )| {
                UserRecord {
                    id,
                    username,
                    email,
                    role: Role::parse(&role).unwrap_or(Role::Viewer),
                    password_hash,
                    auth0_sub,
                    disabled: disabled != 0,
                    created_at,
                    updated_at,
                }
            },
        )
        .collect())
}

/// Returns `(user, newly_created)`.
pub async fn find_or_create_auth0_user(
    pool: &SqlitePool,
    sub: &str,
    username: &str,
    email: Option<&str>,
    role: Role,
) -> ApiResult<(AuthUser, bool)> {
    if let Some(row) = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT id, username, role, COALESCE(disabled, 0) FROM users WHERE auth0_sub = ?",
    )
    .bind(sub)
    .fetch_optional(pool)
    .await?
    {
        if row.3 != 0 {
            return Err(AppError::Unauthorized);
        }
        return Ok((
            AuthUser {
                id: row.0,
                username: row.1,
                role: Role::parse(&row.2).unwrap_or(Role::Viewer),
                provider: "auth0".into(),
            },
            false,
        ));
    }
    let id = Uuid::new_v4().to_string();
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role, auth0_sub, email, disabled, created_at, updated_at) \
         VALUES (?, ?, NULL, ?, ?, ?, 0, ?, ?)",
    )
    .bind(&id)
    .bind(username)
    .bind(role.as_str())
    .bind(sub)
    .bind(email)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok((
        AuthUser {
            id,
            username: username.to_string(),
            role,
            provider: "auth0".into(),
        },
        true,
    ))
}

pub async fn count_enabled_admins(pool: &SqlitePool) -> ApiResult<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE role = 'admin' AND COALESCE(disabled, 0) = 0",
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn ensure_not_last_enabled_admin(pool: &SqlitePool, user: &UserRecord) -> ApiResult<()> {
    if user.role.is_admin() && !user.disabled {
        let n = count_enabled_admins(pool).await?;
        if n <= 1 {
            return Err(AppError::bad(
                "cannot disable or demote the last enabled admin",
            ));
        }
    }
    Ok(())
}

pub fn generate_reset_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Create a password-reset token for a local user. Returns the raw token.
pub async fn create_password_reset_token(pool: &SqlitePool, user_id: &str) -> ApiResult<String> {
    // Invalidate outstanding unused tokens for this user.
    let now = db::now_rfc3339();
    let _ = sqlx::query(
        "UPDATE password_reset_tokens SET used_at = ? WHERE user_id = ? AND used_at IS NULL",
    )
    .bind(&now)
    .bind(user_id)
    .execute(pool)
    .await;

    let raw = generate_reset_token();
    let token_hash = mail::hash_token(&raw);
    let id = Uuid::new_v4().to_string();
    let expires = (Utc::now() + Duration::seconds(RESET_TOKEN_TTL_SECS)).to_rfc3339();
    sqlx::query(
        "INSERT INTO password_reset_tokens (id, user_id, token_hash, expires_at, used_at, created_at) \
         VALUES (?, ?, ?, ?, NULL, ?)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(&token_hash)
    .bind(&expires)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(raw)
}

/// Consume a valid unused non-expired token and return the user id.
pub async fn consume_password_reset_token(pool: &SqlitePool, raw_token: &str) -> ApiResult<String> {
    let token_hash = mail::hash_token(raw_token);
    let row = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, user_id, expires_at, used_at FROM password_reset_tokens WHERE token_hash = ?",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::bad("invalid or expired reset token"))?;

    let (id, user_id, expires_at, used_at) = row;
    if used_at.is_some() {
        return Err(AppError::bad("invalid or expired reset token"));
    }
    let expires = chrono::DateTime::parse_from_rfc3339(&expires_at)
        .map_err(|_| AppError::bad("invalid or expired reset token"))?;
    if expires < Utc::now() {
        return Err(AppError::bad("invalid or expired reset token"));
    }

    let now = db::now_rfc3339();
    let result = sqlx::query(
        "UPDATE password_reset_tokens SET used_at = ? WHERE id = ? AND used_at IS NULL",
    )
    .bind(&now)
    .bind(&id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::bad("invalid or expired reset token"));
    }
    Ok(user_id)
}

pub async fn set_user_password(pool: &SqlitePool, user_id: &str, password: &str) -> ApiResult<()> {
    let hash = hash_password(password).map_err(AppError::Anyhow)?;
    let now = db::now_rfc3339();
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(&hash)
        .bind(&now)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::str::FromStr;

    #[test]
    fn role_parse_validates() {
        assert_eq!(Role::parse("admin"), Some(Role::Admin));
        assert_eq!(Role::parse("operator"), Some(Role::Operator));
        assert_eq!(Role::parse("viewer"), Some(Role::Viewer));
        assert_eq!(Role::parse("root"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn role_permissions() {
        assert!(Role::Admin.can_mutate());
        assert!(Role::Operator.can_mutate());
        assert!(!Role::Viewer.can_mutate());
        assert!(Role::Admin.is_admin());
        assert!(!Role::Operator.is_admin());
    }

    async fn test_pool() -> SqlitePool {
        let opts = sqlx::sqlite::SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        db::migrate(&pool).await.unwrap();
        pool
    }

    async fn insert_user(
        pool: &SqlitePool,
        username: &str,
        role: Role,
        password: Option<&str>,
        disabled: bool,
    ) -> String {
        let id = Uuid::new_v4().to_string();
        let now = db::now_rfc3339();
        let hash = password.map(|p| hash_password(p).unwrap());
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, email, disabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind(&hash)
        .bind(role.as_str())
        .bind(format!("{username}@example.com"))
        .bind(if disabled { 1 } else { 0 })
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn disabled_user_flag_visible_on_lookup() {
        let pool = test_pool().await;
        insert_user(&pool, "bob", Role::Viewer, Some("password1"), true).await;
        let row = find_user_by_username(&pool, "bob").await.unwrap().unwrap();
        assert!(row.3);
        assert!(verify_password("password1", row.1.as_ref().unwrap()));
    }

    #[tokio::test]
    async fn password_reset_token_single_use() {
        let pool = test_pool().await;
        let id = insert_user(&pool, "alice", Role::Operator, Some("oldpass12"), false).await;
        let raw = create_password_reset_token(&pool, &id).await.unwrap();
        let uid = consume_password_reset_token(&pool, &raw).await.unwrap();
        assert_eq!(uid, id);
        let err = consume_password_reset_token(&pool, &raw).await.unwrap_err();
        assert!(err.to_string().contains("invalid or expired"));
    }

    #[tokio::test]
    async fn password_reset_token_rejects_expired() {
        let pool = test_pool().await;
        let id = insert_user(&pool, "carol", Role::Viewer, Some("oldpass12"), false).await;
        let raw = generate_reset_token();
        let token_hash = mail::hash_token(&raw);
        let past = (Utc::now() - Duration::hours(2)).to_rfc3339();
        let now = db::now_rfc3339();
        sqlx::query(
            "INSERT INTO password_reset_tokens (id, user_id, token_hash, expires_at, used_at, created_at) \
             VALUES (?, ?, ?, ?, NULL, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(&token_hash)
        .bind(&past)
        .bind(&now)
        .execute(&pool)
        .await
        .unwrap();
        let err = consume_password_reset_token(&pool, &raw).await.unwrap_err();
        assert!(err.to_string().contains("invalid or expired"));
    }

    #[tokio::test]
    async fn final_enabled_admin_protected() {
        let pool = test_pool().await;
        let id = insert_user(&pool, "admin", Role::Admin, Some("adminpass"), false).await;
        let user = get_user(&pool, &id).await.unwrap().unwrap();
        let err = ensure_not_last_enabled_admin(&pool, &user)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("last enabled admin"));

        insert_user(&pool, "admin2", Role::Admin, Some("adminpass"), false).await;
        ensure_not_last_enabled_admin(&pool, &user).await.unwrap();
    }

    #[tokio::test]
    async fn auth0_only_source() {
        let pool = test_pool().await;
        let (user, created) = find_or_create_auth0_user(
            &pool,
            "auth0|1",
            "sso@ex.com",
            Some("sso@ex.com"),
            Role::Viewer,
        )
        .await
        .unwrap();
        assert!(created);
        let rec = get_user(&pool, &user.id).await.unwrap().unwrap();
        assert_eq!(rec.source(), "auth0");
        assert!(rec.is_auth0_only());
        assert!(!rec.is_local());

        let (_, created2) = find_or_create_auth0_user(
            &pool,
            "auth0|1",
            "sso@ex.com",
            Some("sso@ex.com"),
            Role::Admin,
        )
        .await
        .unwrap();
        assert!(!created2);
    }
}
