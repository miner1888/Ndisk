//! WebDAV:`/dav/` 根即当前账号的个人空间,管理员发布的共享目录以名称列在根下。
//! 支持 OPTIONS / PROPFIND / GET / HEAD / PUT / MKCOL / DELETE / MOVE /
//! COPY / PROPPATCH / LOCK / UNLOCK;权限与网页端一致(Basic 或 Bearer 认证)。
//!
//! 拆分:动词分发与文件操作在本文件;XML 属性 / 日期格式化在 `props`。

pub mod props;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures_core::Stream as _;

use crate::app::{now_secs, App};
use crate::db::{AccessRule, Share};
use crate::domain::{access as perms, auth, chunkup, fsx, media};
use crate::error::{ApiError, ApiResult};
use crate::web::ctx::{child_rel, ensure_manageable, file_name_of, locate, stream_file, Spot};
use crate::web::identity::Identity;

use props::{href_encode, href_of, prop_entry, wrap_multistatus};

const DAV_PUT_MAX: u64 = 256 * 1024 * 1024;

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Basic realm=\"NDisk WebDAV\"")],
        "需要认证:WebDAV 使用 Basic 认证(用户名 + 密码)",
    )
        .into_response()
}

/// WebDAV 认证:Basic(用户名 + 登录密码)或 Bearer(会话 token)。
async fn dav_identity(app: &App, headers: &HeaderMap) -> Result<Identity, Response> {
    let Some(raw) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return Err(unauthorized());
    };
    if let Some(enc) = raw.strip_prefix("Basic ") {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(enc.trim()) {
            if let Ok(s) = String::from_utf8(bytes) {
                if let Some((name, pass)) = s.split_once(':') {
                    if let Ok(Some(user)) = app.db.user_by_name(name).await {
                        if auth::verify_password(pass, &user.password_hash) {
                            return Ok(Identity { user: Some(user), token: None });
                        }
                    }
                }
            }
        }
        return Err(unauthorized());
    }
    if let Some(tok) = raw.strip_prefix("Bearer ") {
        let tok = tok.trim();
        if let Ok(Some(user)) = app.db.user_by_token(tok).await {
            return Ok(Identity { user: Some(user), token: Some(tok.to_string()) });
        }
        return Err(unauthorized());
    }
    Err(unauthorized())
}

fn share_visible(identity: &Identity, share: &Share, rules: &[AccessRule]) -> bool {
    if perms::is_private(share) {
        return identity.as_user().map(|u| perms::is_owner(share, u)).unwrap_or(false);
    }
    match perms::level_of(rules, "") {
        perms::Level::Guest => true,
        perms::Level::Login => identity.as_user().is_some(),
        perms::Level::Admin => identity.is_admin(),
    }
}

async fn visible_shares(app: &App, identity: &Identity) -> ApiResult<Vec<(Share, Vec<AccessRule>)>> {
    let mut out = Vec::new();
    for s in app.db.all_shares().await? {
        let rules = app.db.rules_of(s.id).await?;
        if share_visible(identity, &s, &rules) {
            out.push((s, rules));
        }
    }
    Ok(out)
}

async fn home_share(app: &App, identity: &Identity) -> ApiResult<Option<(Share, Vec<AccessRule>)>> {
    Ok(visible_shares(app, identity)
        .await?
        .into_iter()
        .find(|(s, _)| s.kind == "home" && s.owner_id == identity.as_user().map(|u| u.id)))
}

struct DavTarget {
    share: Share,
    rules: Vec<AccessRule>,
    spot: Spot,
    /// `/dav/` 后第一段(个人空间内路径或共享名)。
    first: String,
}

/// 解析 `/dav/{first}/{rel}`:优先个人空间,其次按名称匹配可见共享。
async fn resolve_target(app: &App, identity: &Identity, rest: &str) -> ApiResult<DavTarget> {
    let (first, rel) = decode_path(rest);
    let home = home_share(app, identity)
        .await?
        .ok_or_else(|| ApiError::forbidden("当前账号没有个人空间"))?;
    let full_rel = if rel.is_empty() { first.clone() } else { child_rel(&first, &rel) };
    if !first.is_empty() {
        let home_spot = locate(app, home.0.id, &full_rel).await?;
        if home_spot.full.exists() {
            return Ok(DavTarget { share: home.0.clone(), rules: home.1.clone(), spot: home_spot, first });
        }
        for (s, r) in visible_shares(app, identity).await? {
            if s.kind != "home" && s.name == first {
                let spot = locate(app, s.id, &rel).await?;
                return Ok(DavTarget { share: s, rules: r, spot, first });
            }
        }
    }
    let spot = locate(app, home.0.id, &full_rel).await?;
    Ok(DavTarget { share: home.0.clone(), rules: home.1.clone(), spot, first })
}

/// `%20` 解码 + 逐段重组。
pub fn decode_path(rest: &str) -> (String, String) {
    let rest = rest.trim_start_matches('/');
    let (name, rel) = match rest.split_once('/') {
        Some((n, r)) => (n, r),
        None => (rest, ""),
    };
    let dec = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string();
    let rel = rel.split('/').filter(|s| !s.is_empty()).map(dec).collect::<Vec<_>>().join("/");
    (dec(name), rel)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn dav_options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            ("DAV", "1, 2"),
            ("MS-Author-Via", "DAV"),
            ("Allow", "OPTIONS, GET, HEAD, PUT, PROPFIND, PROPPATCH, MKCOL, DELETE, MOVE, COPY, LOCK, UNLOCK"),
        ],
        Body::empty(),
    )
        .into_response()
}

// ── 入口 ──

/// `/dav` 与 `/dav/`:根 PROPFIND(个人空间 + 可见共享目录列表)。
pub async fn dav_root(State(app): State<App>, headers: HeaderMap, method: Method) -> Response {
    let identity = match dav_identity(&app, &headers).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    match method.as_str() {
        "OPTIONS" => dav_options(),
        "PROPFIND" => match home_share(&app, &identity).await {
            Ok(Some((home, _))) => {
                let mut entries = vec![prop_entry("/dav/", "NDisk", true, 0, now_secs())];
                if let Ok(home_spot) = locate(&app, home.id, "").await {
                    if let Ok(rd) = std::fs::read_dir(&home_spot.full) {
                        for de in rd.flatten() {
                            let name = de.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') {
                                continue;
                            }
                            let Ok(cm) = de.metadata() else { continue };
                            entries.push(prop_entry(
                                &format!("/dav/{}", href_encode(&name)),
                                &name,
                                cm.is_dir(),
                                if cm.is_dir() { 0 } else { cm.len() },
                                fsx::unix_secs(cm.modified()),
                            ));
                        }
                    }
                }
                for (s, _) in visible_shares(&app, &identity).await.unwrap_or_default() {
                    if s.kind != "home" {
                        entries.push(prop_entry(&format!("/dav/{}/", href_encode(&s.name)), &s.name, true, 0, now_secs()));
                    }
                }
                wrap_multistatus(entries)
            }
            Ok(None) => (StatusCode::FORBIDDEN, "当前账号没有个人空间").into_response(),
            Err(e) => e.into_response(),
        },
        _ => (StatusCode::METHOD_NOT_ALLOWED, "该资源仅支持 OPTIONS / PROPFIND").into_response(),
    }
}

/// `/dav/{*path}`:按方法分发。
pub async fn dav_handler(
    State(app): State<App>,
    headers: HeaderMap,
    method: Method,
    AxumPath(rest): AxumPath<String>,
    body: Body,
) -> Response {
    let identity = match dav_identity(&app, &headers).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let target = match resolve_target(&app, &identity, &rest).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let share_name = target.first.clone();
    tracing::info!(
        "[WebDAV] {} /dav/{} {} → 共享[{}](id={}) 路径[{}] 用户[{}]",
        method.as_str(),
        share_name,
        if target.spot.rel.is_empty() { "(根)".to_string() } else { target.spot.rel.clone() },
        target.share.name,
        target.share.id,
        target.spot.rel,
        identity.as_user().map(|u| u.username.clone()).unwrap_or_default(),
    );

    match method.as_str() {
        "OPTIONS" => dav_options(),
        "PROPFIND" => propfind(&target.spot, &share_name).await,
        "GET" => dav_get(&identity, &target.share, &target.rules, &target.spot, &headers, false).await,
        "HEAD" => dav_get(&identity, &target.share, &target.rules, &target.spot, &headers, true).await,
        "PUT" => dav_put(&app, &identity, &target.share, &target.rules, &headers, &target.spot, body).await,
        "MKCOL" => dav_mkcol(&app, &identity, &target.share, &target.rules, &target.spot, &target.spot.rel).await,
        "DELETE" => dav_delete(&app, &identity, &target.share, &target.spot).await,
        "MOVE" => dav_move(&app, &identity, &target.share, &target.rules, &headers, &target.spot, &target.first).await,
        "COPY" => dav_copy(&app, &identity, &target.share, &target.rules, &headers, &target.spot, &target.first).await,
        "PROPPATCH" => wrap_multistatus(vec![prop_entry(
            &href_of(&share_name, &target.spot.rel),
            &file_name_of(&target.spot.rel),
            target.spot.full.is_dir(),
            0,
            0,
        )]),
        // 锁是最小实现:返回合法 LOCK 响应以兼容客户端编辑流程,不强制互斥。
        "LOCK" => dav_lock(),
        "UNLOCK" => StatusCode::NO_CONTENT.into_response(),
        _ => (StatusCode::METHOD_NOT_ALLOWED, "WebDAV 不支持该方法").into_response(),
    }
}

// ── 各动词 ──

async fn propfind(spot: &Spot, share_name: &str) -> Response {
    if !spot.full.exists() {
        return (StatusCode::NOT_FOUND, "文件不存在").into_response();
    }
    let meta = match std::fs::metadata(&spot.full) {
        Ok(m) => m,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    let is_dir = meta.is_dir();
    let mtime = fsx::unix_secs(meta.modified());
    let mut entries = vec![prop_entry(
        &href_of(share_name, &spot.rel),
        &file_name_of(&spot.rel),
        is_dir,
        if is_dir { 0 } else { meta.len() },
        mtime,
    )];
    if is_dir {
        if let Ok(rd) = std::fs::read_dir(&spot.full) {
            for de in rd.flatten() {
                let name = de.file_name().to_string_lossy().to_string();
                let child = child_rel(&spot.rel, &name);
                let Ok(cm) = de.metadata() else { continue };
                entries.push(prop_entry(
                    &href_of(share_name, &child),
                    &name,
                    cm.is_dir(),
                    if cm.is_dir() { 0 } else { cm.len() },
                    fsx::unix_secs(cm.modified()),
                ));
            }
        }
    }
    wrap_multistatus(entries)
}

async fn dav_get(
    identity: &Identity,
    share: &Share,
    rules: &[AccessRule],
    spot: &Spot,
    headers: &HeaderMap,
    is_head: bool,
) -> Response {
    if perms::ensure_download(identity.as_user(), share, rules, &spot.rel).is_err() {
        return (StatusCode::FORBIDDEN, "当前账号无下载权限").into_response();
    }
    if spot.full.is_dir() {
        // 极简目录浏览页(便于浏览器直接访问)。
        let mut items = vec!["<li><a href=\"../\">../</a></li>".to_string()];
        if let Ok(rd) = std::fs::read_dir(&spot.full) {
            for de in rd.flatten() {
                let name = de.file_name().to_string_lossy().to_string();
                let slash = if de.path().is_dir() { "/" } else { "" };
                items.push(format!(
                    "<li><a href=\"{}{}\">{}{}</a></li>",
                    href_encode(&name),
                    slash,
                    esc(&name),
                    slash
                ));
            }
        }
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            format!("<h2>{}</h2><ul>{}</ul>", esc(&spot.rel), items.join("")),
        )
            .into_response();
    }
    let ext = crate::domain::pathguard::ext_of(&spot.rel);
    stream_file(
        &spot.full,
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        &media::mime_of(&ext),
        false,
        !is_head,
        &file_name_of(&spot.rel),
    )
    .await
}

async fn dav_put(
    app: &App,
    identity: &Identity,
    share: &Share,
    rules: &[AccessRule],
    headers: &HeaderMap,
    spot: &Spot,
    body: Body,
) -> Response {
    use tokio::io::AsyncWriteExt;
    if spot.rel.is_empty() {
        return (StatusCode::CONFLICT, "不能覆盖共享根目录").into_response();
    }
    if let Err(e) = perms::ensure_write(identity.as_user(), share, rules, &spot.rel, |x| x.can_upload, "上传") {
        return e.into_response();
    }
    let len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if len > DAV_PUT_MAX {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "文件超过 WebDAV 单次上传上限(256MB),请使用网页端分片上传",
        )
            .into_response();
    }
    if let Some(parent) = spot.full.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return (StatusCode::CONFLICT, "父目录创建失败").into_response();
        }
    }
    if spot.full.is_dir() {
        return (StatusCode::CONFLICT, "目标位置是文件夹,无法覆盖").into_response();
    }
    let existed = spot.full.is_file();
    if existed && ensure_manageable(app, identity, share, &spot.rel).await.is_err() {
        return (StatusCode::FORBIDDEN, "同名文件已存在且不是你创建的,无法覆盖").into_response();
    }
    if let Err(e) = chunkup::ensure_disk_space(&spot.full, len as i64) {
        return e.into_response();
    }
    // 流式写临时文件 → rename(原子落盘)。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = spot
        .full
        .parent()
        .unwrap_or(spot.full.as_path())
        .join(format!(".nd-dav-{}-{}.tmp", now_secs(), nanos));
    let mut stream = std::pin::pin!(body.into_data_stream());
    {
        let mut f = match tokio::fs::File::create(&tmp).await {
            Ok(f) => f,
            Err(e) => return ApiError::internal(format!("写入失败: {e}")).into_response(),
        };
        while let Some(chunk) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            match chunk {
                Ok(bytes) => {
                    if let Err(e) = f.write_all(&bytes).await {
                        let _ = tokio::fs::remove_file(&tmp).await;
                        return ApiError::internal(format!("写入失败: {e}")).into_response();
                    }
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return ApiError::internal(format!("传输中断: {e}")).into_response();
                }
            }
        }
        if let Err(e) = f.flush().await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return ApiError::internal(format!("写入失败: {e}")).into_response();
        }
    }
    if let Err(e) = tokio::fs::rename(&tmp, &spot.full).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return ApiError::internal(format!("保存失败: {e}")).into_response();
    }
    let _ = app.db.set_meta_owner(share.id, &spot.rel, identity.id()).await;
    if existed {
        media::purge(&app.db, &app.media_dir, share.id, &spot.rel).await;
    }
    app.spawn_replace(share.id, &spot.rel, &spot.full);
    (StatusCode::CREATED, "created").into_response()
}

async fn dav_mkcol(
    app: &App,
    identity: &Identity,
    share: &Share,
    rules: &[AccessRule],
    spot: &Spot,
    rel: &str,
) -> Response {
    if spot.rel.is_empty() {
        return (StatusCode::METHOD_NOT_ALLOWED, "共享根目录已存在").into_response();
    }
    if let Err(e) = perms::ensure_write(identity.as_user(), share, rules, rel, |x| x.can_mkdir, "建目录") {
        return e.into_response();
    }
    if spot.full.exists() {
        return (StatusCode::METHOD_NOT_ALLOWED, "同名文件或文件夹已存在").into_response();
    }
    let Some(parent) = spot.full.parent() else {
        return (StatusCode::CONFLICT, "无效路径").into_response();
    };
    if !parent.is_dir() {
        return (StatusCode::CONFLICT, "上级目录不存在").into_response();
    }
    if let Err(e) = std::fs::create_dir(&spot.full) {
        return ApiError::internal(format!("创建失败: {e}")).into_response();
    }
    let _ = app.db.set_meta_owner(share.id, rel, identity.id()).await;
    StatusCode::CREATED.into_response()
}

async fn dav_delete(app: &App, identity: &Identity, share: &Share, spot: &Spot) -> Response {
    if spot.rel.is_empty() {
        return (StatusCode::FORBIDDEN, "不能删除共享根目录").into_response();
    }
    if !spot.full.exists() {
        return (StatusCode::NOT_FOUND, "目标不存在").into_response();
    }
    if let Err(e) = perms::ensure_write(
        identity.as_user(),
        share,
        &app.db.rules_of(share.id).await.unwrap_or_default(),
        &spot.rel,
        |x| x.can_delete,
        "删除",
    ) {
        return e.into_response();
    }
    if ensure_manageable(app, identity, share, &spot.rel).await.is_err() {
        return (StatusCode::FORBIDDEN, "该内容不是由你创建的,无法删除").into_response();
    }
    let mut files = Vec::new();
    fsx::collect_files(&spot.full, &spot.rel, &mut files, 0);
    for (rel, _) in &files {
        media::purge(&app.db, &app.media_dir, share.id, rel).await;
    }
    let result = if spot.full.is_dir() {
        std::fs::remove_dir_all(&spot.full)
    } else {
        std::fs::remove_file(&spot.full)
    };
    if let Err(e) = result {
        return ApiError::internal(format!("删除失败: {e}")).into_response();
    }
    media::purge(&app.db, &app.media_dir, share.id, &spot.rel).await;
    let _ = app.db.drop_meta(share.id, &spot.rel).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Destination 头 → `(第一段, 相对路径)`。
fn parse_destination(headers: &HeaderMap) -> Result<(String, String), Box<Response>> {
    let raw = headers
        .get("Destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Box::new((StatusCode::BAD_REQUEST, "缺少 Destination 头").into_response()))?;
    let path = raw
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/').map(|(_, p)| format!("/{p}")))
        .unwrap_or_else(|| raw.to_string());
    let path = path.split('?').next().unwrap_or(&path).to_string();
    let rest = path.trim_start_matches("/dav").trim_start_matches('/');
    Ok(decode_path(rest))
}

fn overwrite_flag(headers: &HeaderMap) -> bool {
    headers
        .get("Overwrite")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("t"))
        .unwrap_or(true)
}

async fn dav_move(
    app: &App,
    identity: &Identity,
    share: &Share,
    rules: &[AccessRule],
    headers: &HeaderMap,
    spot: &Spot,
    target_first: &str,
) -> Response {
    let (dest_first, dest_rel) = match parse_destination(headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if dest_first != target_first {
        return (StatusCode::BAD_GATEWAY, "不支持跨空间移动,请在网页端操作").into_response();
    }
    if dest_rel == spot.rel {
        return (StatusCode::FORBIDDEN, "源与目标相同").into_response();
    }
    let dest = match locate(app, share.id, &dest_rel).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if dest.full.exists() {
        if !overwrite_flag(headers) {
            return (StatusCode::PRECONDITION_FAILED, "目标已存在且禁止覆盖").into_response();
        }
        if let Err(e) = perms::ensure_write(
            identity.as_user(),
            share,
            &app.db.rules_of(share.id).await.unwrap_or_default(),
            &dest_rel,
            |x| x.can_delete,
            "删除",
        ) {
            return e.into_response();
        }
        if ensure_manageable(app, identity, share, &dest_rel).await.is_err() {
            return (StatusCode::FORBIDDEN, "目标内容不是由你创建的,无法覆盖").into_response();
        }
        let r = if dest.full.is_dir() {
            std::fs::remove_dir_all(&dest.full)
        } else {
            std::fs::remove_file(&dest.full)
        };
        if let Err(e) = r {
            return ApiError::internal(format!("清理目标失败: {e}")).into_response();
        }
    }
    if let Err(e) = perms::ensure_write(identity.as_user(), share, rules, &spot.rel, |x| x.can_delete, "移动") {
        return e.into_response();
    }
    if ensure_manageable(app, identity, share, &spot.rel).await.is_err() {
        return (StatusCode::FORBIDDEN, "该内容不是由你创建的,无法移动").into_response();
    }
    if let Some(parent) = dest.full.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return (StatusCode::CONFLICT, "目标目录创建失败").into_response();
        }
    }
    if let Err(e) = std::fs::rename(&spot.full, &dest.full) {
        return ApiError::internal(format!("移动失败: {e}")).into_response();
    }
    let _ = app.db.rename_meta(share.id, &spot.rel, &dest_rel).await;
    media::purge(&app.db, &app.media_dir, share.id, &spot.rel).await;
    StatusCode::CREATED.into_response()
}

async fn dav_copy(
    app: &App,
    identity: &Identity,
    share: &Share,
    rules: &[AccessRule],
    headers: &HeaderMap,
    spot: &Spot,
    target_first: &str,
) -> Response {
    let (dest_first, dest_rel) = match parse_destination(headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if dest_first != target_first {
        return (StatusCode::BAD_GATEWAY, "不支持跨空间复制,请在网页端操作").into_response();
    }
    let dest = match locate(app, share.id, &dest_rel).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if dest.full.exists() {
        if !overwrite_flag(headers) {
            return (StatusCode::PRECONDITION_FAILED, "目标已存在且禁止覆盖").into_response();
        }
        if let Err(e) = perms::ensure_write(
            identity.as_user(),
            share,
            &app.db.rules_of(share.id).await.unwrap_or_default(),
            &dest_rel,
            |x| x.can_delete,
            "删除",
        ) {
            return e.into_response();
        }
        if ensure_manageable(app, identity, share, &dest_rel).await.is_err() {
            return (StatusCode::FORBIDDEN, "目标内容不是由你创建的,无法覆盖").into_response();
        }
        let _ = if dest.full.is_dir() {
            std::fs::remove_dir_all(&dest.full)
        } else {
            std::fs::remove_file(&dest.full)
        };
    }
    if let Err(e) = perms::ensure_write(identity.as_user(), share, rules, &dest_rel, |x| x.can_upload, "复制") {
        return e.into_response();
    }
    if copy_rec(&spot.full, &dest.full, 0).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "复制失败").into_response();
    }
    let _ = app.db.set_meta_owner(share.id, &dest_rel, identity.id()).await;
    StatusCode::CREATED.into_response()
}

fn copy_rec(src: &std::path::Path, dst: &std::path::Path, depth: u32) -> std::io::Result<()> {
    if depth > 24 {
        return Ok(());
    }
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for de in std::fs::read_dir(src)?.flatten() {
            copy_rec(&de.path(), &dst.join(de.file_name()), depth + 1)?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn dav_lock() -> Response {
    let token = format!("opaquelocktoken:ndisk-{}", auth::new_token());
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope>\
         <D:depth>0</D:depth><D:timeout>Second-3600</D:timeout>\
         <D:locktoken><D:href>{token}</D:href></D:locktoken>\
         </D:activelock></D:lockdiscovery></D:prop>"
    );
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response();
    if let Ok(v) = header::HeaderValue::from_str(&format!("<{token}>")) {
        resp.headers_mut().insert(header::HeaderName::from_static("lock-token"), v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_path_segments() {
        let (first, rel) = decode_path("%E4%B8%8B%E8%BD%BD/a%20b/c.txt");
        assert_eq!(first, "下载");
        assert_eq!(rel, "a b/c.txt");
        let (f2, r2) = decode_path("/");
        assert_eq!((f2.as_str(), r2.as_str()), ("", ""));
    }

    #[test]
    fn overwrite_default_true() {
        let mut h = HeaderMap::new();
        assert!(overwrite_flag(&h));
        h.insert("Overwrite", "F".parse().unwrap());
        assert!(!overwrite_flag(&h));
    }
}
