//! 请求上下文工具:共享定位(Spot)、通用文件流式响应、Range 解析、
//! 共享目录内条目的可管理性判定。

use std::io::SeekFrom;
use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::app::App;
use crate::db::{AccessRule, Share, User};
use crate::domain::access as perms;
use crate::domain::pathguard;
use crate::error::{ApiError, ApiResult};
use crate::web::identity::Identity;

/// 一次"共享 + 规则 + 规范化相对路径 + 绝对路径"的定位结果。
pub struct Spot {
    pub share: Share,
    pub rules: Vec<AccessRule>,
    pub rel: String,
    pub full: PathBuf,
}

pub async fn locate(app: &App, share_id: i64, rel: &str) -> ApiResult<Spot> {
    let share = app.db.share(share_id).await?.ok_or_else(|| ApiError::not_found("共享不存在"))?;
    let root = PathBuf::from(&share.host_path);
    if !root.is_dir() {
        return Err(ApiError::internal("共享根目录不可用"));
    }
    let rel = pathguard::normalize(rel).map_err(ApiError::bad_request)?;
    let full = pathguard::join_under(&root, &rel).map_err(ApiError::bad_request)?;
    let rules = app.db.rules_of(share_id).await?;
    Ok(Spot { share, rules, rel, full })
}

/// 定位并要求可读。
pub async fn locate_readable(app: &App, identity: &Identity, share_id: i64, rel: &str) -> ApiResult<Spot> {
    let spot = locate(app, share_id, rel).await?;
    perms::ensure_read(identity.as_user(), &spot.share, &spot.rules, &spot.rel)?;
    Ok(spot)
}

/// 定位并要求可写(叠加账号能力开关 `flag`)。
pub async fn locate_writable(
    app: &App,
    identity: &Identity,
    share_id: i64,
    rel: &str,
    flag: impl Fn(&User) -> bool + Send,
    what: &str,
) -> ApiResult<Spot> {
    let spot = locate(app, share_id, rel).await?;
    perms::ensure_write(identity.as_user(), &spot.share, &spot.rules, &spot.rel, flag, what)?;
    Ok(spot)
}

pub fn file_name_of(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_string()
}

pub fn child_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// 共享目录内条目可否管理:管理员与个人空间属主可管理一切;
/// 其它用户只能管理"完全由本人创建"的条目(file_meta 归属)。
pub async fn can_manage(app: &App, identity: &Identity, share: &Share, rel: &str) -> bool {
    let Some(u) = identity.user.as_ref() else { return false };
    if u.is_admin || share.kind == "home" {
        return true;
    }
    match app.db.meta_owners_under(share.id, rel).await {
        Ok(owners) => !owners.is_empty() && owners.iter().all(|(_, oid)| *oid == u.id),
        Err(_) => false,
    }
}

/// 同 `can_manage`,但给出更具体的错误信息(区分"非本人创建"与"混有他人内容")。
pub async fn ensure_manageable(app: &App, identity: &Identity, share: &Share, rel: &str) -> ApiResult<()> {
    let Some(u) = identity.user.as_ref() else {
        return Err(ApiError::unauthorized("需要登录"));
    };
    if u.is_admin || share.kind == "home" {
        return Ok(());
    }
    let owners = app.db.meta_owners_under(share.id, rel).await?;
    if owners.is_empty() {
        return Err(ApiError::forbidden("该内容不是由你创建的,只有管理员可以修改或删除"));
    }
    if owners.iter().any(|(_, oid)| *oid != u.id) {
        return Err(ApiError::forbidden("所选文件夹内包含他人创建的内容,无法整体操作"));
    }
    Ok(())
}

/// 解析单个 `Range: bytes=a-b` 头;多段范围与越界范围返回 None(退回 200/416)。
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
    format!("attachment; {ascii}; filename*=UTF-8''{}", crate::domain::ticket::urlencode(&safe))
}

/// 通用文件流式响应:Range(206)、HEAD(无 body)、inline/attachment、
/// `nosniff` 与 CSP sandbox(防直链打开恶意 HTML)。
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
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT, Some(format!("bytes {s}-{e}/{size}"))),
        None => {
            if range.is_some() {
                let mut resp = Response::new(Body::empty());
                *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                resp.headers_mut().insert("Content-Range", format!("bytes */{size}").parse().unwrap());
                return resp;
            }
            (0, size.saturating_sub(1), StatusCode::OK, None)
        }
    };
    let name = if display_name.is_empty() {
        file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
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
    headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(parse_range("bytes=1000-", 1000), None);
    }

    #[test]
    fn child_rel_join() {
        assert_eq!(child_rel("", "a"), "a");
        assert_eq!(child_rel("x", "a"), "x/a");
    }

    #[test]
    fn disposition_attachment_for_utf8() {
        let d = disposition(false, "文件.txt");
        assert!(d.starts_with("attachment;"));
        assert!(d.contains("filename*=UTF-8''"));
        let d2 = disposition(true, "x.txt");
        assert_eq!(d2, "inline");
    }
}
