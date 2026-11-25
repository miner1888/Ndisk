//! eD2K 子系统:任务执行器、链接解析 / 服务器查源、对端会话、KAD 查源。
//!
//! 语义约定(与历史版本一致):
//! - **纯下载**:只发起出站连接请求分片,不开监听端口、不向服务器登记文件、
//!   不发送任何上传 / 数据帧(端到端测试会断言只出现请求类操作码)。
//! - 下载完成后按"逐块 MD4 + 根哈希(MD4(块哈希拼接))"全量校验。

mod kad;
mod link;
mod peer;

pub use link::{concurrency, default_servers, parse_link, ED2K_CONCURRENCY_KEY, ED2K_SERVERS_KEY};
pub use peer::{part_count, PART_SIZE};
pub use kad::{kad_enabled, status_fields, KAD_ENABLED_KEY, KAD_NODES_URL_KEY};

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::app::{now_secs, App};
use crate::domain::human_bytes;

use link::{md4, Ed2kShared};

/// 任务主流程:查源 → 并发块下载 → 全量校验 → 移入网盘。
pub async fn run_ed2k_task(app: App, task_id: i64, url: String, user_id: i64) {
    async fn state_of(app: &App, task_id: i64) -> String {
        app.db
            .offline_task(task_id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.get("state").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_default()
    }
    async fn fail(app: &App, user_id: i64, task_id: i64, tmp: &PathBuf, msg: String) {
        let _ = tokio::fs::remove_file(tmp).await;
        super::set_state(app, user_id, task_id, "failed", Some(msg)).await;
    }

    let Ok(link) = super::normalize_url(&url).map(|x| x.0) else {
        return fail(&app, user_id, task_id, &PathBuf::new(), "链接无效".into()).await;
    };
    let Ok(l) = parse_link(&link) else {
        return fail(&app, user_id, task_id, &PathBuf::new(), "链接无效".into()).await;
    };
    let limit = super::size_limit(&app).await;
    if l.size > limit {
        return fail(&app, user_id, task_id, &PathBuf::new(), format!("文件超过单任务上限({})", human_bytes(limit))).await;
    }
    app.hub.update(&app, user_id, task_id, json!({"name": l.name, "size": l.size, "downloaded": 0})).await;

    let tmp = app.tmp_dir.join(format!("ed2k-{task_id}.part"));
    let Ok(part) = tokio::fs::File::create(&tmp).await else {
        return fail(&app, user_id, task_id, &PathBuf::new(), "创建临时文件失败".into()).await;
    };
    let parts_total = peer::part_count(l.size);
    let shared = std::sync::Arc::new(Ed2kShared {
        file: tokio::sync::Mutex::new(part),
        parts: tokio::sync::Mutex::new(vec![link::Ed2kPart::default(); parts_total]),
        hashset: tokio::sync::OnceCell::new(),
        received: std::sync::atomic::AtomicU64::new(0),
        fails: tokio::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let deadline = now_secs() + 6 * 3600;
    let mut pool: std::collections::VecDeque<(u32, u16)> = l.sources.iter().copied().collect();
    let mut join: tokio::task::JoinSet<Result<(), String>> = tokio::task::JoinSet::new();
    let mut last_update = 0i64;
    let mut have_sources = !pool.is_empty();
    let mut next_requery = 0i64;

    loop {
        if app.hub.is_cancelled(task_id).await || state_of(&app, task_id).await == "cancelled" {
            join.abort_all();
            let _ = tokio::fs::remove_file(&tmp).await;
            super::set_state(&app, user_id, task_id, "cancelled", None).await;
            return;
        }
        let done_count = {
            let ps = shared.parts.lock().await;
            ps.iter().filter(|p| p.done).count()
        };
        if done_count >= parts_total {
            break;
        }
        if !have_sources {
            match link::find_sources(&app, &l.hash, l.size).await {
                Ok(v) if !v.is_empty() => {
                    pool.extend(v);
                    have_sources = true;
                    let _ = app.db.update_offline_task(task_id, json!({"state": "downloading"})).await;
                    app.hub.update(&app, user_id, task_id, json!({"state": "downloading"})).await;
                }
                Ok(_) => return fail(&app, user_id, task_id, &tmp, "未找到下载来源".into()).await,
                Err(e) => return fail(&app, user_id, task_id, &tmp, format!("{e}")).await,
            }
        } else if join.is_empty() && pool.is_empty() {
            // 来源耗尽但块未齐:90 秒后重新查源;超时则失败。
            if now_secs() >= deadline {
                return fail(&app, user_id, task_id, &tmp, "下载超时,来源不足".into()).await;
            }
            if next_requery == 0 {
                next_requery = now_secs() + 90;
                tracing::info!(
                    "[离线下载] eD2K 来源耗尽({}/{} 块),90s 后重新查源",
                    done_count,
                    parts_total
                );
            }
            if now_secs() >= next_requery {
                have_sources = false;
                next_requery = 0;
                continue;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        if state_of(&app, task_id).await == "parsing" {
            let _ = app.db.update_offline_task(task_id, json!({"state": "downloading"})).await;
            app.hub.update(&app, user_id, task_id, json!({"state": "downloading"})).await;
        }
        // 并发拉起来源 worker;连续失败 2 次的来源拉黑。
        let conc = concurrency(&app).await;
        while join.len() < conc {
            let Some(addr) = pool.pop_front() else { break };
            let fails = shared.fails.lock().await.get(&addr).copied().unwrap_or(0);
            if fails >= 2 {
                continue;
            }
            join.spawn(link::peer_worker(
                addr,
                l.hash,
                l.size,
                parts_total,
                std::sync::Arc::clone(&shared),
            ));
        }
        if join.is_empty() {
            tokio::time::sleep(Duration::from_millis(300)).await;
        } else {
            match tokio::time::timeout(Duration::from_millis(1000), join.join_next()).await {
                Ok(Some(res)) => {
                    if let Ok(Err(e)) = res {
                        tracing::info!("[离线下载] eD2K 来源退出: {e}");
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
        if now_secs() - last_update >= 1 {
            last_update = now_secs();
            let dl = shared.received.load(std::sync::atomic::Ordering::Relaxed).min(l.size as u64) as i64;
            app.hub.progress(&app, user_id, task_id, dl.min(l.size), l.size).await;
        }
        if now_secs() >= deadline {
            return fail(&app, user_id, task_id, &tmp, "下载超时,来源不足".into()).await;
        }
    }

    // ── 全量校验:大小 + root MD4(小文件直接算,大文件按块哈希拼接) ──
    let meta = tokio::fs::metadata(&tmp).await.ok();
    if meta.map(|m| m.len() as i64).unwrap_or(0) != l.size {
        return fail(&app, user_id, task_id, &tmp, "文件大小校验失败".into()).await;
    }
    let ps = shared.parts.lock().await;
    let part_md4s: Vec<[u8; 16]> = ps.iter().take(parts_total).filter_map(|p| p.md4).collect();
    drop(ps);
    if part_md4s.len() != parts_total {
        return fail(&app, user_id, task_id, &tmp, "部分块缺少校验值".into()).await;
    }
    let ok_root = if (l.size as u64) < peer::PART_SIZE {
        let data = match tokio::fs::read(&tmp).await {
            Ok(d) => d,
            Err(_) => return fail(&app, user_id, task_id, &tmp, "读取临时文件失败".into()).await,
        };
        md4(&data) == l.hash
    } else {
        let mut concat = Vec::with_capacity(parts_total * 16);
        for h in &part_md4s {
            concat.extend_from_slice(h);
        }
        md4(&concat) == l.hash
    };
    if !ok_root {
        return fail(&app, user_id, task_id, &tmp, "MD4 校验失败,文件损坏".into()).await;
    }

    // ── 移入网盘 ──
    let row = app.db.offline_task(task_id).await.unwrap_or(None).unwrap_or(json!({}));
    let share_id = row.get("shareId").and_then(|v| v.as_i64()).unwrap_or(0);
    let rel_dir = row.get("relDir").and_then(|v| v.as_str()).unwrap_or("下载").to_string();
    let host = app
        .db
        .share(share_id)
        .await
        .ok()
        .flatten()
        .map(|s| PathBuf::from(s.host_path))
        .unwrap_or_default();
    let target_dir = host.join(&rel_dir);
    let _ = tokio::fs::create_dir_all(&target_dir).await;
    let name = super::safe_name(&l.name, task_id);
    let final_path = super::unique_name(&target_dir, &name);
    if tokio::fs::rename(&tmp, &final_path).await.is_err() {
        return fail(&app, user_id, task_id, &tmp, "移入网盘失败".into()).await;
    }
    let rel = if rel_dir.is_empty() { name.clone() } else { format!("{}/{}", rel_dir, name) };
    let _ = app.db.set_meta_owner(share_id, &rel, user_id).await;
    app.spawn_replace(share_id, &rel, &final_path);
    app.hub.set_done(&app, user_id, task_id, json!({"size": l.size})).await;
    tracing::info!("[离线下载] eD2K 任务完成: {}", l.name);
}
