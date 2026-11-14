// WebDAV 支持:Basic 认证,复用站内权限 / 归属 / 媒体缓存体系。
// 协议子集:OPTIONS / PROPFIND / GET / HEAD / PUT / MKCOL / DELETE / MOVE / COPY / PROPPATCH / LOCK / UNLOCK。
// 客户端(Windows 网络驱动器 / RaiDrive / Cyberduck 等)连接 http://<host>:<port>/dav/,建议走 HTTPS。

use std::path::Path;

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use futures_core::Stream as _;

use crate::api::{child_rel, ensure_manageable, locate, stream_file};
use crate::domain::auth::{self, Identity};
use crate::domain::fsx;
use crate::domain::media;
use crate::domain::perms::{self, Level};
use crate::domain::uplink;
use crate::error::{ApiError, ApiResult};
use crate::state::{now_secs, AppState};

/// WebDAV 单次 PUT 上限(与 axum 全局请求体上限一致)。
const DAV_PUT_MAX: u64 = 256 * 1024 * 1024;

// ---------------- 认证 ----------------

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Basic realm=\"NDisk WebDAV\"")],
        "需要认证:WebDAV 使用 Basic 认证(用户名 + 密码)",
    )
        .into_response()
}

/// Basic(用户名+密码)或 Bearer(会话令牌)二选一。
async fn dav_identity(st: &AppState, headers: &HeaderMap) -> Result<Identity, Response> {
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Err(unauthorized());
    };
    if let Some(enc) = raw.strip_prefix("Basic ") {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(enc.trim()) {
            if let Ok(s) = String::from_utf8(bytes) {
                if let Some((name, pass)) = s.split_once(':') {
                    if let Ok(Some(user)) = st.db.user_by_name(name).await {
                        if auth::verify_password(pass, &user.password_hash) {
                            return Ok(Identity {
                                user: Some(user),
                                token: None,
                            });
                        }
                    }
                }
            }
        }
        return Err(unauthorized());
    }
    if let Some(tok) = raw.strip_prefix("Bearer ") {
        let tok = tok.trim();
        if let Ok(Some(user)) = st.db.user_by_token(tok).await {
            return Ok(Identity {
                user: Some(user),
                token: Some(tok.to_string()),
            });
        }
        return Err(unauthorized());
    }
    Err(unauthorized())
}

// ---------------- 共享映射 ----------------

fn share_visible(
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
) -> bool {
    if perms::is_private(share) {
        return identity
            .user
            .as_ref()
            .map(|u| perms::is_owner(share, u))
            .unwrap_or(false);
    }
    match perms::level_of(rules, "") {
        Level::Guest => true,
        Level::Login => identity.user.is_some(),
        Level::Admin => identity.is_admin(),
    }
}

/// 当前用户在 WebDAV 中可见的全部共享。
async fn visible_shares(
    st: &AppState,
    identity: &Identity,
) -> ApiResult<Vec<(crate::db::Share, Vec<crate::db::AccessRule>)>> {
    let mut out = Vec::new();
    for s in st.db.all_shares().await? {
        let rules = st.db.rules_of(s.id).await?;
        if share_visible(identity, &s, &rules) {
            out.push((s, rules));
        }
    }
    Ok(out)
}

/// 当前用户自己的个人空间(kind=home 且属主为本人)。
async fn home_share(
    st: &AppState,
    identity: &Identity,
) -> ApiResult<Option<(crate::db::Share, Vec<crate::db::AccessRule>)>> {
    Ok(visible_shares(st, identity)
        .await?
        .into_iter()
        .find(|(s, _)| s.kind == "home" && s.owner_id == identity.user.as_ref().map(|u| u.id)))
}

/// 解析 /dav/<路径> 的目标。
/// 语义:/dav/ 根 = 当前用户的个人空间;根下的非个人空间集合 = 其它共享目录。
/// 命中顺序:个人空间内已存在的条目优先,其次匹配共享目录名,否则落到个人空间(用于创建)。
async fn resolve_target(st: &AppState, identity: &Identity, rest: &str) -> ApiResult<DavTarget> {
    let (first, rel) = decode_path(rest);
    let home = home_share(st, identity)
        .await?
        .ok_or_else(|| ApiError::forbidden("当前账号没有个人空间"))?;
    let full_rel = if rel.is_empty() {
        first.clone()
    } else {
        child_rel(&first, &rel)
    };
    if !first.is_empty() {
        // 个人空间内已存在的条目优先(文件 / 文件夹)
        let home_spot = locate(st, home.0.id, &full_rel).await?;
        if home_spot.full.exists() {
            return Ok(DavTarget {
                share: home.0.clone(),
                rules: home.1.clone(),
                spot: home_spot,
                first,
            });
        }
        // 根段匹配其它共享目录 → 该共享内解析
        if rel.is_empty() || !rel.is_empty() {
            for (s, r) in visible_shares(st, identity).await? {
                if s.kind != "home" && s.name == first {
                    let spot = locate(st, s.id, &rel).await?;
                    return Ok(DavTarget {
                        share: s,
                        rules: r,
                        spot,
                        first,
                    });
                }
            }
        }
    }
    // 根本身 / 新条目:落在个人空间
    let spot = locate(st, home.0.id, &full_rel).await?;
    Ok(DavTarget {
        share: home.0.clone(),
        rules: home.1.clone(),
        spot,
        first,
    })
}

/// 一次 WebDAV 请求解析出的目标。
struct DavTarget {
    share: crate::db::Share,
    rules: Vec<crate::db::AccessRule>,
    spot: crate::api::Spot,
    /// 路径第一段(href 用)
    first: String,
}

// ---------------- 工具 ----------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn href_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// RFC 1123 日期(UTC),无外部依赖。
fn http_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let weekday = ((days % 7 + 11) % 7) as usize; // 1970-01-01 是周四
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if d < dy {
            break;
        }
        d -= dy;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1usize;
    for md in mdays {
        if d < md {
            break;
        }
        d -= md;
        m += 1;
    }
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday],
        d + 1,
        MONTHS[m - 1],
        y,
        hh,
        mm,
        ss
    )
}

/// /dav/<share>/<rel..> 解码为(共享名, 相对路径),逐段百分号解码。
fn decode_path(rest: &str) -> (String, String) {
    let rest = rest.trim_start_matches('/');
    let (name, rel) = match rest.split_once('/') {
        Some((n, r)) => (n, r),
        None => (rest, ""),
    };
    let dec = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .to_string()
    };
    let rel = rel
        .split('/')
        .filter(|s| !s.is_empty())
        .map(dec)
        .collect::<Vec<_>>()
        .join("/");
    (dec(name), rel)
}

fn dav_options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            ("DAV", "1, 2"),
            ("MS-Author-Via", "DAV"),
            (
                "Allow",
                "OPTIONS, GET, HEAD, PUT, PROPFIND, PROPPATCH, MKCOL, DELETE, MOVE, COPY, LOCK, UNLOCK",
            ),
        ],
        Body::empty(),
    )
        .into_response()
}

fn href_of(share_name: &str, rel: &str) -> String {
    let mut h = format!("/dav/{}", href_encode(share_name));
    if !rel.is_empty() {
        h.push('/');
        h.push_str(&href_encode(rel));
    }
    if rel.is_empty() {
        h.push('/');
    }
    h
}

// ---------------- PROPFIND ----------------

fn prop_entry(href: &str, display: &str, is_dir: bool, size: u64, mtime: i64) -> String {
    let mtime = if mtime <= 0 { now_secs() } else { mtime };
    let rt = if is_dir {
        "<D:resourcetype><D:collection/></D:resourcetype>"
    } else {
        "<D:resourcetype/>"
    };
    let size_prop = if is_dir {
        String::new()
    } else {
        format!("<D:getcontentlength>{size}</D:getcontentlength>")
    };
    format!(
        "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{rt}\
         <D:displayname>{display}</D:displayname>{size_prop}\
         <D:getlastmodified>{}</D:getlastmodified>\
         <D:getetag>\"{}-{mtime}\"</D:getetag><D:supportedlock>\
         <D:lockentry><D:lockscope><D:exclusive/></D:lockscope>\
         <D:locktype><D:write/></D:locktype></D:lockentry></D:supportedlock>\
         </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        http_date(mtime),
        size,
        display = esc(display),
        href = esc(href),
    )
}

fn wrap_multistatus(entries: Vec<String>) -> Response {
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">{}</D:multistatus>",
            entries.join("")
        ),
    )
        .into_response()
}

// ---------------- 主分发 ----------------

pub async fn dav_root(State(st): State<AppState>, headers: HeaderMap, method: Method) -> Response {
    let identity = match dav_identity(&st, &headers).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    match method.as_str() {
        "OPTIONS" => dav_options(),
        "PROPFIND" => match home_share(&st, &identity).await {
            Ok(Some((home, _))) => {
                let mut entries = vec![prop_entry("/dav/", "NDisk", true, 0, now_secs())];
                // /dav/ 根 = 当前用户的个人空间:列出其根目录子项
                if let Ok(home_spot) = locate(&st, home.id, "").await {
                    if let Ok(rd) = std::fs::read_dir(&home_spot.full) {
                        for de in rd.flatten() {
                            let name = de.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') || name.starts_with(".nd-") {
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
                // 其它共享目录作为独立集合挂在根下
                for (s, _) in visible_shares(&st, &identity).await.unwrap_or_default() {
                    if s.kind != "home" {
                        entries.push(prop_entry(
                            &format!("/dav/{}/", href_encode(&s.name)),
                            &s.name,
                            true,
                            0,
                            now_secs(),
                        ));
                    }
                }
                wrap_multistatus(entries)
            }
            Ok(None) => (StatusCode::FORBIDDEN, "当前账号没有个人空间").into_response(),
            Err(e) => e.into_response(),
        },
        _ => (
            StatusCode::METHOD_NOT_ALLOWED,
            "该资源仅支持 OPTIONS / PROPFIND",
        )
            .into_response(),
    }
}

pub async fn dav_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    method: Method,
    AxumPath(rest): AxumPath<String>,
    body: Body,
) -> Response {
    let identity = match dav_identity(&st, &headers).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let target = match resolve_target(&st, &identity, &rest).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let share_name = target.first.clone();
    // 操作留痕:多端同步排查时可直接看到每个请求落到了哪个空间 / 路径
    tracing::info!(
        "[WebDAV] {} /dav/{} {} → 共享[{}](id={}) 路径[{}] 用户[{}]",
        method.as_str(),
        share_name,
        if target.spot.rel.is_empty() {
            "(根)".to_string()
        } else {
            target.spot.rel.clone()
        },
        target.share.name,
        target.share.id,
        target.spot.rel,
        identity
            .user
            .as_ref()
            .map(|u| u.username.clone())
            .unwrap_or_default(),
    );

    match method.as_str() {
        "OPTIONS" => dav_options(),
        "PROPFIND" => propfind(&target.spot, &share_name).await,
        "GET" => {
            dav_get(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &target.spot,
                &headers,
                false,
            )
            .await
        }
        "HEAD" => {
            dav_get(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &target.spot,
                &headers,
                true,
            )
            .await
        }
        "PUT" => {
            dav_put(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &headers,
                &target.spot,
                body,
            )
            .await
        }
        "MKCOL" => {
            dav_mkcol(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &target.spot,
                &target.spot.rel,
            )
            .await
        }
        "DELETE" => dav_delete(&st, &identity, &target.share, &target.spot).await,
        "MOVE" => {
            dav_move(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &headers,
                &target.spot,
                &target.first,
            )
            .await
        }
        "COPY" => {
            dav_copy(
                &st,
                &identity,
                &target.share,
                &target.rules,
                &headers,
                &target.spot,
                &target.first,
            )
            .await
        }
        "PROPPATCH" => {
            // 属性只读(lastmodified 等),按规范返回全部忽略的 207
            wrap_multistatus(vec![prop_entry(
                &href_of(&share_name, &target.spot.rel),
                &super::file_name_of(&target.spot.rel),
                target.spot.full.is_dir(),
                0,
                0,
            )])
        }
        "LOCK" => dav_lock(),
        "UNLOCK" => StatusCode::NO_CONTENT.into_response(),
        _ => (StatusCode::METHOD_NOT_ALLOWED, "WebDAV 不支持该方法").into_response(),
    }
}

// ---------------- PROPFIND ----------------

async fn propfind(spot: &crate::api::Spot, share_name: &str) -> Response {
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
        &super::file_name_of(&spot.rel),
        is_dir,
        if is_dir { 0 } else { meta.len() },
        mtime,
    )];
    if is_dir {
        if let Ok(rd) = std::fs::read_dir(&spot.full) {
            for de in rd.flatten() {
                let name = de.file_name().to_string_lossy().to_string();
                let child_rel = child_rel(&spot.rel, &name);
                let Ok(cm) = de.metadata() else { continue };
                entries.push(prop_entry(
                    &href_of(share_name, &child_rel),
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

// ---------------- GET / HEAD ----------------

async fn dav_get(
    _st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
    spot: &crate::api::Spot,
    headers: &HeaderMap,
    is_head: bool,
) -> Response {
    if perms::ensure_download(identity, share, rules, &spot.rel).is_err() {
        return (StatusCode::FORBIDDEN, "当前账号无下载权限").into_response();
    }
    if spot.full.is_dir() {
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
    let ext = crate::domain::paths::ext_of(&spot.rel);
    stream_file(
        &spot.full,
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        &media::mime_of(&ext),
        false,
        !is_head,
        &super::file_name_of(&spot.rel),
    )
    .await
}

// ---------------- PUT ----------------

async fn dav_put(
    st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
    headers: &HeaderMap,
    spot: &crate::api::Spot,
    body: Body,
) -> Response {
    if spot.rel.is_empty() {
        return (StatusCode::CONFLICT, "不能覆盖共享根目录").into_response();
    }
    if let Err(e) = perms::ensure_write(identity, share, rules, &spot.rel, |x| x.can_upload, "上传")
    {
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
    // 自动创建父目录(WebDAV 客户端常直接 PUT 深路径)
    if let Some(parent) = spot.full.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return (StatusCode::CONFLICT, "父目录创建失败").into_response();
        }
    }
    if spot.full.is_dir() {
        return (StatusCode::CONFLICT, "目标位置是文件夹,无法覆盖").into_response();
    }
    let existed = spot.full.is_file();
    if existed {
        // 共享目录内覆盖:普通用户只能覆盖自己创建的文件
        if ensure_manageable(st, identity, share, &spot.rel)
            .await
            .is_err()
        {
            return (
                StatusCode::FORBIDDEN,
                "同名文件已存在且不是你创建的,无法覆盖",
            )
                .into_response();
        }
    }
    if let Err(e) = uplink::ensure_disk_space(&spot.full, len as i64) {
        return e.into_response();
    }
    // 流式写入同目录临时文件 → 原子替换
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = spot
        .full
        .parent()
        .unwrap_or(spot.full.as_path())
        .join(format!(".nd-dav-{}-{}.tmp", now_secs(), nanos));
    // 注意:必须按值固定(pin!)——若对 &mut stream 取 Pin,Send 会要求 BodyDataStream: Sync
    let mut stream = std::pin::pin!(body.into_data_stream());
    let mut written: u64 = 0;
    {
        use tokio::io::AsyncWriteExt;
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
                    written += bytes.len() as u64;
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
    let _ = st
        .db
        .set_meta_owner(share.id, &spot.rel, identity.id())
        .await;
    if existed {
        media::purge(st, share.id, &spot.rel).await;
    }
    // 上传视频自动转码替换(与网页上传同一策略,受后台开关控制)
    let st3 = st.clone();
    let sid = share.id;
    let rel3 = spot.rel.clone();
    let target3 = spot.full.clone();
    tokio::spawn(async move {
        media::maybe_replace_transcode(&st3, sid, &rel3, &target3).await;
    });
    let _ = written;
    (StatusCode::CREATED, "created").into_response()
}

// ---------------- MKCOL / DELETE ----------------

async fn dav_mkcol(
    st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
    spot: &crate::api::Spot,
    rel: &str,
) -> Response {
    if spot.rel.is_empty() {
        return (StatusCode::METHOD_NOT_ALLOWED, "共享根目录已存在").into_response();
    }
    if let Err(e) = perms::ensure_write(identity, share, rules, rel, |x| x.can_mkdir, "建目录") {
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
    let _ = st.db.set_meta_owner(share.id, rel, identity.id()).await;
    StatusCode::CREATED.into_response()
}

async fn dav_delete(
    st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    spot: &crate::api::Spot,
) -> Response {
    if spot.rel.is_empty() {
        return (StatusCode::FORBIDDEN, "不能删除共享根目录").into_response();
    }
    if !spot.full.exists() {
        return (StatusCode::NOT_FOUND, "目标不存在").into_response();
    }
    if let Err(e) = perms::ensure_write(
        identity,
        share,
        &st.db.rules_of(share.id).await.unwrap_or_default(),
        &spot.rel,
        |x| x.can_delete,
        "删除",
    ) {
        return e.into_response();
    }
    if ensure_manageable(st, identity, share, &spot.rel)
        .await
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "该内容不是由你创建的,无法删除").into_response();
    }
    // 递归删除(WebDAV 语义)+ 清理媒体缓存
    let mut files = Vec::new();
    fsx::collect_files(&spot.full, &spot.rel, &mut files, 0);
    for (rel, _) in &files {
        media::purge(st, share.id, rel).await;
    }
    let result = if spot.full.is_dir() {
        std::fs::remove_dir_all(&spot.full)
    } else {
        std::fs::remove_file(&spot.full)
    };
    if let Err(e) = result {
        return ApiError::internal(format!("删除失败: {e}")).into_response();
    }
    media::purge(st, share.id, &spot.rel).await;
    let _ = st.db.drop_meta(share.id, &spot.rel).await;
    StatusCode::NO_CONTENT.into_response()
}

// ---------------- MOVE / COPY ----------------

/// 解析 Destination 头 → (共享名, 相对路径);只允许同共享内移动/复制。
fn parse_destination(headers: &HeaderMap) -> Result<(String, String), Box<Response>> {
    let raw = headers
        .get("Destination")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            Box::new((StatusCode::BAD_REQUEST, "缺少 Destination 头").into_response())
        })?;
    let path = raw
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/').map(|(_, p)| format!("/{p}")))
        .unwrap_or_else(|| raw.to_string());
    let path = path.split('?').next().unwrap_or(&path).to_string();
    let rest = path.trim_start_matches("/dav").trim_start_matches('/');
    Ok(decode_path(rest))
}

async fn dav_move(
    st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
    headers: &HeaderMap,
    spot: &crate::api::Spot,
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
    let dest = match locate(st, share.id, &dest_rel).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if dest.full.exists() {
        let overwrite = headers
            .get("Overwrite")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("t"))
            .unwrap_or(true);
        if !overwrite {
            return (StatusCode::PRECONDITION_FAILED, "目标已存在且禁止覆盖").into_response();
        }
        if let Err(e) = perms::ensure_write(
            identity,
            share,
            &st.db.rules_of(share.id).await.unwrap_or_default(),
            &dest_rel,
            |x| x.can_delete,
            "删除",
        ) {
            return e.into_response();
        }
        if ensure_manageable(st, identity, share, &dest_rel)
            .await
            .is_err()
        {
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
    if let Err(e) = perms::ensure_write(identity, share, rules, &spot.rel, |x| x.can_delete, "移动")
    {
        return e.into_response();
    }
    if ensure_manageable(st, identity, share, &spot.rel)
        .await
        .is_err()
    {
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
    let _ = st.db.rename_meta(share.id, &spot.rel, &dest_rel).await;
    media::purge(st, share.id, &spot.rel).await;
    StatusCode::CREATED.into_response()
}

async fn dav_copy(
    st: &AppState,
    identity: &Identity,
    share: &crate::db::Share,
    rules: &[crate::db::AccessRule],
    headers: &HeaderMap,
    spot: &crate::api::Spot,
    target_first: &str,
) -> Response {
    let (dest_first, dest_rel) = match parse_destination(headers) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    if dest_first != target_first {
        return (StatusCode::BAD_GATEWAY, "不支持跨空间复制,请在网页端操作").into_response();
    }
    let dest = match locate(st, share.id, &dest_rel).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if dest.full.exists() {
        let overwrite = headers
            .get("Overwrite")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("t"))
            .unwrap_or(true);
        if !overwrite {
            return (StatusCode::PRECONDITION_FAILED, "目标已存在且禁止覆盖").into_response();
        }
        if let Err(e) = perms::ensure_write(
            identity,
            share,
            &st.db.rules_of(share.id).await.unwrap_or_default(),
            &dest_rel,
            |x| x.can_delete,
            "删除",
        ) {
            return e.into_response();
        }
        let _ = if dest.full.is_dir() {
            std::fs::remove_dir_all(&dest.full)
        } else {
            std::fs::remove_file(&dest.full)
        };
    }
    if let Err(e) = perms::ensure_write(identity, share, rules, &dest_rel, |x| x.can_upload, "复制")
    {
        return e.into_response();
    }
    if copy_rec(&spot.full, &dest.full, 0).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "复制失败").into_response();
    }
    let _ = st
        .db
        .set_meta_owner(share.id, &dest_rel, identity.id())
        .await;
    StatusCode::CREATED.into_response()
}

fn copy_rec(src: &Path, dst: &Path, depth: u32) -> std::io::Result<()> {
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

// ---------------- LOCK / UNLOCK(最小实现,不强制锁) ----------------

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
        resp.headers_mut()
            .insert(header::HeaderName::from_static("lock-token"), v);
    }
    resp
}
