use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::file_name_of;
use crate::domain::auth::Identity;
use crate::domain::{media, paths, perms};
use crate::error::{err, ok, ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateReq {
    share_id: i64,
    #[serde(default)]
    path: String,
    /// 有效期(天);0 = 永不过期
    #[serde(default)]
    ttl_days: i64,
}

pub async fn create(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<CreateReq>,
) -> ApiResult<Response> {
    let user = identity.require()?.clone();
    let spot = crate::api::locate(&st, req.share_id, &req.path).await?;
    perms::ensure_download(&identity, &spot.share, &spot.rules, &spot.rel)?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    if !(0..=3650).contains(&req.ttl_days) {
        return Err(ApiError::bad_request(
            "有效期需在 0-3650 天之间(0 = 永不过期)",
        ));
    }
    let link = st
        .db
        .ensure_link(
            spot.share.id,
            &spot.rel,
            Some(user.id),
            req.ttl_days * 86400,
        )
        .await
        .map_err(ApiError::internal)?;
    let md = tokio::fs::metadata(&spot.full).await?;
    Ok(ok(json!({
        "token": link.token,
        "url": format!("/s/{}", link.token),
        "name": file_name_of(&spot.rel),
        "size": md.len() as i64,
        "created_at": 0,
        "hits": link.hits,
        "expires_at": link.expires_at,
    })))
}

#[derive(Deserialize)]
pub struct ListQ {
    #[serde(default)]
    share_id: i64,
}

pub async fn list(
    State(st): State<AppState>,
    identity: Identity,
    Query(q): Query<ListQ>,
) -> ApiResult<Response> {
    let user = identity.require()?;
    if q.share_id <= 0 {
        return Err(ApiError::bad_request("缺少 share_id"));
    }
    let spot = crate::api::locate(&st, q.share_id, "").await?;
    perms::ensure_read(&identity, &spot.share, &spot.rules, "")?;
    let links = st.db.links_of(spot.share.id).await?;
    let out: Vec<_> = links
        .into_iter()
        .filter(|l| identity.is_admin() || l.created_by == Some(user.id))
        .map(|l| json!({ "token": l.token, "share_id": l.share_id, "path": l.rel_path, "created_at": l.created_at, "hits": l.hits, "expires_at": l.expires_at }))
        .collect();
    Ok(ok(json!({ "links": out })))
}

pub async fn delete(
    State(st): State<AppState>,
    identity: Identity,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let user = identity.require()?.clone();
    let link = st
        .db
        .link_by_token(&token)
        .await?
        .ok_or_else(|| ApiError::not_found("分享不存在"))?;
    if !identity.is_admin() && link.created_by != Some(user.id) {
        return Err(ApiError::forbidden("无权取消他人创建的分享"));
    }
    st.db.delete_link(&token).await?;
    Ok(ok(json!({ "ok": true })))
}

/// 匿名分享页:链接有效 + 创建者仍保有下载权限才可访问。
async fn target(
    st: &AppState,
    token: &str,
) -> Result<(crate::api::Spot, crate::db::Link, String), Response> {
    let link = match st.db.link_by_token(token).await {
        Ok(Some(l)) => l,
        Ok(None) => return Err(err(StatusCode::NOT_FOUND, "分享不存在或已失效")),
        Err(e) => return Err(ApiError::from(e).into_response()),
    };
    if link.expired() {
        let _ = st.db.delete_link(token).await;
        return Err(err(StatusCode::NOT_FOUND, "分享链接已过期"));
    }
    let spot = crate::api::locate(st, link.share_id, &link.rel_path)
        .await
        .map_err(|e| e.into_response())?;
    if let Some(uid) = link.created_by {
        match st.db.user_by_id(uid).await {
            Ok(Some(creator)) => {
                let rules = st.db.rules_of(link.share_id).await.unwrap_or_default();
                if !perms::can_download_user(&creator, &spot.share, &rules, &spot.rel) {
                    return Err(err(
                        StatusCode::FORBIDDEN,
                        "分享者已无该文件的下载权限,链接失效",
                    ));
                }
            }
            _ => return Err(err(StatusCode::FORBIDDEN, "分享者账号已注销,链接失效")),
        }
    } else {
        return Err(err(StatusCode::FORBIDDEN, "分享链接无效"));
    }
    let name = file_name_of(&spot.rel);
    Ok((spot, link, name))
}

pub async fn page(State(st): State<AppState>, Path(token): Path<String>) -> Response {
    let (spot, _link, name) = match target(&st, &token).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !spot.full.is_file() {
        return err(StatusCode::NOT_FOUND, "文件不存在或已失效");
    }
    let size = match tokio::fs::metadata(&spot.full).await {
        Ok(m) => m.len(),
        Err(_) => return err(StatusCode::NOT_FOUND, "文件不存在或已失效"),
    };
    let kind = media::kind_of(&paths::ext_of(&name));
    let inline = matches!(
        kind,
        media::Kind::Image | media::Kind::Video | media::Kind::Audio
    );
    let esc = html_escape(&name);
    let dl = format!("/s/{token}/api/download?dl=1");
    let preview = format!("/s/{token}/api/download");
    let preview_btn = if inline {
        format!(r#"<a class="btn primary" href="{preview}">在线预览</a>"#)
    } else {
        String::new()
    };
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>分享文件 - {esc}</title>
<style>
  body {{ margin:0; font-family:system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif; background:#f2f4fa; color:#1f2430; }}
  .wrap {{ max-width:520px; margin:9vh auto; background:#fff; border-radius:14px; box-shadow:0 12px 34px rgba(20,26,48,.12); padding:34px 32px; }}
  h1 {{ font-size:17px; margin:0 0 6px; }}
  .name {{ font-size:13px; color:#4f6df5; word-break:break-all; margin-bottom:18px; }}
  .meta {{ font-size:13px; color:#8289a2; margin-bottom:22px; }}
  .btns {{ display:flex; gap:12px; flex-wrap:wrap; }}
  .btn {{ flex:1; min-width:180px; text-align:center; padding:12px 16px; border-radius:10px; text-decoration:none; font-size:14px; box-sizing:border-box; display:inline-flex; align-items:center; justify-content:center; }}
  .btn.primary {{ background:#4f6df5; color:#fff; }}
  .btn.ghost {{ background:#eef1fe; color:#3d5bdd; }}
  .tip {{ margin-top:20px; font-size:12px; color:#9aa3bd; }}
</style>
</head>
<body>
  <div class="wrap">
    <h1>文件分享</h1>
    <div class="name">{esc}</div>
    <div class="meta">大小 {human} · 该链接无需登录即可访问</div>
    <div class="btns">{preview_btn}<a class="btn ghost" href="{dl}">下载</a></div>
    <div class="tip">由分享者创建,请勿转发给不希望看到此文件的人。</div>
  </div>
</body>
</html>"#,
        esc = esc,
        human = human_size(size),
        preview_btn = preview_btn,
        dl = dl,
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn human_size(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[i])
    }
}

#[derive(Deserialize)]
pub struct DlQ {
    #[serde(default)]
    dl: i64,
}

/// 分享链接匿名下载;访问计数 +1。
pub async fn download(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Query(q): Query<DlQ>,
) -> Response {
    let (spot, _link, name) = match target(&st, &token).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !spot.full.is_file() {
        return err(StatusCode::NOT_FOUND, "文件不存在或已失效");
    }
    let _ = st.db.bump_link(&token).await;
    let ext = paths::ext_of(&spot.rel);
    let mime = media::mime_of(&ext);
    let inline = q.dl == 0
        && matches!(
            media::kind_of(&ext),
            media::Kind::Image | media::Kind::Video | media::Kind::Audio
        );
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    crate::api::stream_file(&spot.full, range.as_deref(), &mime, inline, true, &name).await
}
