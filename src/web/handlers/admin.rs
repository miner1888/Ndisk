//! 管理后台:运行状态 / 缓存清理 / 离线下载设置 / 自动转码开关。

use axum::extract::State;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::App;
use crate::domain::{human_bytes, media};
use crate::engine::ed2k::{self, status_fields as kad_status_fields};
use crate::engine::hub;
use crate::error::{ok, ApiError, ApiResult};
use crate::web::identity::Identity;

pub async fn status(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    identity.require_admin()?;
    let cache = media::stats(&app.db, &app.media_dir).await;
    // 展示与实际行为一致:未设置时默认关闭(与 maybe_replace_transcode 的默认相同)。
    let tr = app.db.setting("transcode_replace").await.as_deref() == Some("1");
    let offline_limit = app
        .db
        .setting(hub::OFFLINE_SIZE_LIMIT_KEY)
        .await
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(hub::DEFAULT_SIZE_LIMIT);
    let allow_private = app.db.setting("offline_allow_private").await.as_deref() != Some("0");
    // 展示用的服务器列表:剔除占位项;为空时回默认列表。
    let ed2k_servers: String = app
        .db
        .setting(ed2k::ED2K_SERVERS_KEY)
        .await
        .unwrap_or_else(ed2k::default_servers)
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.to_lowercase().starts_with("public-ip-"))
        .collect::<Vec<_>>()
        .join(",");
    let ed2k_servers = if ed2k_servers.is_empty() { ed2k::default_servers() } else { ed2k_servers };
    let kad = kad_status_fields(&app).await;
    let http_threads = crate::engine::direct::thread_count(&app).await;
    Ok(ok(json!({
        "ffmpeg": app.ffmpeg.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        "cache": cache,
        "transcode_replace": tr,
        "offline_limit": offline_limit,
        "offline_allow_private": allow_private,
        "bt_seed": app.db.setting("offline_bt_seed").await.as_deref() == Some("1"),
        "http_threads": http_threads,
        "ed2k_servers": ed2k_servers,
        "kad_enabled": kad["kad_enabled"],
        "ed2k_concurrency": ed2k::concurrency(&app).await,
        "kad_nodes_url": kad["kad_nodes_url"],
        "tracker_sub_url": crate::engine::trackers::status_fields(&app).await["tracker_sub_url"],
        "chunk_size": app.chunk_size,
        "data_dir": app.data_dir.to_string_lossy().to_string(),
    })))
}

pub async fn clear_cache(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    identity.require_admin()?;
    let freed = media::clear_all(&app.db, &app.media_dir).await;
    let cache = media::stats(&app.db, &app.media_dir).await;
    Ok(ok(json!({ "freed": freed, "cache": cache })))
}

#[derive(Deserialize)]
pub struct ReplaceReq {
    enabled: bool,
}

pub async fn set_transcode_replace(
    State(app): State<App>,
    identity: Identity,
    Json(req): Json<ReplaceReq>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    app.db
        .set_setting("transcode_replace", if req.enabled { "1" } else { "0" })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(ok(json!({ "ok": true, "enabled": req.enabled })))
}

#[derive(Deserialize, Default)]
pub struct OfflineSetReq {
    ed2k_servers: Option<String>,
    kad_enabled: Option<bool>,
    kad_nodes_url: Option<String>,
    tracker_sub_url: Option<String>,
    ed2k_concurrency: Option<i64>,
    limit: Option<i64>,
    allow_private: Option<bool>,
    bt_seed: Option<bool>,
    http_threads: Option<i64>,
}

pub async fn set_offline_settings(
    State(app): State<App>,
    identity: Identity,
    Json(req): Json<OfflineSetReq>,
) -> ApiResult<Response> {
    identity.require_admin()?;
    if let Some(limit) = req.limit {
        if !(1024 * 1024..=1024i64 * 1024 * 1024 * 1024).contains(&limit) {
            return Err(ApiError::bad_request(format!(
                "单任务上限需在 1MB ~ 1TB 之间(当前 {})",
                human_bytes(limit)
            )));
        }
        app.db
            .set_setting(hub::OFFLINE_SIZE_LIMIT_KEY, &limit.to_string())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(sv) = req.ed2k_servers {
        for item in sv.split([',', ';', '\n']).map(str::trim).filter(|s| !s.is_empty()) {
            let bad = item.rsplit_once(':').map(|(h, p)| {
                p.parse::<u16>().is_err() || h.is_empty() || h.contains(['/', ' ', '|'])
            });
            if bad == Some(true) || bad.is_none() {
                return Err(ApiError::bad_request(format!("eD2K 服务器格式无效: {item}(应为 ip:port)")));
            }
        }
        app.db
            .set_setting(ed2k::ED2K_SERVERS_KEY, sv.trim())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(on) = req.kad_enabled {
        app.db
            .set_setting(ed2k::KAD_ENABLED_KEY, if on { "1" } else { "0" })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(url) = req.kad_nodes_url {
        let t = url.trim().to_string();
        if !t.is_empty() && !t.starts_with("http://") && !t.starts_with("https://") {
            return Err(ApiError::bad_request("nodes.dat 地址需以 http(s):// 开头"));
        }
        app.db
            .set_setting(ed2k::KAD_NODES_URL_KEY, &t)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(url) = req.tracker_sub_url {
        let t = url.trim().to_string();
        let bad = t
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .any(|s| !s.starts_with("http://") && !s.starts_with("https://"));
        if bad {
            return Err(ApiError::bad_request("tracker 订阅地址需以 http(s):// 开头(多个用逗号分隔)"));
        }
        app.db
            .set_setting(crate::engine::trackers::TRACKER_SUB_KEY, &t)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(c) = req.ed2k_concurrency {
        if !(1..=16).contains(&c) {
            return Err(ApiError::bad_request("eD2K 并行来源数需在 1-16 之间"));
        }
        app.db
            .set_setting(ed2k::ED2K_CONCURRENCY_KEY, &c.to_string())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(t) = req.http_threads {
        if !(1..=32).contains(&t) {
            return Err(ApiError::bad_request("下载线程数需在 1-32 之间"));
        }
        app.db
            .set_setting(crate::engine::direct::HTTP_THREADS_KEY, &t.to_string())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(allow) = req.allow_private {
        app.db
            .set_setting("offline_allow_private", if allow { "1" } else { "0" })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    if let Some(seed) = req.bt_seed {
        app.db
            .set_setting("offline_bt_seed", if seed { "1" } else { "0" })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(ok(json!({ "ok": true })))
}
