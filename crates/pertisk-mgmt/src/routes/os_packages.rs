//! Catalog of signed OS A/B upgrade packages (version + arch).

use std::path::{Path, PathBuf};

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::audit;
use crate::db;
use crate::error::{ApiResult, AppError};
use crate::jobs;
use crate::os_upgrade;
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/os-packages", get(list))
        .merge(
            Router::new()
                .route("/os-packages", post(create))
                .layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route("/os-packages/{id}", get(get_one).delete(delete))
        .route("/os-packages/{id}/apply", post(apply))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(crate) struct PackageOut {
    pub id: String,
    pub version: String,
    pub arch: String,
    pub path: String,
    pub size_bytes: i64,
    pub has_trust_pk: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
struct ApplyIn {
    cluster_id: Option<String>,
    #[serde(default)]
    cluster_ids: Vec<String>,
    #[serde(default = "default_reboot")]
    reboot: bool,
}

fn default_reboot() -> bool {
    true
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<Vec<PackageOut>>> {
    let rows = sqlx::query_as::<_, PackageOut>(
        "SELECT id, version, arch, path, size_bytes, has_trust_pk, created_at, updated_at \
         FROM os_packages ORDER BY version DESC, arch",
    )
    .fetch_all(state.pool())
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<PackageOut>> {
    load_package(&state, &id)
        .await?
        .ok_or(AppError::NotFound)
        .map(Json)
}

async fn load_package(state: &AppState, id: &str) -> ApiResult<Option<PackageOut>> {
    Ok(sqlx::query_as::<_, PackageOut>(
        "SELECT id, version, arch, path, size_bytes, has_trust_pk, created_at, updated_at \
         FROM os_packages WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(state.pool())
    .await?)
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    mut multipart: Multipart,
) -> ApiResult<Json<PackageOut>> {
    require_mutate(&user)?;
    let tmp_id = Uuid::new_v4().to_string();
    let dest = state.cfg().os_packages_dir().join(format!(".upload-{tmp_id}"));
    std::fs::create_dir_all(&dest).map_err(anyhow::Error::from)?;

    let mut zip_bytes: Option<Vec<u8>> = None;
    let mut arch_hint: Option<String> = None;
    let mut name_hint = String::new();

    let ingest = async {
        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::bad(format!("multipart: {e}")))?
        {
            let name = field.name().unwrap_or("").to_string();
            let file_name = field.file_name().unwrap_or("").to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| AppError::bad(format!("read field {name}: {e}")))?;

            match name.as_str() {
                "arch" => {
                    let raw = String::from_utf8_lossy(&data);
                    if !raw.trim().is_empty() {
                        arch_hint = Some(
                            os_upgrade::normalize_arch(raw.trim())
                                .map_err(|e| AppError::bad(e.to_string()))?,
                        );
                    }
                }
                "bundle" | "archive" | "zip" => {
                    if !file_name.is_empty() {
                        name_hint = file_name.clone();
                    }
                    zip_bytes = Some(data.to_vec());
                }
                _ => {
                    let orig = if file_name.is_empty() {
                        name.clone()
                    } else {
                        file_name.clone()
                    };
                    if orig.to_ascii_lowercase().ends_with(".zip") {
                        name_hint = orig;
                        zip_bytes = Some(data.to_vec());
                    } else if os_upgrade::canonical_bundle_name(
                        Path::new(&orig)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&orig),
                    )
                    .is_some()
                    {
                        os_upgrade::write_bundle_file(&dest, &orig, &data)
                            .map_err(|e| AppError::bad(e.to_string()))?;
                    }
                }
            }
        }

        let version = if let Some(bytes) = zip_bytes {
            let zip_path = dest.join("_upload.zip");
            std::fs::write(&zip_path, &bytes).map_err(anyhow::Error::from)?;
            let v = os_upgrade::extract_bundle_zip(&zip_path, &dest)
                .map_err(|e| AppError::bad(e.to_string()))?;
            let _ = std::fs::remove_file(&zip_path);
            v
        } else {
            os_upgrade::validate_bundle_dir(&dest).map_err(|e| AppError::bad(e.to_string()))?
        };

        let arch = arch_hint
            .or_else(|| os_upgrade::infer_arch_from_name(&name_hint))
            .unwrap_or_else(|| "amd64".into());
        let arch =
            os_upgrade::normalize_arch(&arch).map_err(|e| AppError::bad(e.to_string()))?;

        upsert_package(&state, &dest, &version, &arch).await
    }
    .await;

    let _ = std::fs::remove_dir_all(&dest);
    let pkg = ingest?;

    audit(
        state.pool(),
        Some(&user.id),
        "os_package.create",
        Some(&pkg.id),
        Some(&format!("{} {}", pkg.version, pkg.arch)),
    )
    .await;
    Ok(Json(pkg))
}

/// Copy a validated bundle directory into the catalog (upsert by version+arch).
pub(crate) async fn upsert_package(
    state: &AppState,
    src: &Path,
    version: &str,
    arch: &str,
) -> ApiResult<PackageOut> {
    os_upgrade::validate_bundle_dir(src).map_err(|e| AppError::bad(e.to_string()))?;
    let now = db::now_rfc3339();
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id, path FROM os_packages WHERE version = ? AND arch = ?",
    )
    .bind(version)
    .bind(arch)
    .fetch_optional(state.pool())
    .await?;

    let (id, dest) = if let Some((id, path)) = existing {
        (id, PathBuf::from(path))
    } else {
        let id = Uuid::new_v4().to_string();
        let dest = state.cfg().os_packages_dir().join(&id);
        (id, dest)
    };

    os_upgrade::copy_bundle_dir(src, &dest).map_err(anyhow::Error::from)?;
    let size = os_upgrade::dir_size_bytes(&dest) as i64;
    let has_pk = i64::from(os_upgrade::bundle_trust_pk(&dest).is_some());
    let path_s = dest.display().to_string();

    sqlx::query(
        r#"INSERT INTO os_packages (id, version, arch, path, size_bytes, has_trust_pk, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(version, arch) DO UPDATE SET
             path = excluded.path,
             size_bytes = excluded.size_bytes,
             has_trust_pk = excluded.has_trust_pk,
             updated_at = excluded.updated_at"#,
    )
    .bind(&id)
    .bind(version)
    .bind(arch)
    .bind(&path_s)
    .bind(size)
    .bind(has_pk)
    .bind(&now)
    .bind(&now)
    .execute(state.pool())
    .await?;

    sqlx::query_as::<_, PackageOut>(
        "SELECT id, version, arch, path, size_bytes, has_trust_pk, created_at, updated_at \
         FROM os_packages WHERE version = ? AND arch = ?",
    )
    .bind(version)
    .bind(arch)
    .fetch_optional(state.pool())
    .await?
    .ok_or_else(|| AppError::bad("failed to load saved OS package"))
}

pub(crate) async fn enqueue_from_package_id(
    state: &AppState,
    user_id: &str,
    cluster_id: &str,
    package_id: &str,
    reboot: bool,
    node_ids: Option<Vec<String>>,
) -> ApiResult<(String, String)> {
    let pkg = load_package(state, package_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let job_id =
        enqueue_from_package(state, user_id, cluster_id, &pkg, reboot, node_ids).await?;
    Ok((job_id, pkg.version))
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let pkg = load_package(&state, &id).await?.ok_or(AppError::NotFound)?;
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind = 'upgrade_os' AND status IN ('queued', 'running') \
         AND payload_json LIKE ?",
    )
    .bind(format!("%{}%", pkg.id))
    .fetch_one(state.pool())
    .await?;
    if busy > 0 {
        return Err(AppError::Conflict(
            "package is in use by a running OS upgrade".into(),
        ));
    }
    sqlx::query("DELETE FROM os_packages WHERE id = ?")
        .bind(&id)
        .execute(state.pool())
        .await?;
    let _ = std::fs::remove_dir_all(&pkg.path);
    audit(
        state.pool(),
        Some(&user.id),
        "os_package.delete",
        Some(&id),
        Some(&format!("{} {}", pkg.version, pkg.arch)),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn apply(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ApplyIn>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let pkg = load_package(&state, &id).await?.ok_or(AppError::NotFound)?;
    os_upgrade::validate_bundle_dir(Path::new(&pkg.path))
        .map_err(|e| AppError::bad(format!("package on disk is incomplete: {e}")))?;

    let mut cluster_ids = body.cluster_ids;
    if let Some(one) = body.cluster_id {
        if !one.trim().is_empty() {
            cluster_ids.push(one);
        }
    }
    cluster_ids.retain(|s| !s.trim().is_empty());
    cluster_ids.sort();
    cluster_ids.dedup();
    if cluster_ids.is_empty() {
        return Err(AppError::bad("cluster_id or cluster_ids is required"));
    }

    let mut jobs_out = Vec::new();
    for cid in cluster_ids {
        let job_id = enqueue_from_package(&state, &user.id, &cid, &pkg, body.reboot, None).await?;
        jobs_out.push(serde_json::json!({ "cluster_id": cid, "job_id": job_id }));
    }
    Ok(Json(serde_json::json!({
        "version": pkg.version,
        "arch": pkg.arch,
        "jobs": jobs_out,
    })))
}

pub(crate) async fn enqueue_from_package(
    state: &AppState,
    user_id: &str,
    cluster_id: &str,
    pkg: &PackageOut,
    reboot: bool,
    node_ids: Option<Vec<String>>,
) -> ApiResult<String> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT id, arch, status FROM clusters WHERE id = ?",
    )
    .bind(cluster_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((_, cluster_arch, status)) = row else {
        return Err(AppError::NotFound);
    };
    if status == "deleting" {
        return Err(AppError::Conflict("cluster is deleting".into()));
    }
    let cluster_arch = os_upgrade::normalize_arch(&cluster_arch)
        .unwrap_or_else(|_| cluster_arch.to_ascii_lowercase());
    if cluster_arch != pkg.arch {
        return Err(AppError::bad(format!(
            "package arch {} does not match cluster arch {cluster_arch}",
            pkg.arch
        )));
    }
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE cluster_id = ? AND kind IN ('upgrade_cluster', 'upgrade_os') \
         AND status IN ('queued', 'running')",
    )
    .bind(cluster_id)
    .fetch_one(state.pool())
    .await?;
    if running > 0 {
        return Err(AppError::Conflict(
            "an upgrade is already queued or running on this cluster".into(),
        ));
    }

    let job_id = jobs::enqueue(
        state,
        Some(cluster_id),
        "upgrade_os",
        serde_json::json!({
            "bundle_dir": pkg.path,
            "version": pkg.version,
            "reboot": reboot,
            "node_ids": node_ids,
            "package_id": pkg.id,
        }),
    )
    .await
    .map_err(AppError::Anyhow)?;
    audit(
        state.pool(),
        Some(user_id),
        "cluster.os_upgrade",
        Some(cluster_id),
        Some(&pkg.version),
    )
    .await;
    Ok(job_id)
}
