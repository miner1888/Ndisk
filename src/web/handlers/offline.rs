//! 离线下载 API:添加 / 文件选择 / 列表 / 取消 / 移除 / SSE 事件流。

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::{response::Response, Json};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;

use crate::app::App;
use crate::db::Share;
use crate::domain::pathguard;
use crate::error::{ok, ApiError, ApiResult};
use crate::web::identity::Identity;

async fn home_share(app: &App, user_id: i64) -> ApiResult<Share> {
    app.db
        .all_shares()
        .await?
        .into_iter()
        .find(|s| s.kind == "home" && s.owner_id == Some(user_id))
        .ok_or_else(|| ApiError::forbidden("当前账号没有个人空间"))
}

/// BT 直链(.torrent URL)由 BT 引擎拉取,与 HTTP 直链做同样的协议/内网防护;
/// 磁力 / 电驴 / 上传种子不需要。
async fn ensure_url_allowed(app: &App, kind: &str, url: &str) -> ApiResult<()> {
    if kind == "bittorrent" && (url.starts_with("http://") || url.starts_with("https://")) {
        crate::engine::validate_url(app, url).await.map_err(ApiError::bad_request)?;
    }
    Ok(())
}

/// 按任务类型启动对应引擎 worker(add 与 retry 共用唯一出口)。
fn spawn_by_kind(app: &App, user_id: i64, task_id: i64, kind: &str, url: &str, target_dir: &std::path::Path, host: &std::path::Path) {
    match kind {
        "bittorrent" => {
            let a1 = app.clone();
            let a2 = app.clone();
            crate::engine::spawn_task(a1, user_id, task_id, "BT 下载", crate::engine::run_torrent_task(a2, task_id, url.to_string(), user_id));
        }
        "ed2k" => {
            let a1 = app.clone();
            let a2 = app.clone();
            crate::engine::spawn_task(a1, user_id, task_id, "eD2K 下载", crate::engine::ed2k::run_ed2k_task(a2, task_id, url.to_string(), user_id));
        }
        _ => {
            let a1 = app.clone();
            let a2 = app.clone();
            crate::engine::spawn_task(
                a1,
                user_id,
                task_id,
                "HTTP 直链下载",
                crate::engine::run_direct_task(a2, task_id, url.to_string(), target_dir.to_path_buf(), host.to_path_buf(), user_id),
            );
        }
    }
}

#[derive(Deserialize)]
pub struct AddReq {
    #[serde(default)]
    dir: String,
    url: String,
}

pub async fn add(State(app): State<App>, identity: Identity, Json(req): Json<AddReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    if !user.can_upload {
        return Err(ApiError::forbidden("当前账号无上传权限"));
    }
    let (url, kind) = crate::engine::normalize_url(&req.url).map_err(ApiError::bad_request)?;
    let kind = match kind {
        "bt" => "bittorrent",
        "ed2k" => "ed2k",
        _ => "http",
    };
    let share = home_share(&app, user.id).await?;
    let rel_dir = if req.dir.trim().is_empty() {
        "下载".to_string()
    } else {
        pathguard::check_name(req.dir.trim()).map_err(ApiError::bad_request)?
    };
    let host = std::path::PathBuf::from(&share.host_path);
    let target_dir = host.join(&rel_dir);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;

    ensure_url_allowed(&app, kind, &url).await?;
    let task_id = app
        .db
        .insert_offline_task(user.id, share.id, &rel_dir, &url, kind)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    spawn_by_kind(&app, user.id, task_id, kind, &url, &target_dir, &host);
    Ok(ok(json!({ "ok": true, "id": task_id, "kind": kind, "dir": rel_dir })))
}

/// 重新下载:用失败任务记录的原始链接新建一条任务(旧行留作历史)。
/// 仅任务所有者可操作,且只有 failed 状态可重试。
pub async fn retry(State(app): State<App>, identity: Identity, Json(req): Json<TaskReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    let task = app
        .db
        .offline_task(req.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if task.get("userId").and_then(|v| v.as_i64()) != Some(user.id) {
        return Err(ApiError::forbidden("无权操作该任务"));
    }
    if task.get("state").and_then(|v| v.as_str()) != Some("failed") {
        return Err(ApiError::conflict("只有失败的任务可以重新下载"));
    }
    let url = task.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let kind = task.get("kind").and_then(|v| v.as_str()).unwrap_or("http").to_string();
    let rel_dir = task.get("relDir").and_then(|v| v.as_str()).unwrap_or("下载").to_string();
    if url.is_empty() {
        return Err(ApiError::bad_request("任务缺少原始链接,无法重新下载"));
    }
    let share = home_share(&app, user.id).await?;
    let host = std::path::PathBuf::from(&share.host_path);
    let target_dir = host.join(&rel_dir);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;

    ensure_url_allowed(&app, &kind, &url).await?;
    let new_id = app
        .db
        .insert_offline_task(user.id, share.id, &rel_dir, &url, &kind)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    spawn_by_kind(&app, user.id, new_id, &kind, &url, &target_dir, &host);
    tracing::info!("[离线下载] 任务 {new_id} 由失败任务 {req_id} 重新下载", req_id = req.id);
    Ok(ok(json!({ "ok": true, "id": new_id, "kind": kind, "dir": rel_dir, "retried": req.id })))
}

/// 种子文件上传建任务:内容级校验(必须是可解析的 BT 元数据)→ 落盘 tmp →
/// 交给 BT 引擎统一流程(解析清单 → 勾选 → 下载 → 移入网盘)。
pub async fn add_torrent(
    State(app): State<App>,
    identity: Identity,
    mut mp: axum::extract::Multipart,
) -> ApiResult<Response> {
    const TORRENT_MAX: usize = 16 * 1024 * 1024;
    let user = identity.require()?;
    if !user.can_upload {
        return Err(ApiError::forbidden("当前账号无上传权限"));
    }
    let share = home_share(&app, user.id).await?;
    let mut dir = String::new();
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("上传数据无效: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "dir" => dir = field.text().await.unwrap_or_default(),
            "file" => {
                let name = field.file_name().unwrap_or_default().trim().to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request(format!("种子文件读取失败: {e}")))?;
                upload = Some((name, data.to_vec()));
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    let (_, data) = upload.ok_or_else(|| ApiError::bad_request("请选择 .torrent 文件"))?;
    if data.is_empty() || data.len() > TORRENT_MAX {
        return Err(ApiError::bad_request("种子文件需在 16MB 以内"));
    }
    let (torrent_name, magnet) =
        crate::engine::torrent::parse_torrent_upload(&data).map_err(ApiError::bad_request)?;
    let rel_dir = if dir.trim().is_empty() {
        "下载".to_string()
    } else {
        pathguard::check_name(dir.trim()).map_err(ApiError::bad_request)?
    };
    let host = std::path::PathBuf::from(&share.host_path);
    tokio::fs::create_dir_all(host.join(&rel_dir))
        .await
        .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;

    let task_id = app
        .db
        .insert_offline_task(user.id, share.id, &rel_dir, &magnet, "bittorrent")
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    // 展示名立即落库(种子 info.name),避免解析完成前列表显示长磁力链接。
    app.db
        .update_offline_task(task_id, json!({ "name": torrent_name }))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let tmp_file = app.tmp_dir.join(format!("bt-upload-{task_id}.torrent"));
    if tokio::fs::write(&tmp_file, &data).await.is_err() {
        crate::engine::set_state(&app, user.id, task_id, "failed", Some("种子文件保存失败".into())).await;
        return Err(ApiError::internal("种子文件保存失败"));
    }
    let app2 = app.clone();
    let app3 = app.clone();
    let uid = user.id;
    let path2 = tmp_file.display().to_string();
    crate::engine::spawn_task(app2, uid, task_id, "BT 下载", crate::engine::run_torrent_task(app3, task_id, path2, uid));
    tracing::info!("[离线下载] 种子上传任务 {task_id}: {torrent_name}");
    Ok(ok(json!({ "ok": true, "id": task_id, "kind": "bittorrent", "dir": rel_dir, "name": torrent_name })))
}

#[derive(Deserialize)]
pub struct SelectReq {
    id: i64,
    #[serde(default)]
    selected: Vec<usize>,
}

/// 磁力多文件任务:把用户勾选写入 files 字段并置为 downloading。
pub async fn select(State(app): State<App>, identity: Identity, Json(req): Json<SelectReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    let task = app
        .db
        .offline_task(req.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if task.get("userId").and_then(|v| v.as_i64()) != Some(user.id) {
        return Err(ApiError::forbidden("无权操作该任务"));
    }
    if task.get("state").and_then(|v| v.as_str()) != Some("awaiting_select") {
        return Err(ApiError::conflict("任务当前不接受文件选择"));
    }
    if req.selected.is_empty() {
        return Err(ApiError::bad_request("请至少勾选一个要下载的文件"));
    }
    let files = task.get("files").cloned().unwrap_or(json!([]));
    let files = match files {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(&s).unwrap_or(json!([])),
        v => v,
    };
    let files = if let serde_json::Value::Array(arr) = files {
        let sel: std::collections::HashSet<usize> = req.selected.iter().copied().collect();
        serde_json::Value::Array(
            arr.into_iter()
                .map(|mut f| {
                    let idx = f.get("idx").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                    if let Some(obj) = f.as_object_mut() {
                        obj.insert("selected".into(), json!(sel.contains(&(idx as usize))));
                    }
                    f
                })
                .collect(),
        )
    } else {
        files
    };
    app.db
        .update_offline_task(
            req.id,
            json!({"state": "downloading", "files": serde_json::to_string(&files).unwrap_or_default()}),
        )
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(ok(json!({ "ok": true })))
}

pub async fn list(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    let user = identity.require()?;
    let tasks = app
        .db
        .offline_tasks_of(user.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(ok(json!({ "tasks": tasks })))
}

#[derive(Deserialize)]
pub struct TaskReq {
    id: i64,
}

pub async fn remove(State(app): State<App>, identity: Identity, Json(req): Json<TaskReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    let task = app
        .db
        .offline_task(req.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if task.get("userId").and_then(|v| v.as_i64()) != Some(user.id) {
        return Err(ApiError::forbidden("无权操作该任务"));
    }
    let state = task.get("state").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(state, "downloading" | "parsing" | "awaiting_select") {
        // 活跃任务:取消而不是删除。
        app.hub.cancel(req.id).await;
        crate::engine::set_state(&app, user.id, req.id, "cancelled", None).await;
        return Ok(ok(json!({ "ok": true, "removed": false })));
    }
    app.hub.cancel(req.id).await;
    app.db.delete_offline_task(req.id).await.map_err(|e| ApiError::internal(e.to_string()))?;
    app.hub.removed(user.id, req.id).await;
    Ok(ok(json!({ "ok": true, "removed": true })))
}

pub async fn cancel(State(app): State<App>, identity: Identity, Json(req): Json<TaskReq>) -> ApiResult<Response> {
    let user = identity.require()?;
    let task = app
        .db
        .offline_task(req.id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("任务不存在"))?;
    if task.get("userId").and_then(|v| v.as_i64()) != Some(user.id) {
        return Err(ApiError::forbidden("无权操作该任务"));
    }
    let _ = app.db.update_offline_task(req.id, json!({"state": "cancelled"})).await;
    app.hub.cancel(req.id).await;
    Ok(ok(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct EventsQ {
    #[serde(default)]
    token: Option<String>,
}

pub async fn events(
    State(app): State<App>,
    identity: Identity,
    axum::extract::Query(q): axum::extract::Query<EventsQ>,
) -> Result<Sse<impl futures_util::stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // EventSource 无法自定义请求头,允许用 ?token= 传会话 token。
    let user = match q.token.filter(|t| !t.trim().is_empty()) {
        Some(t) => app
            .db
            .user_by_token(t.trim())
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?
            .ok_or_else(|| ApiError::unauthorized("需要登录"))?,
        None => identity.require()?.clone(),
    };
    let rx = app.hub.subscribe();
    let uid = user.id;
    let stream = futures_util::stream::unfold(
        (rx, uid, true),
        move |(mut rx, uid, mut first)| async move {
            if first {
                first = false;
                return Some((Ok(Event::default().data("{\"type\":\"snapshot\"}")), (rx, uid, first)));
            }
            match rx.recv().await {
                Ok(ev) if ev.user_id() == uid => {
                    Some((Ok(Event::default().data(ev.to_json())), (rx, uid, first)))
                }
                Ok(_) => Some((Ok(Event::default().data("{\"type\":\"ping\"}")), (rx, uid, first))),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    Some((Ok(Event::default().data("{\"type\":\"resync\"}")), (rx, uid, first)))
                }
                Err(_) => None,
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}
