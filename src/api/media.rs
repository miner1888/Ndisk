use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::{file_name_of, headers_range, locate, locate_readable, stream_file};
use crate::domain::auth::Identity;
use crate::domain::media;
use crate::domain::paths;
use crate::error::{err, ok, ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct MediaQ {
    #[serde(default)]
    path: String,
    #[serde(default)]
    t: i64,
    #[serde(default)]
    dl: i64,
}

/// 解析出实际要播放的文件(转码产物或原文件)与 MIME。
async fn source(
    st: &AppState,
    identity: &Identity,
    share_id: i64,
    rel: &str,
    use_transcoded: bool,
) -> Result<(crate::api::Spot, String), Response> {
    let spot = match locate(st, share_id, rel).await {
        Ok(s) => s,
        Err(e) => return Err(e.into_response()),
    };
    if let Err(e) =
        crate::domain::perms::ensure_download(identity, &spot.share, &spot.rules, &spot.rel)
    {
        return Err(e.into_response());
    }
    let mut full = spot.full.clone();
    let mime = if use_transcoded {
        match st.db.transcode(share_id, &spot.rel).await {
            Ok(Some((status, out, _)))
                if status == "done" && std::path::Path::new(&out).is_file() =>
            {
                full = std::path::PathBuf::from(out);
                "video/mp4".to_string()
            }
            _ => return Err(err(StatusCode::NOT_FOUND, "转码尚未完成")),
        }
    } else {
        media::mime_of(&paths::ext_of(&spot.rel))
    };
    if !full.is_file() {
        return Err(err(StatusCode::NOT_FOUND, "文件不存在"));
    }
    Ok((spot, mime))
}

pub async fn get(
    State(st): State<AppState>,
    identity: Identity,
    headers: HeaderMap,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    let (spot, mime) = match source(&st, &identity, share_id, &q.path, q.t != 0).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let inline = q.dl == 0 && media::preview_inline(&paths::ext_of(&spot.rel));
    stream_file(
        &spot.full,
        headers_range(&headers).as_deref(),
        &mime,
        inline,
        true,
        &file_name_of(&spot.rel),
    )
    .await
}

pub async fn head(
    State(st): State<AppState>,
    identity: Identity,
    headers: HeaderMap,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    let (spot, mime) = match source(&st, &identity, share_id, &q.path, q.t != 0).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let inline = q.dl == 0 && media::preview_inline(&paths::ext_of(&spot.rel));
    stream_file(
        &spot.full,
        headers_range(&headers).as_deref(),
        &mime,
        inline,
        false,
        &file_name_of(&spot.rel),
    )
    .await
}

/// 播放状态:图片 / 音频直接播放;原生视频容器直连;其它视频触发转码。
pub async fn status(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> ApiResult<Response> {
    let spot = locate_readable(&st, &identity, share_id, &q.path).await?;
    if !spot.full.is_file() {
        return Err(ApiError::not_found("文件不存在"));
    }
    let ext = paths::ext_of(&spot.rel);
    let kind = media::kind_of(&ext);
    let src = |t: bool| {
        let mut u = format!(
            "/api/media/{share_id}?path={}",
            crate::domain::ticket::urlencode(&spot.rel)
        );
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
            if st.ffmpeg.is_none() {
                json!({ "kind": "video", "direct": false, "state": "unsupported" })
            } else {
                let row = st.db.transcode(share_id, &spot.rel).await?;
                match row {
                    Some((status, out, _))
                        if status == "done" && std::path::Path::new(&out).is_file() =>
                    {
                        json!({ "kind": "video", "direct": false, "state": "ready", "src": src(true) })
                    }
                    Some((status, _, updated))
                        if status == "running" && crate::state::now_secs() - updated < 30 * 60 =>
                    {
                        json!({ "kind": "video", "direct": false, "state": "converting" })
                    }
                    _ => {
                        media::ensure_transcode(&st, share_id, &spot.rel, &spot.full).await?;
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
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    asset(&st, &identity, share_id, &q.path, true, "仅视频支持封面").await
}

pub async fn image_thumb(
    State(st): State<AppState>,
    identity: Identity,
    Path(share_id): Path<i64>,
    Query(q): Query<MediaQ>,
) -> Response {
    asset(
        &st,
        &identity,
        share_id,
        &q.path,
        false,
        "非图片无需生成缩略图",
    )
    .await
}

async fn asset(
    st: &AppState,
    identity: &Identity,
    share_id: i64,
    path: &str,
    is_video: bool,
    wrong_kind: &str,
) -> Response {
    let spot = match locate_readable(st, identity, share_id, path).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    if !spot.full.is_file() {
        return not_found_body("文件不存在");
    }
    let ext = paths::ext_of(&spot.rel);
    let ok_kind = if is_video {
        media::kind_of(&ext) == media::Kind::Video
    } else {
        media::kind_of(&ext) == media::Kind::Image
    };
    if !ok_kind {
        return err(StatusCode::BAD_REQUEST, wrong_kind);
    }
    let out = if is_video {
        media::ensure_video_thumb(st, share_id, &spot.rel, &spot.full).await
    } else {
        media::ensure_image_thumb(st, share_id, &spot.rel, &spot.full).await
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
