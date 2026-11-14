use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{file_name_of, headers_range, locate, locate_readable, locate_writable, stream_file};
use crate::domain::auth::Identity;
use crate::domain::{fsx, media, paths, perms, ticket};
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct PathQ {
    #[serde(default)]
    path: String,
}

/// 浏览目录:条目列表 + 当前用户能力标志。
pub async fn browse(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&st, &identity, share_id, &q.path).await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目录不存在"));
    }
    let mut entries = Vec::new();
    let rd =
        std::fs::read_dir(&spot.full).map_err(|e| ApiError::internal(format!("读取失败: {e}")))?;
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let child_rel = if spot.rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", spot.rel, name)
        };
        if !perms::can_read(&identity, &spot.share, &spot.rules, &child_rel) {
            continue;
        }
        let md = de.metadata().ok();
        let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
        let ext = if is_dir {
            String::new()
        } else {
            paths::ext_of(&name)
        };
        let kind = if is_dir {
            "dir".to_string()
        } else {
            media::kind_label(media::kind_of(&ext)).to_string()
        };
        entries.push(json!({
            "name": name,
            "is_dir": is_dir,
            "size": md.as_ref().map(|m| m.len() as i64).unwrap_or(0),
            "mtime": md.as_ref().map(|m| fsx::unix_secs(m.modified())).unwrap_or(0),
            "ext": ext,
            "kind": kind,
        }));
    }
    entries.sort_by(|a, b| {
        let da = a["is_dir"].as_bool().unwrap();
        let db = b["is_dir"].as_bool().unwrap();
        db.cmp(&da).then_with(|| {
            a["name"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap().to_lowercase())
        })
    });
    let can = |flag: fn(&crate::db::User) -> bool| {
        perms::can_write(&identity, &spot.share, &spot.rules, &spot.rel, flag)
    };
    Ok(ok(json!({
        "path": spot.rel,
        "share": { "id": spot.share.id, "name": spot.share.name, "owner_id": spot.share.owner_id, "home": spot.share.kind == "home" },
        "can": {
            "upload": can(|u| u.can_upload),
            "mkdir": can(|u| u.can_mkdir),
            "delete": can(|u| u.can_delete),
            "download": perms::can_download(&identity, &spot.share, &spot.rules, &spot.rel),
        },
        "entries": entries,
    })))
}

/// 文件元信息(预览用;要求下载权限)。
pub async fn meta(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate(&st, share_id, &q.path).await?;
    perms::ensure_download(&identity, &spot.share, &spot.rules, &spot.rel)?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let md = tokio::fs::metadata(&spot.full).await?;
    let ext = paths::ext_of(&spot.rel);
    Ok(ok(json!({
        "share_id": share_id,
        "path": spot.rel,
        "name": file_name_of(&spot.rel),
        "size": md.len() as i64,
        "mtime": fsx::unix_secs(md.modified()),
        "ext": ext,
        "mime": media::mime_of(&ext),
        "kind": media::kind_label(media::kind_of(&ext)),
        "writable": perms::can_write(&identity, &spot.share, &spot.rules, &spot.rel, |u| u.can_upload)
            && super::can_manage(&st, &identity, &spot.share, &spot.rel).await,
    })))
}

/// 文件 / 文件夹详情:文件夹递归统计大小与条目数;只需读权限。
pub async fn detail(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&st, &identity, share_id, &q.path).await?;
    let md = tokio::fs::metadata(&spot.full)
        .await
        .map_err(|e| ApiError::not_found(format!("读取失败: {e}")))?;
    let mtime = fsx::unix_secs(md.modified());
    let created = std::cmp::min(
        if md.created().is_ok() {
            fsx::unix_secs(md.created())
        } else {
            mtime
        },
        mtime,
    );
    let ext = paths::ext_of(&spot.rel);
    let (size, files, folders) = if md.is_dir() {
        let (mut total, mut fc, mut dc) = (0u64, 0u64, 0u64);
        fsx::stat_tree(&spot.full, 0, &mut total, &mut fc, &mut dc);
        (total as i64, fc as i64, dc as i64)
    } else {
        (md.len() as i64, 0, 0)
    };
    Ok(ok(json!({
        "share_id": share_id,
        "path": spot.rel,
        "name": file_name_of(&spot.rel),
        "is_dir": md.is_dir(),
        "size": size,
        "files": files,
        "folders": folders,
        "mtime": mtime,
        "created": created,
        "ext": ext,
    })))
}

/// 相册:递归列出个人空间内的全部图片(前端只显示缩略图,不显示名称)。
pub async fn album(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
) -> ApiResult<Response> {
    let spot = locate_readable(&st, &identity, share_id, "").await?;
    if spot.share.kind != "home" {
        return Err(ApiError::bad_request("相册仅适用于个人空间"));
    }
    let root = spot.full.clone();
    let mut images = tokio::task::spawn_blocking(move || collect_media(&root, "", 0))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // 最近上传 / 修改的图片排在前面
    images.sort_by_key(|x| std::cmp::Reverse(x.2));
    images.truncate(MAX_ALBUM);
    let out: Vec<_> = images
        .into_iter()
        .map(|(rel, size, mtime, kind)| {
            json!({
                "path": rel,
                "name": super::file_name_of(&rel),
                "size": size,
                "mtime": mtime,
                "kind": kind,
            })
        })
        .collect();
    Ok(ok(json!({ "count": out.len(), "images": out })))
}

const MAX_ALBUM: usize = 5000;

/// 递归收集图片与视频(相对路径, 字节数, 修改时间, 类型);跳过隐藏项与符号链接。
fn collect_media(
    dir: &std::path::Path,
    rel: &str,
    depth: u32,
) -> Vec<(String, i64, i64, &'static str)> {
    let mut out = Vec::new();
    if depth > 24 {
        return out;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(de.path()) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let child_rel = super::child_rel(rel, &name);
        if meta.is_dir() {
            out.extend(collect_media(&de.path(), &child_rel, depth + 1));
        } else if meta.is_file() {
            let kind = media::kind_of(&paths::ext_of(&name));
            if matches!(kind, media::Kind::Image | media::Kind::Video) {
                let label = if kind == media::Kind::Video {
                    "video"
                } else {
                    "image"
                };
                out.push((
                    child_rel,
                    meta.len() as i64,
                    fsx::unix_secs(meta.modified()),
                    label,
                ));
            }
        }
    }
    out
}

#[derive(Deserialize)]
pub struct MkdirReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    name: String,
}

pub async fn mkdir(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<MkdirReq>,
) -> ApiResult<Response> {
    let spot = locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.path,
        |u| u.can_mkdir,
        "新建文件夹",
    )
    .await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("父目录不存在"));
    }
    let name = paths::check_name(&req.name).map_err(ApiError::bad_request)?;
    match std::fs::create_dir(spot.full.join(&name)) {
        Ok(()) => {
            if let Some(u) = identity.user.as_ref() {
                let _ = st
                    .db
                    .set_meta_owner(spot.share.id, &super::child_rel(&spot.rel, &name), u.id)
                    .await;
            }
            Ok(ok(json!({ "ok": true })))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ApiError::conflict("同名文件或文件夹已存在"))
        }
        Err(e) => Err(ApiError::internal(format!("创建失败: {e}"))),
    }
}

#[derive(Deserialize)]
pub struct RenameReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    old: String,
    new: String,
}

pub async fn rename(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<RenameReq>,
) -> ApiResult<Response> {
    let spot = locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.path,
        |u| u.can_delete,
        "重命名",
    )
    .await?;
    let old = paths::check_name(&req.old).map_err(ApiError::bad_request)?;
    let new = paths::check_name(&req.new).map_err(ApiError::bad_request)?;
    let from = spot.full.join(&old);
    let to = spot.full.join(&new);
    if !from.exists() {
        return Err(ApiError::not_found("原文件不存在"));
    }
    if to.exists() {
        return Err(ApiError::conflict("目标已存在"));
    }
    let old_rel = super::child_rel(&spot.rel, &old);
    let new_rel = super::child_rel(&spot.rel, &new);
    super::ensure_manageable(&st, &identity, &spot.share, &old_rel).await?;
    std::fs::rename(&from, &to).map_err(|e| ApiError::internal(format!("重命名失败: {e}")))?;
    let _ = st.db.rename_meta(spot.share.id, &old_rel, &new_rel).await;
    // 媒体缓存按 (share, rel) 索引:旧路径缓存随改名清理
    if !from.is_dir() {
        media::purge(&st, req.share_id, &old_rel).await;
    }
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct MoveReq {
    share_id: i64,
    /// 当前所在目录(源目录)
    #[serde(default)]
    path: String,
    /// 要移动的条目名(同级目录内)
    #[serde(default)]
    names: Vec<String>,
    /// 目标目录(共享内相对路径,空 = 根目录)
    #[serde(default)]
    dest: String,
}

/// 同一共享内把多个条目移动到目标目录(拖拽 / 移动弹窗共用)。
pub async fn move_items(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<MoveReq>,
) -> ApiResult<Response> {
    if req.names.is_empty() {
        return Err(ApiError::bad_request("未选择要移动的内容"));
    }
    let src = locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.path,
        |u| u.can_delete,
        "移动",
    )
    .await?;
    let dst = locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.dest,
        |u| u.can_delete,
        "移动",
    )
    .await?;
    if !dst.full.is_dir() {
        return Err(ApiError::not_found("目标文件夹不存在"));
    }
    // 先整体校验(存在性 / 冲突 / 自嵌套 / 归属),全部通过后再动手
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for raw in &req.names {
        let name = paths::check_name(raw).map_err(ApiError::bad_request)?;
        if !seen.insert(name.clone()) {
            continue;
        }
        let from = src.full.join(&name);
        if !from.exists() {
            return Err(ApiError::not_found(format!("“{name}” 不存在")));
        }
        let src_rel = super::child_rel(&src.rel, &name);
        let dst_rel = super::child_rel(&dst.rel, &name);
        // 已在目标位置:跳过即可
        if src_rel == dst_rel {
            continue;
        }
        // 目标位于被移动文件夹内部(含自身)
        if dst_rel.starts_with(&format!("{src_rel}/")) {
            return Err(ApiError::bad_request(format!(
                "不能把 “{name}” 移动到它自己内部"
            )));
        }
        let to = dst.full.join(&name);
        if to.exists() {
            return Err(ApiError::conflict(format!("目标文件夹里已有同名 “{name}”")));
        }
        super::ensure_manageable(&st, &identity, &src.share, &src_rel).await?;
        items.push((name, from, to, src_rel, dst_rel));
    }
    if items.is_empty() {
        return Ok(ok(json!({ "ok": true, "moved": 0, "errors": [] })));
    }
    let mut moved = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (name, from, to, src_rel, dst_rel) in items {
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                let _ = st.db.rename_meta(req.share_id, &src_rel, &dst_rel).await;
                // 媒体缓存按 (share, rel) 索引:文件路径变化后旧缓存作废
                if !from.is_dir() {
                    media::purge(&st, req.share_id, &src_rel).await;
                }
                moved += 1;
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    if moved == 0 && !errors.is_empty() {
        return Err(ApiError::internal(format!(
            "移动失败: {}",
            errors.join(";")
        )));
    }
    Ok(ok(json!({ "ok": true, "moved": moved, "errors": errors })))
}

#[derive(Deserialize)]
pub struct DeleteReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    recursive: bool,
}

pub async fn delete(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<DeleteReq>,
) -> ApiResult<Response> {
    let spot = locate_writable(
        &st,
        &identity,
        req.share_id,
        &req.path,
        |u| u.can_delete,
        "删除",
    )
    .await?;
    if !spot.full.exists() {
        return Err(ApiError::not_found("目标不存在"));
    }
    if spot.rel.is_empty() {
        return Err(ApiError::bad_request("不能删除共享根目录"));
    }
    super::ensure_manageable(&st, &identity, &spot.share, &spot.rel).await?;
    let removed = if spot.full.is_dir() {
        if !req.recursive {
            let mut it =
                std::fs::read_dir(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            if it.next().is_some() {
                return Err(ApiError::conflict("文件夹非空,需要确认递归删除"));
            }
            std::fs::remove_dir(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            1
        } else {
            // 递归删除前清理目录内所有媒体的转码 / 缩略图缓存
            let mut files = Vec::new();
            fsx::collect_files(&spot.full, &spot.rel, &mut files, 0);
            for (rel, _) in &files {
                media::purge(&st, req.share_id, rel).await;
            }
            let mut n = 1usize;
            count_entries(&spot.full, &mut n);
            std::fs::remove_dir_all(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            n as i64
        }
    } else {
        std::fs::remove_file(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
        media::purge(&st, req.share_id, &spot.rel).await;
        1
    };
    let _ = st.db.drop_meta(spot.share.id, &spot.rel).await;
    Ok(ok(json!({ "ok": true, "removed": removed })))
}

fn count_entries(dir: &std::path::Path, n: &mut usize) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for de in rd.flatten() {
            *n += 1;
            if de.path().is_dir() {
                count_entries(&de.path(), n);
            }
        }
    }
}

/// 读取文本内容(UTF-8;GBK 等编码由前端按原始字节智能处理)。
pub async fn text_get(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    const TEXT_CAP: usize = 8 * 1024 * 1024;
    let spot = locate_readable(&st, &identity, share_id, &q.path).await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let ext = paths::ext_of(&spot.rel);
    if !media::editable_text(&ext) {
        return Err(ApiError::bad_request("该类型不支持文本预览"));
    }
    let len = tokio::fs::metadata(&spot.full).await?.len() as usize;
    if len > TEXT_CAP {
        return Err(ApiError::bad_request("文件过大,请下载后编辑"));
    }
    let raw = tokio::fs::read(&spot.full).await?;
    let text = String::from_utf8_lossy(&raw).to_string();
    Ok(ok(json!({
        "text": text,
        "size": len,
        "editable": perms::can_write(&identity, &spot.share, &spot.rules, &spot.rel, |u| u.can_upload)
            && super::can_manage(&st, &identity, &spot.share, &spot.rel).await,
        "ext": ext,
    })))
}

#[derive(Deserialize)]
pub struct TextSaveReq {
    content: String,
}

pub async fn text_put(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
    Json(req): Json<TextSaveReq>,
) -> ApiResult<Response> {
    const TEXT_CAP: usize = 8 * 1024 * 1024;
    let spot = locate_writable(
        &st,
        &identity,
        share_id,
        &q.path,
        |u| u.can_upload,
        "保存文件",
    )
    .await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    super::ensure_manageable(&st, &identity, &spot.share, &spot.rel).await?;
    let ext = paths::ext_of(&spot.rel);
    if !media::editable_text(&ext) {
        return Err(ApiError::bad_request("该类型不支持在线编辑"));
    }
    if req.content.len() > TEXT_CAP {
        return Err(ApiError::bad_request("内容过大"));
    }
    fsx::atomic_write(&spot.full, req.content.as_bytes())
        .await
        .map_err(ApiError::internal)?;
    Ok(ok(json!({ "ok": true, "size": req.content.len() })))
}

/// 表格二进制覆盖保存(仅允许已有表格文件)。
pub async fn binary_put(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
    body: Bytes,
) -> ApiResult<Response> {
    const BIN_CAP: usize = 32 * 1024 * 1024;
    let spot = locate_writable(
        &st,
        &identity,
        share_id,
        &q.path,
        |u| u.can_upload,
        "保存文件",
    )
    .await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    super::ensure_manageable(&st, &identity, &spot.share, &spot.rel).await?;
    let ext = paths::ext_of(&spot.rel);
    if !media::is_sheet(&ext) {
        return Err(ApiError::bad_request("该类型不支持在线保存"));
    }
    if body.len() > BIN_CAP {
        return Err(ApiError::bad_request("文件过大"));
    }
    fsx::atomic_write(&spot.full, &body)
        .await
        .map_err(ApiError::internal)?;
    Ok(ok(json!({ "ok": true, "size": body.len() })))
}

/// 纯文件流式响应(鉴权由调用方完成)。
async fn serve_download(st: &AppState, headers: &HeaderMap, share_id: i64, rel: &str) -> Response {
    let Ok(spot) = locate(st, share_id, rel).await else {
        return super::not_found("文件不存在或已失效");
    };
    if !spot.full.is_file() {
        return super::not_found("文件不存在或已失效");
    }
    let ext = paths::ext_of(&spot.rel);
    stream_file(
        &spot.full,
        headers_range(headers).as_deref(),
        &media::mime_of(&ext),
        false,
        true,
        &file_name_of(&spot.rel),
    )
    .await
}

/// 签名直链下载(匿名可用)。
pub async fn dl_signed(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((share_id, name)): Path<(i64, String)>,
    Query(q): Query<SigQ>,
) -> Response {
    // 实际下载目标以路径段为准(查询参数里的 path 仅参与签名校验)
    let rel = if q.path.is_empty() { name } else { q.path.clone() };
    if let Err(e) = ticket::verify(&st, "file", share_id, &rel, q.exp, &q.sig) {
        return e.into_response();
    }
    serve_download(&st, &headers, share_id, &rel).await
}

/// /dl/ 统一下载端点:带签名 → 验签免登录放行;无签名 → 按会话鉴权(游客可见文件免签)。
/// axum 已对通配捕获做一次百分号解码,不再二次解码(否则文件名含 % 时会错)。
pub async fn dl_short(
    State(st): State<AppState>,
    identity: Identity,
    headers: HeaderMap,
    Path(rest): Path<String>,
    Query(q): Query<SigQ>,
) -> Response {
    let rest = rest.trim_start_matches('/');
    let Some((share, rel)) = rest.split_once('/') else {
        return super::not_found("共享不存在");
    };
    let Ok(share_id) = share.parse::<i64>() else {
        return super::not_found("共享不存在");
    };
    if !q.sig.is_empty() {
        if let Err(e) = ticket::verify(&st, "file", share_id, rel, q.exp, &q.sig) {
            return e.into_response();
        }
    } else {
        let spot = match locate(&st, share_id, rel).await {
            Ok(s) => s,
            Err(e) => return e.into_response(),
        };
        if let Err(e) = perms::ensure_download(&identity, &spot.share, &spot.rules, &spot.rel) {
            return e.into_response();
        }
    }
    serve_download(&st, &headers, share_id, rel).await
}

#[derive(Deserialize)]
pub struct SigQ {
    #[serde(default)]
    path: String,
    #[serde(default)]
    exp: i64,
    #[serde(default)]
    sig: String,
}

#[derive(Deserialize)]
pub struct TicketReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    names: Vec<String>,
}

/// 复制下载链接:单文件签直链(游客可见文件用免签短链),多选签打包链接。
pub async fn dlticket(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<TicketReq>,
) -> ApiResult<Response> {
    let url = if req.names.is_empty() {
        let spot = locate(&st, req.share_id, &req.path).await?;
        perms::ensure_download(&identity, &spot.share, &spot.rules, &spot.rel)?;
        if !spot.full.is_file() {
            return Err(ApiError::not_found("文件不存在"));
        }
        let guest_ok = !perms::is_private(&spot.share)
            && perms::level_of(&spot.rules, &spot.rel) == perms::Level::Guest;
        ticket::file_link(&st, spot.share.id, &spot.rel, !guest_ok)
    } else {
        let spot = locate(&st, req.share_id, &req.path).await?;
        for name in &req.names {
            let Ok(name) = paths::check_name(name) else {
                continue;
            };
            let rel = if spot.rel.is_empty() {
                name
            } else {
                format!("{}/{}", spot.rel, name)
            };
            perms::ensure_download(&identity, &spot.share, &spot.rules, &rel)?;
        }
        ticket::zip_link(&st, spot.share.id, &spot.rel, &req.names)
    };
    Ok(ok(json!({ "url": url, "expires_in": ticket::LINK_TTL })))
}

#[derive(Deserialize)]
pub struct ZipReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    names: Vec<String>,
}

/// POST 批量打包下载(按当前会话鉴权)。
pub async fn zip_post(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<ZipReq>,
) -> Response {
    match zip_response(&st, &identity, req.share_id, &req.path, &req.names, false).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ZipGetQ {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    exp: i64,
    #[serde(default)]
    sig: String,
}

/// GET 签名打包下载。文件名可能本身含逗号,必须按原始查询串切分再逐个解码。
pub async fn zip_get(
    State(st): State<AppState>,
    identity: Identity,
    Query(q): Query<ZipGetQ>,
    raw_q: RawQuery,
) -> Response {
    let names = raw_names(&raw_q);
    let payload = format!("{}|{}", q.path, names.join(","));
    if let Err(e) = ticket::verify(&st, "zip", q.share_id, &payload, q.exp, &q.sig) {
        return e.into_response();
    }
    match zip_response(&st, &identity, q.share_id, &q.path, &names, true).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

fn raw_names(raw_q: &RawQuery) -> Vec<String> {
    raw_q
        .0
        .as_deref()
        .and_then(|raw| {
            raw.split('&').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                (k == "names").then_some(v)
            })
        })
        .map(|v| {
            v.split(',')
                .map(|s| {
                    percent_encoding::percent_decode_str(s)
                        .decode_utf8_lossy()
                        .to_string()
                })
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

async fn zip_response(
    st: &AppState,
    identity: &Identity,
    share_id: i64,
    path: &str,
    names: &[String],
    signed: bool,
) -> ApiResult<Response> {
    let spot = locate(st, share_id, path).await?;
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for name in names {
        let Ok(name) = paths::check_name(name) else {
            continue;
        };
        let full = spot.full.join(&name);
        let rel = if spot.rel.is_empty() {
            name
        } else {
            format!("{}/{}", spot.rel, name)
        };
        if full.is_dir() {
            fsx::collect_files(&full, &rel, &mut files, 0);
        } else if full.is_file() {
            files.push((rel, full));
        }
    }
    if files.is_empty() {
        return Err(ApiError::not_found("没有可打包的文件"));
    }
    if !signed {
        for (rel, _) in &files {
            perms::ensure_download(identity, &spot.share, &spot.rules, rel)?;
        }
    }
    // 打包总体积上限(磁盘临时文件,内存占用恒定)
    const MAX_ZIP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let mut total: u64 = 0;
    for (_, p) in &files {
        total += tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0);
    }
    if total > MAX_ZIP_BYTES {
        return Err(ApiError::bad_request(format!(
            "打包内容超过上限({} MB),请分批下载",
            MAX_ZIP_BYTES / 1024 / 1024
        )));
    }
    crate::domain::uplink::ensure_disk_space(&st.tmp_dir, total as i64)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i64)
        .unwrap_or(0);
    let tmp_out = st.tmp_dir.join(format!(
        "zip-{}.zip",
        crate::state::now_secs() * 1000 + nanos
    ));
    let tmp_build = tmp_out.clone();
    let size = tokio::task::spawn_blocking(move || fsx::build_zip_to_file(&files, &tmp_build))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(ApiError::internal)?;
    let file = match tokio::fs::File::open(&tmp_out).await {
        Ok(f) => f,
        Err(_) => return Err(ApiError::internal("打包临时文件丢失")),
    };
    // 流式发送,EOF 时自动删除临时文件
    let stream = ZipCleanupStream {
        inner: Box::pin(tokio_util::io::ReaderStream::new(file)),
        tmp: Some(tmp_out),
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/zip")
        .header("Content-Disposition", "attachment; filename=\"batch.zip\"")
        .header("Content-Length", size.to_string())
        .body(axum::body::Body::from_stream(stream))
        .unwrap())
}

/// 包装 ReaderStream:流结束后删除临时打包文件(包括客户端中断连接的情况)。
struct ZipCleanupStream {
    inner: std::pin::Pin<Box<tokio_util::io::ReaderStream<tokio::fs::File>>>,
    tmp: Option<PathBuf>,
}

impl futures_core::Stream for ZipCleanupStream {
    type Item = Result<axum::body::Bytes, std::io::Error>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = &mut *self;
        let r = this.inner.as_mut().poll_next(cx);
        if matches!(r, std::task::Poll::Ready(None)) {
            if let Some(p) = this.tmp.take() {
                let _ = std::fs::remove_file(p);
            }
        }
        r
    }
}

impl Drop for ZipCleanupStream {
    fn drop(&mut self) {
        // 客户端中途断开时 body 被丢弃:立即删除临时文件
        if let Some(p) = self.tmp.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}
