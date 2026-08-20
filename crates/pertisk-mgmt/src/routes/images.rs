//! Cloud qcow2 catalog (`images_dir`). Upload / list / delete; no image build.

use std::path::{Path, PathBuf};

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, State};
use axum::routing::get;
use axum::{Json, Router};
use tokio::io::AsyncWriteExt;

use uuid::Uuid;

use crate::auth::audit;
use crate::cloud_images;
use crate::error::{ApiResult, AppError};
use crate::rbac::require_mutate;
use crate::routes::CurrentUser;
use crate::state::AppState;

/// Sparse qcow2 from `make cloud` is typically a few hundred MiB; role-sized
/// copies can be larger. Cap well above that so a full 50G sparse file still fails loudly.
const MAX_UPLOAD: usize = 8 * 1024 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/images", get(list))
        .merge(
            Router::new()
                .route("/images", axum::routing::post(create))
                .layer(DefaultBodyLimit::max(MAX_UPLOAD)),
        )
        .route("/images/{name}", axum::routing::delete(delete))
}

async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> ApiResult<Json<cloud_images::Catalog>> {
    Ok(Json(cloud_images::list(&state.cfg().images_dir)))
}

async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    mut multipart: Multipart,
) -> ApiResult<Json<cloud_images::CloudImage>> {
    require_mutate(&user)?;
    let dir = state.cfg().images_dir.clone();
    std::fs::create_dir_all(&dir).map_err(anyhow::Error::from)?;

    let mut arch_hint: Option<String> = None;
    let mut orig_name = String::new();
    let mut tmp_path: Option<PathBuf> = None;

    let ingest: ApiResult<cloud_images::CloudImage> = async {
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::bad(format!("multipart: {e}")))?
        {
            let name = field.name().unwrap_or("").to_string();
            let file_name = field.file_name().unwrap_or("").to_string();
            match name.as_str() {
                "arch" => {
                    let data = field
                        .bytes()
                        .await
                        .map_err(|e| AppError::bad(format!("read arch: {e}")))?;
                    let raw = String::from_utf8_lossy(&data);
                    if !raw.trim().is_empty() {
                        arch_hint = Some(
                            crate::os_upgrade::normalize_arch(raw.trim())
                                .map_err(|e| AppError::bad(e.to_string()))?,
                        );
                    }
                }
                "image" | "file" | "qcow2" | "disk" => {
                    if !file_name.is_empty() {
                        orig_name = file_name.clone();
                    }
                    tmp_path = Some(stream_to_temp(&dir, &mut field).await?);
                }
                _ => {
                    if file_name.to_ascii_lowercase().ends_with(".qcow2") {
                        orig_name = file_name.clone();
                        tmp_path = Some(stream_to_temp(&dir, &mut field).await?);
                    }
                }
            }
        }

        let tmp = tmp_path
            .as_ref()
            .ok_or_else(|| AppError::bad("missing qcow2 file (field image/file)"))?;
        let orig = if orig_name.is_empty() {
            "disk.qcow2".into()
        } else {
            cloud_images::sanitize_filename(&orig_name).map_err(|e| AppError::bad(e.to_string()))?
        };
        cloud_images::verify_qcow2(tmp).map_err(|e| AppError::bad(e.to_string()))?;

        let arch = arch_hint
            .clone()
            .or_else(|| crate::os_upgrade::infer_arch_from_name(&orig))
            .unwrap_or_else(|| "amd64".into());
        let arch =
            crate::os_upgrade::normalize_arch(&arch).map_err(|e| AppError::bad(e.to_string()))?;
        let dest_name = cloud_images::dest_name(&orig, &arch);
        let dest = dir.join(&dest_name);
        if dest.exists() {
            std::fs::remove_file(&dest).map_err(anyhow::Error::from)?;
        }
        std::fs::rename(tmp, &dest).map_err(anyhow::Error::from)?;

        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let is_default = dest_name.eq_ignore_ascii_case(&format!("pertisk-cloud-{arch}.qcow2"));
        let created_at = std::fs::metadata(&dest)
            .ok()
            .as_ref()
            .and_then(cloud_images::file_created_at);
        Ok(cloud_images::CloudImage {
            name: dest_name.clone(),
            arch: arch.clone(),
            size_bytes: size,
            role: cloud_images::role_from_name(&dest_name, &arch),
            is_default,
            created_at,
        })
    }
    .await;

    if ingest.is_err() {
        if let Some(tmp) = tmp_path {
            let _ = std::fs::remove_file(tmp);
        }
    }
    let img = ingest?;
    audit(
        state.pool(),
        Some(&user.id),
        "image.create",
        Some(&img.name),
        Some(&format!("{} {}", img.arch, img.name)),
    )
    .await;
    Ok(Json(img))
}

async fn stream_to_temp(
    dir: &Path,
    field: &mut axum::extract::multipart::Field<'_>,
) -> ApiResult<PathBuf> {
    let tmp = dir.join(format!(".upload-{}.qcow2", Uuid::new_v4()));
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(anyhow::Error::from)?;
    let mut written: u64 = 0;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|e| AppError::bad(format!("read image: {e}")))?
    {
        written += chunk.len() as u64;
        if written > MAX_UPLOAD as u64 {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(AppError::bad("image exceeds 8 GiB upload limit"));
        }
        file.write_all(&chunk).await.map_err(anyhow::Error::from)?;
    }
    file.flush().await.map_err(anyhow::Error::from)?;
    Ok(tmp)
}

async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    require_mutate(&user)?;
    let name = cloud_images::sanitize_filename(&name).map_err(|e| AppError::bad(e.to_string()))?;
    let path = state.cfg().images_dir.join(&name);
    if !path.is_file() {
        return Err(AppError::NotFound);
    }
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE kind IN ('create_cluster', 'add_node') \
         AND status IN ('queued', 'running')",
    )
    .fetch_one(state.pool())
    .await?;
    if busy > 0 {
        return Err(AppError::Conflict(
            "cannot delete an image while a cluster create or add-node job is running".into(),
        ));
    }
    std::fs::remove_file(&path).map_err(anyhow::Error::from)?;
    audit(
        state.pool(),
        Some(&user.id),
        "image.delete",
        Some(&name),
        Some(&name),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}
