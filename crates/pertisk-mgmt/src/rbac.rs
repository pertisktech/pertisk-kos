use crate::auth::{AuthUser, Role};
use crate::error::{ApiResult, AppError};

/// Require at least operator (mutate) privileges.
pub fn require_mutate(user: &AuthUser) -> ApiResult<()> {
    if user.role.can_mutate() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Require admin.
pub fn require_admin(user: &AuthUser) -> ApiResult<()> {
    if user.role.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Map Auth0 claim `https://pertisk.io/role` or `role` to Role.
pub fn role_from_claims(claims: &serde_json::Value) -> Role {
    let role = claims
        .get("https://pertisk.io/role")
        .or_else(|| claims.get("role"))
        .and_then(|v| v.as_str())
        .unwrap_or("viewer");
    Role::parse(role).unwrap_or(Role::Viewer)
}
