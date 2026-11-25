//! HTTP(S) 直链引擎。
//!
//! 手动跟随最多 5 跳重定向并逐跳校验目标地址;≥8MB 且支持 Range 时
//! 最多 8 段并行(段任务由 JoinSet 管理,带停滞检测与超时),否则单连接流式写盘。
//! 完成后 rename 到 个人空间/下载/<name>,并登记 file_meta 归属。

use futures_core::Stream as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::app::{now_secs, App};
use crate::domain::human_bytes;

/// 从 Content-Disposition 提取文件名(filename* 优先)。
fn content_name(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let cd = headers.get("Content-Disposition")?.to_str().ok()?;
    for part in cd.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename*=UTF-8''") {
            return Some(percent_encoding::percent_decode_str(v).decode_utf8_lossy().to_string());
        }
        if let Some(v) = part.strip_prefix("filename=") {
            return Some(v.trim_matches('"').to_string());
        }
    }
    None
}

/// HTTP 下载并发段数("下载线程数",后台可调 1-32,默认 8;1 = 始终单线程)。
pub const HTTP_THREADS_KEY: &str = "offline_http_threads";
pub const HTTP_THREADS_DEFAULT: i64 = 8;

pub async fn thread_count(app: &App) -> u32 {
    app.db
        .setting(HTTP_THREADS_KEY)
        .await
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(HTTP_THREADS_DEFAULT)
        .clamp(1, 32) as u32
}

pub async fn run_direct_task(app: App, task_id: i64, url: String, target_dir: PathBuf, share_host: PathBuf, user_id: i64) {
    use tokio::io::AsyncWriteExt;
    let cancel = app.hub.register(task_id).await;
    app.hub.update(&app, user_id, task_id, json!({"state": "downloading"})).await;

    // 不自动跟随重定向:每一跳都要过一遍内网地址校验。
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return fail(&app, user_id, task_id, format!("初始化失败: {e}")).await,
    };

    let mut current = url.clone();
    let mut response = None;
    for _ in 0..5 {
        if cancel.load(Ordering::Relaxed) {
            return cancelled(&app, user_id, task_id).await;
        }
        if let Err(e) = super::validate_url(&app, &current).await {
            return fail(&app, user_id, task_id, e).await;
        }
        let resp = match client.get(&current).header("Range", "bytes=0-1").send().await {
            Ok(r) => r,
            Err(e) => return fail(&app, user_id, task_id, format!("连接失败: {e}")).await,
        };
        if resp.status().is_redirection() {
            let Some(loc) = resp.headers().get("Location").and_then(|v| v.to_str().ok()) else {
                return fail(&app, user_id, task_id, "重定向缺少 Location".into()).await;
            };
            current = match reqwest::Url::parse(&current).unwrap().join(loc) {
                Ok(u) => u.to_string(),
                Err(e) => return fail(&app, user_id, task_id, format!("重定向地址无效: {e}")).await,
            };
            continue;
        }
        response = Some(resp);
        break;
    }
    let resp = match response {
        Some(r) => r,
        None => return fail(&app, user_id, task_id, "重定向次数过多".into()).await,
    };
    let final_url = resp.url().to_string();
    if let Err(e) = super::validate_url(&app, &final_url).await {
        return fail(&app, user_id, task_id, e).await;
    }
    if !resp.status().is_success() {
        return fail(&app, user_id, task_id, format!("服务器返回 {}", resp.status())).await;
    }
    let range_total: u64 = resp
        .headers()
        .get("content-range")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next())
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    let range_ok = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT && range_total > 0;
    let total = if range_ok { range_total } else { resp.content_length().unwrap_or(0) };
    let limit = super::size_limit(&app).await;
    if total > limit as u64 {
        return fail(&app, user_id, task_id, format!("文件超过单任务上限({})", human_bytes(limit))).await;
    }

    let name = content_name(resp.headers())
        .or_else(|| {
            reqwest::Url::parse(&final_url)
                .ok()?
                .path_segments()?
                .rev()
                .find(|s| !s.is_empty())
                .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string())
        })
        .filter(|s| !s.is_empty())
        .map(|n| super::safe_name(&n, task_id))
        .unwrap_or_else(|| format!("download-{task_id}"));

    let tmp = app.tmp_dir.join(format!("offline-{task_id}.part"));

    let threads = thread_count(&app).await;
    if range_ok && total >= 8 * 1024 * 1024 && threads > 1 {
        let ctx = MultiCtx {
            app: &app,
            user_id,
            task_id,
            url: &final_url,
            tmp: &tmp,
            total,
            threads,
        };
        if let Err(e) = run_http_multi(ctx, &cancel).await {
            return fail(&app, user_id, task_id, e).await;
        }
    } else {
        // 单连接流式写盘。
        let mut f = match tokio::fs::File::create(&tmp).await {
            Ok(f) => f,
            Err(e) => return fail(&app, user_id, task_id, format!("创建临时文件失败: {e}")).await,
        };
        let mut downloaded: u64 = 0;
        let mut last_update = now_secs();
        let mut stream = std::pin::pin!(resp.bytes_stream());
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = tokio::fs::remove_file(&tmp).await;
                return cancelled(&app, user_id, task_id).await;
            }
            let chunk = match std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
                Some(Ok(c)) => c,
                Some(Err(e)) => {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return fail(&app, user_id, task_id, format!("下载中断: {e}")).await;
                }
                None => break,
            };
            if let Err(e) = f.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&tmp).await;
                return fail(&app, user_id, task_id, format!("写入失败: {e}")).await;
            }
            downloaded += chunk.len() as u64;
            if downloaded > limit as u64 {
                let _ = tokio::fs::remove_file(&tmp).await;
                return fail(&app, user_id, task_id, format!("超过单任务上限({})", human_bytes(limit))).await;
            }
            if now_secs() - last_update >= 1 {
                last_update = now_secs();
                app.hub.progress(&app, user_id, task_id, downloaded as i64, total as i64).await;
            }
        }
        let _ = f.flush().await;
        drop(f);
    }

    app.hub
        .update(&app, user_id, task_id, json!({"size": total as i64, "downloaded": total as i64, "name": name}))
        .await;

    let final_path = super::unique_name(&target_dir, &name);
    let _ = tokio::fs::create_dir_all(&target_dir).await;
    if let Err(e) = tokio::fs::rename(&tmp, &final_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return fail(&app, user_id, task_id, format!("移入网盘失败: {e}")).await;
    }
    let rel = final_path
        .strip_prefix(&share_host)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| name.clone());
    let _ = app.db.set_meta_owner(containing_share(&app, &target_dir).await, &rel, user_id).await;
    app.hub
        .set_done(&app, user_id, task_id, json!({"size": total as i64, "downloaded": total as i64, "name": name}))
        .await;
    tracing::info!("[离线下载] HTTP 任务完成: {rel}");
}

/// 找到包含该目录的共享 id(用于登记条目归属)。
async fn containing_share(app: &App, dir: &std::path::Path) -> i64 {
    if let Ok(shares) = app.db.all_shares().await {
        for s in shares {
            let root = std::path::PathBuf::from(&s.host_path);
            if dir.starts_with(&root) {
                return s.id;
            }
        }
    }
    0
}

/// 多段下载的参数包(收敛 run_http_multi 的参数数量)。
struct MultiCtx<'a> {
    app: &'a App,
    user_id: i64,
    task_id: i64,
    url: &'a str,
    tmp: &'a std::path::Path,
    total: u64,
    threads: u32,
}

async fn run_http_multi(ctx: MultiCtx<'_>, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let MultiCtx { app, user_id, task_id, url, tmp, total, threads } = ctx;
    // 段数 = min(线程数, 每 1MB 一段的份数);不足 2 段没有并行意义(外层已保证 ≥8MB)。
    let seg_count = (threads as u64).min(total / (1024 * 1024)).max(2) as usize;
    let seg_len = total.div_ceil(seg_count as u64);
    if let Err(e) = tokio::fs::File::create(tmp).await {
        return Err(format!("创建临时文件失败: {e}"));
    }
    let received = Arc::new(AtomicU64::new(0));
    let client = Arc::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("初始化失败: {e}"))?,
    );

    // 每段一个专属 worker:失败从段内断点自动续传重试,无需跨 worker 协调。
    // 全局兜底:主循环的停滞检测(60s 无字节)与 6h 超时。
    let mut join: tokio::task::JoinSet<Result<(), String>> = tokio::task::JoinSet::new();
    for i in 0..seg_count {
        let start = i as u64 * seg_len;
        let end = (start + seg_len).min(total);
        if start < end {
            join.spawn(seg_worker(
                Arc::clone(&client),
                url.to_string(),
                tmp.to_path_buf(),
                start,
                end,
                Arc::clone(&received),
                Arc::clone(cancel),
            ));
        }
    }
    let deadline = now_secs() + 6 * 3600;
    let mut last_progress = 0u64;
    let mut stalls = 0u32;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut failure: Option<String> = None;
    while !join.is_empty() {
        tokio::select! {
            _ = tick.tick() => {
                if cancel.load(Ordering::Relaxed) || app.hub.is_cancelled(task_id).await {
                    join.abort_all();
                    return Err("已取消".into());
                }
                if now_secs() >= deadline {
                    join.abort_all();
                    return Err("下载超时".into());
                }
                let dl = received.load(Ordering::Relaxed).min(total);
                app.hub.progress(app, user_id, task_id, dl as i64, total as i64).await;
                // 60 秒没有任何字节推进视为停滞。
                if dl <= last_progress {
                    stalls += 1;
                    if stalls >= 60 {
                        join.abort_all();
                        return Err("分段下载停滞".into());
                    }
                } else {
                    stalls = 0;
                    last_progress = dl;
                }
            }
            res = join.join_next() => {
                match res {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(e))) => {
                        // 段重试耗尽(网络不可恢复),整体失败;其余段不再有意义。
                        tracing::info!("[离线下载] HTTP 段失败: {e}");
                        failure = Some(e);
                        join.abort_all();
                    }
                    Some(Err(e)) => {
                        failure = Some(format!("段任务异常: {e}"));
                        join.abort_all();
                    }
                    None => break,
                }
            }
        }
    }
    if let Some(e) = failure {
        return Err(e);
    }
    if received.load(Ordering::Relaxed) < total {
        return Err("分段下载字节数不完整".into());
    }
    app.hub.progress(app, user_id, task_id, total as i64, total as i64).await;
    Ok(())
}

/// 下载一个段:断点续传(`pos` 保留),网络故障时循环重试,
/// 直到完成 / 取消 / 上层停滞或超时兜底。
async fn seg_worker(
    client: Arc<reqwest::Client>,
    url: String,
    tmp: PathBuf,
    start: u64,
    end: u64,
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let mut f = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&tmp)
        .await
        .map_err(|e| format!("打开临时文件失败: {e}"))?;
    let mut pos = start;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        let resp = match client
            .get(&url)
            .header("Range", format!("bytes={}-{}", pos, end - 1))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::info!("[离线下载] HTTP 段连接失败(将重试): {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("分段请求失败: {}", resp.status()));
        }
        let mut stream = std::pin::pin!(resp.bytes_stream());
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }
            // 单块 30 秒无数据视为该次连接中断,回到外层从 pos 续传。
            let chunk = match tokio::time::timeout(
                Duration::from_secs(30),
                std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)),
            )
            .await
            {
                Ok(Some(Ok(c))) => c,
                Ok(Some(Err(e))) => {
                    tracing::info!("[离线下载] HTTP 段中断(将续传): {e}");
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::info!("[离线下载] HTTP 段停滞(将续传)");
                    break;
                }
            };
            if pos + chunk.len() as u64 > end {
                return Err("分段数据越界".into());
            }
            if let Err(e) = f.seek(std::io::SeekFrom::Start(pos)).await {
                return Err(format!("seek 失败: {e}"));
            }
            if let Err(e) = f.write_all(&chunk).await {
                return Err(format!("写盘失败: {e}"));
            }
            pos += chunk.len() as u64;
            received.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            if pos >= end {
                break;
            }
        }
        if pos >= end {
            f.flush().await.map_err(|e| format!("写盘失败: {e}"))?;
            return Ok(());
        }
    }
}

async fn fail(app: &App, user_id: i64, task_id: i64, msg: String) {
    tracing::info!("[离线下载] HTTP 任务失败: {msg}");
    let tmp = app.tmp_dir.join(format!("offline-{task_id}.part"));
    let _ = tokio::fs::remove_file(&tmp).await;
    super::set_state(app, user_id, task_id, "failed", Some(msg)).await;
}

async fn cancelled(app: &App, user_id: i64, task_id: i64) {
    super::set_state(app, user_id, task_id, "cancelled", None).await;
}
