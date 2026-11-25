//! 分片上传:初始化 / 分片写入 / 完成合并 / 中止 / 小文件直传 / 我的上传会话。

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::{now_secs, App};
use crate::db::UploadRow;
use crate::domain::chunkup::{self, UploadPlan};
use crate::domain::{auth, media, pathguard};
use crate::error::{ok, ApiError, ApiResult};
use crate::web::ctx::{can_manage, child_rel, locate_writable};
use crate::web::identity::Identity;

fn into_session(row: UploadRow, app: &App) -> serde_json::Value {
    json!({
        "token": row.token,
        "share_id": row.share_id,
        "path": row.rel_path,
        "filename": row.filename,
        "size": row.total_size,
        "chunk_size": row.chunk_size,
        "num_parts": row.num_parts,
        "received": chunkup::received_parts(&app.uploads_dir, &row.token),
    })
}

#[derive(Deserialize)]
pub struct InitReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    filename: String,
    size: i64,
}

pub async fn init(State(app): State<App>, identity: Identity, Json(req): Json<InitReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    if !user.can_upload {
        return Err(ApiError::forbidden("当前账号无上传权限"));
    }
    if req.size < 0 {
        return Err(ApiError::bad_request("文件大小非法"));
    }
    let filename = pathguard::check_name(&req.filename).map_err(ApiError::bad_request)?;
    let spot = locate_writable(&app, &identity, req.share_id, &req.path, |u| u.can_upload, "上传").await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    chunkup::ensure_disk_space(&spot.full, req.size)?;
    let chunk = app.chunk_size;
    let num_parts = if req.size == 0 { 1 } else { (req.size + chunk - 1) / chunk };
    let plan = UploadPlan {
        token: auth::new_upload_token(),
        user_id: user.id,
        share_id: req.share_id,
        rel_path: spot.rel.clone(),
        filename,
        total_size: req.size,
        chunk_size: chunk,
        num_parts,
    };
    let id = app.db.create_upload(&plan).await?;
    std::fs::create_dir_all(chunkup::part_dir(&app.uploads_dir, &plan.token))?;
    Ok(ok(json!({
        "id": id, "token": plan.token, "filename": plan.filename,
        "share_id": plan.share_id, "path": plan.rel_path,
        "size": plan.total_size, "chunk_size": plan.chunk_size, "num_parts": plan.num_parts,
    })))
}

/// 取属于本人的上传会话(管理员可访问任何人的)。
async fn owned(app: &App, identity: &Identity, token: &str) -> ApiResult<UploadRow> {
    let row = app
        .db
        .upload_by_token(token)
        .await?
        .ok_or_else(|| ApiError::not_found("上传会话不存在"))?;
    if row.user_id != identity.id() && !identity.is_admin() {
        return Err(ApiError::forbidden("无权访问该上传会话"));
    }
    Ok(row)
}

pub async fn parts(State(app): State<App>, identity: Identity, Path(token): Path<String>) -> ApiResult<Response> {
    let row = owned(&app, &identity, &token).await?;
    Ok(ok(json!({ "parts": chunkup::received_parts(&app.uploads_dir, &row.token) })))
}

pub async fn part(
    State(app): State<App>,
    identity: Identity,
    Path((token, part)): Path<(String, i64)>,
    body: Bytes,
) -> ApiResult<Response> {
    let row = owned(&app, &identity, &token).await?;
    // 会话创建于 init,但权限可能随时被调整:每次写分片都按当前权限复核。
    locate_writable(&app, &identity, row.share_id, &row.rel_path, |x| x.can_upload, "上传").await?;
    if part < 0 || part >= row.num_parts {
        return Err(ApiError::bad_request("分片序号越界"));
    }
    if body.len() as i64 > row.chunk_size {
        return Err(ApiError::bad_request("分片过大"));
    }
    let dir = chunkup::part_dir(&app.uploads_dir, &token);
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(format!("part{part}")), &body).await?;
    let _ = app.db.touch_upload(row.id).await;
    Ok(ok(json!({ "received": part, "size": body.len() })))
}

pub async fn complete(State(app): State<App>, identity: Identity, Path(token): Path<String>) -> ApiResult<Response> {
    let row = owned(&app, &identity, &token).await?;
    // 并发 complete 互斥:第二个请求得到明确冲突,避免交错写同一合并产物。
    let _merge = chunkup::MergeLock::acquire(&app.uploads_dir, &token)?;
    let plan = UploadPlan {
        token: row.token.clone(),
        user_id: row.user_id,
        share_id: row.share_id,
        rel_path: row.rel_path.clone(),
        filename: row.filename.clone(),
        total_size: row.total_size,
        chunk_size: row.chunk_size,
        num_parts: row.num_parts,
    };
    let dir = chunkup::part_dir(&app.uploads_dir, &plan.token);
    // 校验每片大小。
    for i in 0..plan.num_parts {
        let want = plan.part_len(i);
        let got = tokio::fs::metadata(dir.join(format!("part{i}")))
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(-1);
        if got != want {
            return Err(ApiError::conflict(format!("分片不完整,缺失或损坏的分片: {i}")));
        }
    }
    let spot = locate_writable(&app, &identity, plan.share_id, &plan.rel_path, |x| x.can_upload, "上传").await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    let filename = pathguard::check_name(&plan.filename).map_err(ApiError::bad_request)?;
    let target = spot.full.join(&filename);
    if target.is_dir() {
        return Err(ApiError::conflict("同名文件夹已存在"));
    }
    // 跨卷时合并产物要复制,需要双份空间。
    if !chunkup::same_volume(&dir, spot.full.parent().unwrap_or(&spot.full)) {
        chunkup::ensure_disk_space(&spot.full, plan.total_size)?;
    }
    let existed = target.is_file();
    let rel_target = child_rel(&spot.rel, &filename);
    if existed && !can_manage(&app, &identity, &spot.share, &rel_target).await {
        return Err(ApiError::forbidden("同名文件已存在且不是你创建的,无法覆盖(请联系管理员)"));
    }
    let app2 = app.clone();
    let plan2 = plan.clone();
    let target2 = target.clone();
    let merge_result = tokio::task::spawn_blocking(move || chunkup::merge_and_finish(&plan2, &app2.uploads_dir, &target2))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    if let Err(e) = merge_result {
        chunkup::abort_session(&app.db, &app.uploads_dir, &plan.token, row.id).await;
        return Err(e);
    }
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let _ = app.db.delete_upload(row.id).await;
    let _ = app.db.set_meta_owner(plan.share_id, &rel_target, plan.user_id).await;
    if existed {
        media::purge(&app.db, &app.media_dir, plan.share_id, &rel_target).await;
    }
    app.spawn_replace(plan.share_id, &rel_target, &target);
    Ok(ok(json!({ "ok": true, "share_id": plan.share_id, "path": rel_target, "size": plan.total_size })))
}

pub async fn abort(State(app): State<App>, identity: Identity, Path(token): Path<String>) -> ApiResult<Response> {
    let row = owned(&app, &identity, &token).await?;
    let _ = app.db.delete_upload(row.id).await;
    let _ = std::fs::remove_dir_all(chunkup::part_dir(&app.uploads_dir, &row.token));
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DirectQ {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    filename: String,
}

/// 小文件直传:单请求原子落盘(≤ 分片大小)。
pub async fn direct_upload(
    State(app): State<App>,
    identity: Identity,
    Query(q): Query<DirectQ>,
    body: Bytes,
) -> ApiResult<Response> {
    let user = identity.require()?;
    if q.filename.is_empty() {
        return Err(ApiError::bad_request("文件名不能为空"));
    }
    if body.len() as i64 > app.chunk_size {
        return Err(ApiError::bad_request("文件超过直传上限,请使用分片上传"));
    }
    let spot = locate_writable(&app, &identity, q.share_id, &q.path, |x| x.can_upload, "上传").await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    let filename = pathguard::check_name(&q.filename).map_err(ApiError::bad_request)?;
    let target = spot.full.join(&filename);
    if target.is_dir() {
        return Err(ApiError::conflict("同名文件夹已存在"));
    }
    let existed = target.is_file();
    let rel_target = child_rel(&spot.rel, &filename);
    if existed && !can_manage(&app, &identity, &spot.share, &rel_target).await {
        return Err(ApiError::forbidden("同名文件已存在且不是你创建的,无法覆盖(请联系管理员)"));
    }
    chunkup::ensure_disk_space(&spot.full, body.len() as i64)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = spot.full.join(format!(".nd-direct-{}-{}.tmp", now_secs(), nanos));
    if let Err(e) = std::fs::write(&tmp, &body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("写入失败: {e}")));
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("保存失败: {e}")));
    }
    let _ = app.db.set_meta_owner(spot.share.id, &rel_target, user.id).await;
    if existed {
        media::purge(&app.db, &app.media_dir, spot.share.id, &rel_target).await;
    }
    app.spawn_replace(spot.share.id, &rel_target, &target);
    Ok(ok(json!({ "ok": true, "path": rel_target, "size": body.len() })))
}

pub async fn list_mine(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    let user = identity.require()?;
    let rows = app.db.uploads_of(user.id).await?;
    let out: Vec<_> = rows.into_iter().map(|r| into_session(r, &app)).collect();
    Ok(ok(json!({ "uploads": out })))
}
