use axum::extract::State;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::Identity;
use crate::domain::media;
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

/// 运行状态 / 硬件信息 / 缓存占用(仅管理员)。
pub async fn status(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    identity.require_adminish()?;
    let cache = media::stats(&st).await;
    let tr = st.db.setting("transcode_replace").await.as_deref() == Some("1");
    Ok(ok(json!({
        "ffmpeg": st.ffmpeg.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        "cache": cache,
        "transcode_replace": tr,
        "chunk_size": st.chunk_size,
        "data_dir": st
            .users_dir
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
    })))
}

/// 清空缩略图 / 转码缓存(仅管理员);播放时需要时再自动生成。
pub async fn clear_cache(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    identity.require_adminish()?;
    let freed = media::clear_all(&st).await;
    let cache = media::stats(&st).await;
    Ok(ok(json!({ "freed": freed, "cache": cache })))
}

#[derive(Deserialize)]
pub struct ReplaceReq {
    enabled: bool,
}

/// 上传视频自动转码替换开关(仅管理员,默认关闭)。
pub async fn set_transcode_replace(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<ReplaceReq>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    st.db
        .set_setting("transcode_replace", if req.enabled { "1" } else { "0" })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(ok(json!({ "ok": true, "enabled": req.enabled })))
}
