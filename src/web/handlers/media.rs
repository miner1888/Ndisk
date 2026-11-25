//! 媒体:原文件 / 转码产物流(含 HEAD 与 Range)、转码状态轮询、图片与视频缩略图。

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::app::{now_secs, App};
use crate::domain::{access as perms, media, pathguard, ticket};
use crate::error::{err, ok, ApiError, ApiResult};
use crate::web::ctx::{file_name_of, headers_range, locate, locate_readable, stream_file, Spot};
use crate::web::identity::Identity;

#[derive(Deserialize)]
pub struct MediaQ {
    #[serde(default)]
    path: String,
    /// t=1 表示取转码产物。
    #[serde(default)]
    t: i64,
    /// dl=1 强制 attachment。
    #[serde(default)]
    dl: i64,
}

/// 定位媒体源文件;`use_transcoded=true` 时改用已完成的转码产物。
/// 返回 `(定位结果, 实际要发送的文件路径, MIME)`。
async fn source(
    app: &App,
    identity: &Identity,
    share_id: i64,
    rel: &str,
    use_transcoded: bool,
) -> Result<(Spot, std::path::PathBuf, String), Response> {
    let spot = match locate(app, share_id, rel).await {
        Ok(s) => s,
        Err(e) => return Err(e.into_response()),
    };
    if let Err(e) = perms::ensure_download(identity.as_user(), &spot.share, &spot.rules, &spot.rel) {
        return Err(e.into_response());
    }
    let mut full = spot.full.clone();
    let mime = if use_transcoded {
        match app.db.transcode(share_id, &spot.rel).await {
            Ok(Some(row)) if row.status == "done" && std::path::Path::new(&row.out_path).is_file() => {
                full = std::path::PathBuf::from(row.out_path);
                "video/mp4".to_string()
            }
            _ => return Err(err(StatusCode::NOT_FOUND, "转码尚未完成")),
        }
    } else {
        media::mime_of(&pathguard::ext_of(&spot.rel))
    };
    if !full.is_file() {
        return Err(err(StatusCode::NOT_FOUND, "文件不存在"));
    }
    Ok((spot, full, mime))
}

pub async fn get(
    State(app): State<App>,
    identity: Identity,
    headers: HeaderMap,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    let (spot, full, mime) = match source(&app, &identity, share_id, &q.path, q.t != 0).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let inline = q.dl == 0 && media::preview_inline(&pathguard::ext_of(&spot.rel));
    stream_file(&full, headers_range(&headers).as_deref(), &mime, inline, true, &file_name_of(&spot.rel)).await
}

pub async fn head(
    State(app): State<App>,
    identity: Identity,
    headers: HeaderMap,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    let (spot, full, mime) = match source(&app, &identity, share_id, &q.path, q.t != 0).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let inline = q.dl == 0 && media::preview_inline(&pathguard::ext_of(&spot.rel));
    stream_file(&full, headers_range(&headers).as_deref(), &mime, inline, false, &file_name_of(&spot.rel)).await
}

/// 播放状态:原生格式 direct=true 直连;其它视频触发 / 轮询转码。
pub async fn status(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&app, &identity, share_id, &q.path).await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let ext = pathguard::ext_of(&spot.rel);
    let kind = media::kind_of(&ext);
    let src = |t: bool| {
        let mut u = format!("/api/media/{share_id}?path={}", ticket::urlencode(&spot.rel));
        if t {
            u += "&t=1";
        }
        u
    };
    let resp = match kind {
        media::Kind::Image => json!({ "kind": "image", "direct": true }),
        media::Kind::Audio => json!({ "kind": "audio", "direct": true }),
        media::Kind::Video if media::native_video(&ext) => {
            json!({ "kind": "video", "direct": true, "src": src(false) })
        }
        media::Kind::Video => {
            if app.ffmpeg.is_none() {
                json!({ "kind": "video", "direct": false, "state": "unsupported" })
            } else {
                match app.db.transcode(share_id, &spot.rel).await? {
                    Some(row) if row.status == "done" && std::path::Path::new(&row.out_path).is_file() => {
                        json!({ "kind": "video", "direct": false, "state": "ready", "src": src(true) })
                    }
                    Some(row) if row.status == "running" && now_secs() - row.updated_at < 30 * 60 => {
                        json!({ "kind": "video", "direct": false, "state": "converting" })
                    }
                    _ => {
                        media::ensure_transcode(&app.db, &app.media_dir, app.ffmpeg.as_deref().unwrap(), share_id, &spot.rel, &spot.full)
                            .await?;
                        json!({ "kind": "video", "direct": false, "state": "converting" })
                    }
                }
            }
        }
        media::Kind::Other => json!({ "kind": "other", "direct": false }),
    };
    Ok(ok(resp))
}

fn not_found_body(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, axum::Json(json!({ "error": msg }))).into_response()
}

pub async fn video_thumb(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    asset(&app, &identity, share_id, &q.path, true, "仅视频支持封面").await
}

pub async fn image_thumb(
    State(app): State<App>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    asset(&app, &identity, share_id, &q.path, false, "非图片无需生成缩略图").await
}

async fn asset(app: &App, identity: &Identity, share_id: i64, path: &str, is_video: bool, wrong_kind: &str) -> Response {
    let spot = match locate_readable(app, identity, share_id, path).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if !spot.full.is_file() {
        return not_found_body("文件不存在");
    }
    let ext = pathguard::ext_of(&spot.rel);
    let ok_kind = if is_video {
        media::kind_of(&ext) == media::Kind::Video
    } else {
        media::kind_of(&ext) == media::Kind::Image
    };
    if !ok_kind {
        return err(StatusCode::BAD_REQUEST, wrong_kind);
    }
    let Some(ffmpeg) = app.ffmpeg.as_deref() else {
        return not_found_body("服务器未安装 ffmpeg,无法生成缩略图");
    };
    let out = if is_video {
        media::ensure_video_thumb(&app.db, &app.media_dir, &app.thumbs, ffmpeg, share_id, &spot.rel, &spot.full).await
    } else {
        media::ensure_image_thumb(&app.db, &app.media_dir, &app.thumbs, ffmpeg, share_id, &spot.rel, &spot.full).await
    };
    let out = match out {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    match tokio::fs::read(&out).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => not_found_body("缩略图不可用"),
    }
}
