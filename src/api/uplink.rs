use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::{self, Identity};
use crate::domain::uplink::{self, Upload};
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct DirectQ {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    filename: String,
}

fn into_session(row: crate::db::UploadRow, st: &AppState) -> serde_json::Value {
    json!({
        "token": row.token,
        "share_id": row.share_id,
        "path": row.rel_path,
        "filename": row.filename,
        "size": row.total_size,
        "chunk_size": row.chunk_size,
        "num_parts": row.num_parts,
        "received": uplink::received_parts(st, &row.token),
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

pub async fn init(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<InitReq>,
) -> ApiResult<Response> {
    let user = identity.require()?;
    if !user.can_upload {
        return Err(ApiError::forbidden("当前账号无上传权限"));
    }
    if req.size < 0 {
        return Err(ApiError::bad_request("文件大小非法"));
    }
    let filename =
        crate::domain::paths::check_name(&req.filename).map_err(ApiError::bad_request)?;
    let spot = crate::api::locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.path,
        |u| u.can_upload,
        "上传",
    )
    .await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    // 磁盘余量预检:文件体积 + 系统余量,不够就在开头明确报错
    crate::domain::uplink::ensure_disk_space(&spot.full, req.size)?;
    let chunk = st.chunk_size;
    let num_parts = if req.size == 0 {
        1
    } else {
        (req.size + chunk - 1) / chunk
    };
    let u = Upload {
        token: auth::new_upload_token(),
        user_id: user.id,
        share_id: req.share_id,
        rel_path: spot.rel.clone(),
        filename,
        total_size: req.size,
        chunk_size: chunk,
        num_parts,
    };
    let id = st.db.create_upload(&u).await?;
    std::fs::create_dir_all(uplink::part_dir(&st, &u.token))?;
    Ok(ok(json!({
        "id": id, "token": u.token, "filename": u.filename,
        "share_id": u.share_id, "path": u.rel_path,
        "size": u.total_size, "chunk_size": u.chunk_size, "num_parts": u.num_parts,
    })))
}

pub async fn parts(
    State(st): State<AppState>,
    identity: Identity,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let row = owned(&st, &identity, &token).await?;
    Ok(ok(
        json!({ "parts": uplink::received_parts(&st, &row.token) }),
    ))
}

/// 归属校验:只有会话属主(或管理员)能操作自己的上传会话。
async fn owned(st: &AppState, identity: &Identity, token: &str) -> ApiResult<crate::db::UploadRow> {
    let row = st
        .db
        .upload_by_token(token)
        .await?
        .ok_or_else(|| ApiError::not_found("上传会话不存在"))?;
    if row.user_id != identity.id() && !identity.is_admin() {
        return Err(ApiError::forbidden("无权访问该上传会话"));
    }
    Ok(row)
}

pub async fn part(
    State(st): State<AppState>,
    identity: Identity,
    Path((token, part)): Path<(String, i64)>,
    body: Bytes,
) -> ApiResult<Response> {
    let row = owned(&st, &identity, &token).await?;
    if part < 0 || part >= row.num_parts {
        return Err(ApiError::bad_request("分片序号越界"));
    }
    if body.len() as i64 > row.chunk_size {
        return Err(ApiError::bad_request("分片过大"));
    }
    let dir = uplink::part_dir(&st, &token);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("part{part}")), &body)?;
    let _ = st.db.touch_upload(row.id).await;
    Ok(ok(json!({ "received": part, "size": body.len() })))
}

pub async fn complete(
    State(st): State<AppState>,
    identity: Identity,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let row = owned(&st, &identity, &token).await?;
    let u = Upload {
        token: row.token.clone(),
        user_id: row.user_id,
        share_id: row.share_id,
        rel_path: row.rel_path.clone(),
        filename: row.filename.clone(),
        total_size: row.total_size,
        chunk_size: row.chunk_size,
        num_parts: row.num_parts,
    };
    let dir = uplink::part_dir(&st, &u.token);
    for i in 0..u.num_parts {
        let want = u.part_len(i);
        let got = tokio::fs::metadata(dir.join(format!("part{i}")))
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(-1);
        if got != want {
            return Err(ApiError::conflict(format!(
                "分片不完整,缺失或损坏的分片: {i}"
            )));
        }
    }
    let spot = crate::api::locate_writable(
        &st,
        &identity,
        u.share_id,
        &u.rel_path,
        |x| x.can_upload,
        "上传",
    )
    .await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    // 合并前文件名再次校验(可能与会话创建时的规则不同)
    let filename = crate::domain::paths::check_name(&u.filename).map_err(ApiError::bad_request)?;
    let target = spot.full.join(&filename);
    if target.is_dir() {
        return Err(ApiError::conflict("同名文件夹已存在"));
    }
    // 合并前磁盘余量预检:分片与目标目录同卷时,分片删除释放的空间正好抵消
    // 临时文件增长(净增 ≈ 0),无需拦截;跨卷时要求目标卷有完整余量
    if !crate::domain::uplink::same_volume(&dir, spot.full.parent().unwrap_or(&spot.full)) {
        crate::domain::uplink::ensure_disk_space(&spot.full, u.total_size)?;
    }
    let existed = target.is_file();
    let rel_target = if spot.rel.is_empty() {
        filename.clone()
    } else {
        format!("{}/{}", spot.rel, filename)
    };
    // 共享目录内覆盖同名文件:普通用户只能覆盖自己创建的文件
    if existed && !crate::api::can_manage(&st, &identity, &spot.share, &rel_target).await {
        return Err(ApiError::forbidden(
            "同名文件已存在且不是你创建的,无法覆盖(请联系管理员)",
        ));
    }
    // 流式合并:分片即耗即删,磁盘峰值 ≈ 文件体积;失败时整个会话终止(分片已耗尽,无法续传)
    let st2 = st.clone();
    let u2 = u.clone();
    let target_out = target.clone();
    let merge_result =
        tokio::task::spawn_blocking(move || uplink::merge_and_finish(&u2, &st2, &target_out))
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    if let Err(e) = merge_result {
        uplink::abort_session(&st, &u.token, row.id).await;
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = st.db.delete_upload(row.id).await;
    // 记录条目归属(共享目录内只有创建者/管理员能后续修改删除)
    let _ = st
        .db
        .set_meta_owner(u.share_id, &rel_target, u.user_id)
        .await;
    // 覆盖同名文件时清理旧内容遗留的转码 / 缩略图缓存
    if existed {
        crate::domain::media::purge(&st, u.share_id, &rel_target).await;
    }
    // 上传视频自动转码替换(后台设置开启时异步执行,不阻塞上传响应)
    let st3 = st.clone();
    let rel3 = rel_target.clone();
    let target3 = target.clone();
    tokio::spawn(async move {
        crate::domain::media::maybe_replace_transcode(&st3, u.share_id, &rel3, &target3).await;
    });
    Ok(ok(
        json!({ "ok": true, "share_id": u.share_id, "path": rel_target, "size": u.total_size }),
    ))
}

pub async fn abort(
    State(st): State<AppState>,
    identity: Identity,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let row = owned(&st, &identity, &token).await?;
    let _ = st.db.delete_upload(row.id).await;
    let _ = std::fs::remove_dir_all(uplink::part_dir(&st, &row.token));
    Ok(ok(json!({ "ok": true })))
}

/// 小文件直传:单请求完成上传,不建分片会话(服务端阈值 = 分片大小,
/// 由 /api/shares 下发 direct_max,前端据此自动判定走直传还是分片续传)。
pub async fn direct_upload(
    State(st): State<AppState>,
    identity: Identity,
    Query(q): Query<DirectQ>,
    body: Bytes,
) -> ApiResult<Response> {
    if q.filename.is_empty() {
        return Err(ApiError::bad_request("文件名不能为空"));
    }
    if body.len() as i64 > st.chunk_size {
        return Err(ApiError::bad_request("文件超过直传上限,请使用分片上传"));
    }
    let spot = crate::api::locate_writable(
        &st,
        &identity,
        q.share_id,
        &q.path,
        |x| x.can_upload,
        "上传",
    )
    .await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目标目录不存在"));
    }
    // 合并前文件名校验(与会话分片路径同规则)
    let filename = crate::domain::paths::check_name(&q.filename).map_err(ApiError::bad_request)?;
    let target = spot.full.join(&filename);
    if target.is_dir() {
        return Err(ApiError::conflict("同名文件夹已存在"));
    }
    let existed = target.is_file();
    let rel_target = if spot.rel.is_empty() {
        filename.clone()
    } else {
        format!("{}/{}", spot.rel, filename)
    };
    // 共享目录内覆盖同名文件:普通用户只能覆盖自己创建的文件
    if existed && !crate::api::can_manage(&st, &identity, &spot.share, &rel_target).await {
        return Err(ApiError::forbidden(
            "同名文件已存在且不是你创建的,无法覆盖(请联系管理员)",
        ));
    }
    crate::domain::uplink::ensure_disk_space(&spot.full, body.len() as i64)?;
    // 同目录临时文件 + 原子替换;失败即删,残留由看护任务兜底
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = spot.full.join(format!(
        ".nd-direct-{}-{}.tmp",
        crate::state::now_secs(),
        nanos
    ));
    if let Err(e) = std::fs::write(&tmp, &body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("写入失败: {e}")));
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("保存失败: {e}")));
    }
    let _ = st
        .db
        .set_meta_owner(
            spot.share.id,
            &rel_target,
            identity.user.as_ref().map(|u| u.id).unwrap_or(0),
        )
        .await;
    // 覆盖同名文件时清理旧内容遗留的转码 / 缩略图缓存
    if existed {
        crate::domain::media::purge(&st, spot.share.id, &rel_target).await;
    }
    // 上传视频自动转码替换(与分片路径同一策略)
    let st3 = st.clone();
    let rel3 = rel_target.clone();
    let target3 = target.clone();
    tokio::spawn(async move {
        crate::domain::media::maybe_replace_transcode(&st3, spot.share.id, &rel3, &target3).await;
    });
    Ok(ok(
        json!({ "ok": true, "path": rel_target, "size": body.len() }),
    ))
}

pub async fn list_mine(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    let user = identity.require()?;
    let rows = st.db.uploads_of(user.id).await?;
    let out: Vec<_> = rows.into_iter().map(|r| into_session(r, &st)).collect();
    Ok(ok(json!({ "uploads": out })))
}
