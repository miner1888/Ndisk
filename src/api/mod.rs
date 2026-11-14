pub mod admin;
pub mod auth;
pub mod dav;
pub mod files;
pub mod links;
pub mod media;
pub mod misc;
pub mod router;
pub mod shares;
pub mod tls;
pub mod uplink;
pub mod users;

use std::io::SeekFrom;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::db::{AccessRule, Share};
use crate::domain::auth::Identity;
use crate::domain::{paths, perms};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// 一次请求定位出的目标:共享 + 规则 + 物理路径 + 内部相对路径。
pub struct Spot {
    pub share: Share,
    pub rules: Vec<AccessRule>,
    pub rel: String,
    pub full: PathBuf,
}

/// 定位共享与路径:共享不存在 404,根目录不可用 500,路径非法 / 越界 400。
pub async fn locate(st: &AppState, share_id: i64, rel: &str) -> ApiResult<Spot> {
    let share = st
        .db
        .share(share_id)
        .await?
        .ok_or_else(|| ApiError::not_found("共享不存在"))?;
    let root = PathBuf::from(&share.host_path);
    if !root.is_dir() {
        return Err(ApiError::internal("共享根目录不可用"));
    }
    let rel = paths::normalize(rel).map_err(ApiError::bad_request)?;
    let full = paths::join_under(&root, &rel).map_err(ApiError::bad_request)?;
    let rules = st.db.rules_of(share_id).await?;
    Ok(Spot {
        share,
        rules,
        rel,
        full,
    })
}

/// 定位 + 读权限校验一步到位。
pub async fn locate_readable(
    st: &AppState,
    identity: &Identity,
    share_id: i64,
    rel: &str,
) -> ApiResult<Spot> {
    let spot = locate(st, share_id, rel).await?;
    perms::ensure_read(identity, &spot.share, &spot.rules, &spot.rel)?;
    Ok(spot)
}

/// 定位 + 写权限校验一步到位(`what` 用于错误提示,如"上传"/"重命名")。
pub async fn locate_writable(
    st: &AppState,
    identity: &Identity,
    share_id: i64,
    rel: &str,
    flag: impl Fn(&crate::db::User) -> bool + Send,
    what: &str,
) -> ApiResult<Spot> {
    let spot = locate(st, share_id, rel).await?;
    perms::ensure_write(identity, &spot.share, &spot.rules, &spot.rel, flag, what)?;
    Ok(spot)
}

pub fn file_name_of(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_string()
}

/// 共享内相对路径拼接。
pub fn child_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// 当前用户能否管理(修改/删除)该条目:管理员与个人空间属主恒可,公共目录限自己创建的。
pub async fn can_manage(st: &AppState, identity: &Identity, share: &Share, rel: &str) -> bool {
    let Some(u) = identity.user.as_ref() else {
        return false;
    };
    if u.is_admin || share.kind == "home" {
        return true;
    }
    match st.db.meta_owners_under(share.id, rel).await {
        Ok(owners) => !owners.is_empty() && owners.iter().all(|(_, oid)| *oid == u.id),
        Err(_) => false,
    }
}

/// 公共(共享)目录内的条目归属校验:普通用户只能修改/删除自己创建的条目,
/// 管理员可以管理全部;个人空间由空间归属决定,不做此限制。
pub async fn ensure_manageable(
    st: &AppState,
    identity: &Identity,
    share: &Share,
    rel: &str,
) -> ApiResult<()> {
    let Some(u) = identity.user.as_ref() else {
        return Err(ApiError::unauthorized("需要登录"));
    };
    if u.is_admin || share.kind == "home" {
        return Ok(());
    }
    let owners = st.db.meta_owners_under(share.id, rel).await?;
    if owners.is_empty() {
        return Err(ApiError::forbidden(
            "该内容不是由你创建的,只有管理员可以修改或删除",
        ));
    }
    if owners.iter().any(|(_, oid)| *oid != u.id) {
        return Err(ApiError::forbidden(
            "所选文件夹内包含他人创建的内容,无法整体操作",
        ));
    }
    Ok(())
}

/// 解析单个 Range 头(仅支持单段);非法 / 不可满足返回 None。
pub fn parse_range(range: &str, size: u64) -> Option<(u64, u64)> {
    let spec = range.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let n: u64 = end.parse().ok()?;
        if n == 0 || size == 0 {
            return None;
        }
        return Some((size.saturating_sub(n), size - 1));
    }
    let s: u64 = start.parse().ok()?;
    if s >= size {
        return None;
    }
    let e = match end.parse::<u64>() {
        Ok(v) => v.min(size - 1),
        Err(_) => size - 1,
    };
    // RFC 9110: 末位小于首位视为不可满足(否则下游 u64 下溢)
    if e < s {
        return None;
    }
    Some((s, e))
}

fn disposition(inline: bool, name: &str) -> String {
    if inline {
        return "inline".into();
    }
    let safe = name.replace(['"', '\\'], "_");
    let ascii = if safe.is_ascii() {
        format!("filename=\"{safe}\"")
    } else {
        "filename=\"download\"".into()
    };
    format!(
        "attachment; {ascii}; filename*=UTF-8''{}",
        crate::domain::ticket::urlencode(&safe)
    )
}

/// 支持 Range 的文件流式响应;统一附加 CSP sandbox 与 nosniff,
/// 防止用户上传内容(尤其 SVG/HTML)在站点源内执行脚本。
pub async fn stream_file(
    file: &std::path::Path,
    range: Option<&str>,
    mime: &str,
    inline: bool,
    want_body: bool,
    display_name: &str,
) -> Response {
    let Ok(meta) = tokio::fs::metadata(file).await else {
        return ApiError::not_found("文件不存在").into_response();
    };
    let size = meta.len();
    let (start, end, status, content_range) = match range.and_then(|r| parse_range(r, size)) {
        Some((s, e)) => (
            s,
            e,
            StatusCode::PARTIAL_CONTENT,
            Some(format!("bytes {s}-{e}/{size}")),
        ),
        None => {
            if range.is_some() {
                let mut resp = Response::new(Body::empty());
                *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                resp.headers_mut()
                    .insert("Content-Range", format!("bytes */{size}").parse().unwrap());
                return resp;
            }
            (0, size.saturating_sub(1), StatusCode::OK, None)
        }
    };
    let name = if display_name.is_empty() {
        file.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        display_name.to_string()
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut resp = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disposition(inline, &name))
        .header("Content-Security-Policy", "sandbox")
        .header("X-Content-Type-Options", "nosniff");
    if let Some(cr) = content_range {
        resp = resp.header("Content-Range", cr);
    }
    resp = resp.header(header::CONTENT_LENGTH, length.to_string());
    if !want_body || size == 0 {
        return resp.body(Body::empty()).unwrap();
    }
    let Ok(mut f) = tokio::fs::File::open(file).await else {
        return ApiError::internal("读取失败").into_response();
    };
    if start > 0 {
        let _ = f.seek(SeekFrom::Start(start)).await;
    }
    let body = Body::from_stream(ReaderStream::with_capacity(f.take(length), 64 * 1024));
    resp.body(body).unwrap()
}

pub fn not_found(m: &str) -> Response {
    ApiError::not_found(m.to_string()).into_response()
}

pub fn headers_range(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}
