//! 文件浏览与操作:列表 / 详情 / 相册 / 新建 / 重命名 / 移动 / 删除 /
//! 文本与二进制保存 / 下载(签名直链与短链)/ 下载票据 / ZIP 打包。

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::{now_secs, App};
use crate::db::User;
use crate::domain::{access as perms, fsx, media, pathguard, ticket, zipx as archive};
use crate::error::{ok, ApiError, ApiResult};
use crate::web::ctx::{can_manage, child_rel, ensure_manageable, file_name_of, headers_range, locate, locate_readable, locate_writable, stream_file};
use crate::web::identity::Identity;

#[derive(Deserialize)]
pub struct PathQ {
    #[serde(default)]
    path: String,
}

pub async fn browse(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&app, &identity, share_id, &q.path).await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("目录不存在"));
    }
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(&spot.full).map_err(|e| ApiError::internal(format!("读取失败: {e}")))?;
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let child = child_rel(&spot.rel, &name);
        if !perms::can_read(identity.as_user(), &spot.share, &spot.rules, &child) {
            continue;
        }
        let md = de.metadata().ok();
        let is_dir = md.as_ref().is_some_and(|m| m.is_dir());
        let ext = if is_dir { String::new() } else { pathguard::ext_of(&name) };
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
    // 目录在前,名称不区分大小写排序。
    entries.sort_by(|a, b| {
        let da = a["is_dir"].as_bool().unwrap();
        let db = b["is_dir"].as_bool().unwrap();
        db.cmp(&da).then_with(|| {
            a["name"].as_str().unwrap().to_lowercase().cmp(&b["name"].as_str().unwrap().to_lowercase())
        })
    });
    let can = |flag: fn(&User) -> bool| perms::can_write(identity.as_user(), &spot.share, &spot.rules, &spot.rel, flag);
    Ok(ok(json!({
        "path": spot.rel,
        "share": { "id": spot.share.id, "name": spot.share.name, "owner_id": spot.share.owner_id, "home": spot.share.kind == "home" },
        "can": {
            "upload": can(|u| u.can_upload),
            "mkdir": can(|u| u.can_mkdir),
            "delete": can(|u| u.can_delete),
            "download": perms::can_download(identity.as_user(), &spot.share, &spot.rules, &spot.rel),
        },
        "entries": entries,
    })))
}

pub async fn meta(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate(&app, share_id, &q.path).await?;
    perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, &spot.rel)?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let md = tokio::fs::metadata(&spot.full).await?;
    let ext = pathguard::ext_of(&spot.rel);
    Ok(ok(json!({
        "share_id": share_id,
        "path": spot.rel,
        "name": file_name_of(&spot.rel),
        "size": md.len() as i64,
        "mtime": fsx::unix_secs(md.modified()),
        "ext": ext,
        "mime": media::mime_of(&ext),
        "kind": media::kind_label(media::kind_of(&ext)),
        "writable": perms::can_write(identity.as_user(), &spot.share, &spot.rules, &spot.rel, |u| u.can_upload)
            && can_manage(&app, &identity, &spot.share, &spot.rel).await,
    })))
}

pub async fn detail(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&app, &identity, share_id, &q.path).await?;
    let md = tokio::fs::metadata(&spot.full).await.map_err(|e| ApiError::not_found(format!("读取失败: {e}")))?;
    let mtime = fsx::unix_secs(md.modified());
    let created = std::cmp::min(
        if md.created().is_ok() { fsx::unix_secs(md.created()) } else { mtime },
        mtime,
    );
    let ext = pathguard::ext_of(&spot.rel);
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

const MAX_ALBUM: usize = 5000;

/// 相册:递归聚合个人空间内全部图片与视频,按修改时间倒序。
pub async fn album(State(app): State<App>, identity: Identity, Path(share_id): Path<i64>) -> ApiResult<Response> {
    let spot = locate_readable(&app, &identity, share_id, "").await?;
    if spot.share.kind != "home" {
        return Err(ApiError::bad_request("相册仅适用于个人空间"));
    }
    let root = spot.full.clone();
    let mut images = tokio::task::spawn_blocking(move || collect_media(&root, "", 0))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    images.sort_by_key(|x| std::cmp::Reverse(x.2));
    images.truncate(MAX_ALBUM);
    let out: Vec<_> = images
        .into_iter()
        .map(|(rel, size, mtime, kind)| {
            json!({
                "path": rel,
                "name": file_name_of(&rel),
                "size": size,
                "mtime": mtime,
                "kind": kind,
            })
        })
        .collect();
    Ok(ok(json!({ "count": out.len(), "images": out })))
}

/// 递归收集图片 / 视频(基于 fsx::walk 的薄封装)。
fn collect_media(dir: &std::path::Path, rel: &str, depth: u32) -> Vec<(String, i64, i64, &'static str)> {
    let mut out = Vec::new();
    fsx::walk(
        dir,
        rel,
        depth,
        24,
        &mut |child, path, meta| {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let kind = media::kind_of(&pathguard::ext_of(&name));
            if matches!(kind, media::Kind::Image | media::Kind::Video) {
                let label = if kind == media::Kind::Video { "video" } else { "image" };
                out.push((child.to_string(), meta.len() as i64, fsx::unix_secs(meta.modified()), label));
            }
        },
        &mut |_rel, _path| true,
    );
    out
}

#[derive(Deserialize)]
pub struct MkdirReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    name: String,
}

pub async fn mkdir(State(app): State<App>, identity: Identity, Json(req): Json<MkdirReq>) -> ApiResult<Response> {
    let spot = locate_writable(&app, &identity, req.share_id, &req.path, |u| u.can_mkdir, "新建文件夹").await?;
    if !spot.full.is_dir() {
        return Err(ApiError::not_found("父目录不存在"));
    }
    let name = pathguard::check_name(&req.name).map_err(ApiError::bad_request)?;
    match std::fs::create_dir(spot.full.join(&name)) {
        Ok(()) => {
            if let Some(u) = identity.as_user() {
                let _ = app.db.set_meta_owner(spot.share.id, &child_rel(&spot.rel, &name), u.id).await;
            }
            Ok(ok(json!({ "ok": true })))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(ok(json!({ "ok": true, "exists": true }))),
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

pub async fn rename(State(app): State<App>, identity: Identity, Json(req): Json<RenameReq>) -> ApiResult<Response> {
    let spot = locate_writable(&app, &identity, req.share_id, &req.path, |u| u.can_delete, "重命名").await?;
    let old = pathguard::check_name(&req.old).map_err(ApiError::bad_request)?;
    let new = pathguard::check_name(&req.new).map_err(ApiError::bad_request)?;
    let from = spot.full.join(&old);
    let to = spot.full.join(&new);
    if !from.exists() {
        return Err(ApiError::not_found("原文件不存在"));
    }
    if to.exists() {
        return Err(ApiError::conflict("目标已存在"));
    }
    let old_rel = child_rel(&spot.rel, &old);
    let new_rel = child_rel(&spot.rel, &new);
    ensure_manageable(&app, &identity, &spot.share, &old_rel).await?;
    std::fs::rename(&from, &to).map_err(|e| ApiError::internal(format!("重命名失败: {e}")))?;
    let _ = app.db.rename_meta(spot.share.id, &old_rel, &new_rel).await;
    if !from.is_dir() {
        media::purge(&app.db, &app.media_dir, req.share_id, &old_rel).await;
    }
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct MoveReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    dest: String,
}

pub async fn move_items(State(app): State<App>, identity: Identity, Json(req): Json<MoveReq>) -> ApiResult<Response> {
    if req.names.is_empty() {
        return Err(ApiError::bad_request("未选择要移动的内容"));
    }
    let src = locate_writable(&app, &identity, req.share_id, &req.path, |u| u.can_delete, "移动").await?;
    let dst = locate_writable(&app, &identity, req.share_id, &req.dest, |u| u.can_delete, "移动").await?;
    if !dst.full.is_dir() {
        return Err(ApiError::not_found("目标文件夹不存在"));
    }
    // 先整体校验(重名 / 自嵌套 / 目标冲突),再逐个执行。
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for raw in &req.names {
        let name = pathguard::check_name(raw).map_err(ApiError::bad_request)?;
        if !seen.insert(name.clone()) {
            continue;
        }
        let from = src.full.join(&name);
        if !from.exists() {
            return Err(ApiError::not_found(format!("“{name}” 不存在")));
        }
        let src_rel = child_rel(&src.rel, &name);
        let dst_rel = child_rel(&dst.rel, &name);
        if src_rel == dst_rel {
            continue;
        }
        if dst_rel.starts_with(&format!("{src_rel}/")) {
            return Err(ApiError::bad_request(format!("不能把 “{name}” 移动到它自己内部")));
        }
        let to = dst.full.join(&name);
        if to.exists() {
            return Err(ApiError::conflict(format!("目标文件夹里已有同名 “{name}”")));
        }
        ensure_manageable(&app, &identity, &src.share, &src_rel).await?;
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
                let _ = app.db.rename_meta(req.share_id, &src_rel, &dst_rel).await;
                if !from.is_dir() {
                    media::purge(&app.db, &app.media_dir, req.share_id, &src_rel).await;
                }
                moved += 1;
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    if moved == 0 && !errors.is_empty() {
        return Err(ApiError::internal(format!("移动失败: {}", errors.join(";"))));
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

pub async fn delete(State(app): State<App>, identity: Identity, Json(req): Json<DeleteReq>) -> ApiResult<Response> {
    let spot = locate_writable(&app, &identity, req.share_id, &req.path, |u| u.can_delete, "删除").await?;
    if !spot.full.exists() {
        return Err(ApiError::not_found("目标不存在"));
    }
    if spot.rel.is_empty() {
        return Err(ApiError::bad_request("不能删除共享根目录"));
    }
    ensure_manageable(&app, &identity, &spot.share, &spot.rel).await?;
    let removed = if spot.full.is_dir() {
        if !req.recursive {
            let mut it = std::fs::read_dir(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            if it.next().is_some() {
                return Err(ApiError::conflict("文件夹非空,需要确认递归删除"));
            }
            std::fs::remove_dir(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            1
        } else {
            // 先逐个回收派生缓存,再整树删除。
            let mut files = Vec::new();
            fsx::collect_files(&spot.full, &spot.rel, &mut files, 0);
            for (rel, _) in &files {
                media::purge(&app.db, &app.media_dir, req.share_id, rel).await;
            }
            let mut n = 1usize;
            fsx::count_entries(&spot.full, &mut n);
            std::fs::remove_dir_all(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
            n as i64
        }
    } else {
        std::fs::remove_file(&spot.full).map_err(|e| ApiError::internal(e.to_string()))?;
        media::purge(&app.db, &app.media_dir, req.share_id, &spot.rel).await;
        1
    };
    let _ = app.db.drop_meta(spot.share.id, &spot.rel).await;
    Ok(ok(json!({ "ok": true, "removed": removed })))
}

const TEXT_CAP: usize = 8 * 1024 * 1024;

pub async fn text_get(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&app, &identity, share_id, &q.path).await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let ext = pathguard::ext_of(&spot.rel);
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
        "editable": perms::can_write(identity.as_user(), &spot.share, &spot.rules, &spot.rel, |u| u.can_upload)
            && can_manage(&app, &identity, &spot.share, &spot.rel).await,
        "ext": ext,
    })))
}

#[derive(Deserialize)]
pub struct TextSaveReq {
    content: String,
}

pub async fn text_put(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
    Json(req): Json<TextSaveReq>,
) -> ApiResult<Response> {
    let spot = locate_writable(&app, &identity, share_id, &q.path, |u| u.can_upload, "保存文件").await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    ensure_manageable(&app, &identity, &spot.share, &spot.rel).await?;
    let ext = pathguard::ext_of(&spot.rel);
    if !media::editable_text(&ext) {
        return Err(ApiError::bad_request("该类型不支持在线编辑"));
    }
    if req.content.len() > TEXT_CAP {
        return Err(ApiError::bad_request("内容过大"));
    }
    fsx::atomic_write(&spot.full, req.content.as_bytes()).await.map_err(ApiError::internal)?;
    Ok(ok(json!({ "ok": true, "size": req.content.len() })))
}

const BIN_CAP: usize = 32 * 1024 * 1024;

/// 在线保存表格二进制(xlsx/xls/ods)。
pub async fn binary_put(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<PathQ>,
    body: Bytes,
) -> ApiResult<Response> {
    let spot = locate_writable(&app, &identity, share_id, &q.path, |u| u.can_upload, "保存文件").await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    ensure_manageable(&app, &identity, &spot.share, &spot.rel).await?;
    let ext = pathguard::ext_of(&spot.rel);
    if !media::is_sheet(&ext) {
        return Err(ApiError::bad_request("该类型不支持在线保存"));
    }
    if body.len() > BIN_CAP {
        return Err(ApiError::bad_request("文件过大"));
    }
    fsx::atomic_write(&spot.full, &body).await.map_err(ApiError::internal)?;
    Ok(ok(json!({ "ok": true, "size": body.len() })))
}

async fn serve_download(app: &App, headers: &HeaderMap, share_id: i64, rel: &str) -> Response {
    let Ok(spot) = locate(app, share_id, rel).await else {
        return crate::web::ctx::not_found("文件不存在或已失效");
    };
    if !spot.full.is_file() {
        return crate::web::ctx::not_found("文件不存在或已失效");
    }
    let ext = pathguard::ext_of(&spot.rel);
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

#[derive(Deserialize)]
pub struct SigQ {
    #[serde(default)]
    path: String,
    #[serde(default)]
    exp: i64,
    #[serde(default)]
    sig: String,
}

/// 签名直链:/api/dl/{share_id}/{*name}(path 查询参数兼容旧格式)。
pub async fn dl_signed(
    State(app): State<App>,
    headers: HeaderMap,
    Path((share_id, name)): Path<(i64, String)>,
    Query(q): Query<SigQ>,
) -> Response {
    let rel = if q.path.is_empty() { name } else { q.path.clone() };
    if let Err(e) = ticket::verify(&app.dl_key, "file", share_id, &rel, q.exp, &q.sig) {
        return e.into_response();
    }
    serve_download(&app, &headers, share_id, &rel).await
}

/// 短链:/dl/{share_id}/{path};带签名免登录,不带签名则要求下载权限。
pub async fn dl_short(
    State(app): State<App>,
    identity: Identity,
    headers: HeaderMap,
    Path(rest): Path<String>,
    Query(q): Query<SigQ>,
) -> Response {
    let rest = rest.trim_start_matches('/');
    let Some((share, rel)) = rest.split_once('/') else {
        return crate::web::ctx::not_found("共享不存在");
    };
    let Ok(share_id) = share.parse::<i64>() else {
        return crate::web::ctx::not_found("共享不存在");
    };
    if !q.sig.is_empty() {
        if let Err(e) = ticket::verify(&app.dl_key, "file", share_id, rel, q.exp, &q.sig) {
            return e.into_response();
        }
    } else {
        let spot = match locate(&app, share_id, rel).await {
            Ok(s) => s,
            Err(e) => return e.into_response(),
        };
        if let Err(e) = perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, &spot.rel) {
            return e.into_response();
        }
    }
    serve_download(&app, &headers, share_id, rel).await
}

#[derive(Deserialize)]
pub struct TicketReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    names: Vec<String>,
}

/// 领取下载票据:单文件 → 免登录直链;多选 → 打包直链。
/// 游客可读的共享不签名(短链),其余附签名(7 天有效)。
pub async fn dlticket(State(app): State<App>, identity: Identity, Json(req): Json<TicketReq>) -> ApiResult<Response> {
    let url = if req.names.is_empty() {
        let spot = locate(&app, req.share_id, &req.path).await?;
        perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, &spot.rel)?;
        if !spot.full.is_file() {
            return Err(ApiError::not_found("文件不存在"));
        }
        let guest_ok = !perms::is_private(&spot.share) && perms::level_of(&spot.rules, &spot.rel) == perms::Level::Guest;
        ticket::file_link(&app.dl_key, spot.share.id, &spot.rel, !guest_ok)
    } else {
        let spot = locate(&app, req.share_id, &req.path).await?;
        for name in &req.names {
            let name = pathguard::check_name(name).map_err(ApiError::bad_request)?;
            let rel = if spot.rel.is_empty() { name } else { format!("{}/{}", spot.rel, name) };
            perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, &rel)?;
        }
        ticket::zip_link(&app.dl_key, spot.share.id, &spot.rel, &req.names)
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

pub async fn zip_post(State(app): State<App>, identity: Identity, Json(req): Json<ZipReq>) -> Response {
    match zip_response(&app, &identity, req.share_id, &req.path, &req.names, false).await {
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

/// 签名打包直链。names 需要从原始查询串提取(避免 serde 丢重名参数)。
pub async fn zip_get(State(app): State<App>, identity: Identity, Query(q): Query<ZipGetQ>, raw_q: RawQuery) -> Response {
    let names = raw_names(&raw_q);
    let payload = format!("{}|{}", q.path, names.join(","));
    if let Err(e) = ticket::verify(&app.dl_key, "zip", q.share_id, &payload, q.exp, &q.sig) {
        return e.into_response();
    }
    match zip_response(&app, &identity, q.share_id, &q.path, &names, true).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

/// 从原始查询串提取 names(签名 URL 可能含重名参数,serde 会丢弃,需手工解析)。
fn raw_names(raw_q: &RawQuery) -> Vec<String> {
    let raw = raw_q.0.as_deref().unwrap_or("");
    url::form_urlencoded::parse(raw.as_bytes())
        .find(|(k, _)| k == "names")
        .map(|(_, v)| {
            v.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

async fn zip_response(
    app: &App,
    identity: &Identity,
    share_id: i64,
    path: &str,
    names: &[String],
    signed: bool,
) -> ApiResult<Response> {
    let spot = locate(app, share_id, path).await?;
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for name in names {
        let Ok(name) = pathguard::check_name(name) else { continue };
        let full = spot.full.join(&name);
        let rel = if spot.rel.is_empty() { name } else { format!("{}/{}", spot.rel, name) };
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
            perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, rel)?;
        }
    }
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
    crate::domain::chunkup::ensure_disk_space(&app.tmp_dir, total as i64)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as i64)
        .unwrap_or(0);
    let tmp_out = app.tmp_dir.join(format!("zip-{}.zip", now_secs() * 1000 + nanos));
    let tmp_build = tmp_out.clone();
    let size = tokio::task::spawn_blocking(move || archive::build_zip_to_file(&files, &tmp_build))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(ApiError::internal)?;
    let file = match tokio::fs::File::open(&tmp_out).await {
        Ok(f) => f,
        Err(_) => return Err(ApiError::internal("打包临时文件丢失")),
    };
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

/// 发送完成或客户端中断时删除临时 ZIP。
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
        if let Some(p) = self.tmp.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}
