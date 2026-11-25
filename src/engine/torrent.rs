//! BT / 磁力引擎(librqbit):任务来源可为磁力链接、http(s) .torrent 直链或
//! 上传的本地种子文件。两阶段 —— 先 list_only 解析文件清单,
//! 多文件时等用户勾选(`/api/offline/select`),再按 `only_files` 真实下载;
//! 完成后把勾选的文件移入网盘,并触发自动转码钩子。
//!
//! 默认纯下载(会话层 disable-upload),`offline_bt_seed=1` 才允许做种;
//! BT 会话在首次使用时创建,该设置在会话重建后生效(通常重启服务)。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::app::{now_secs, App};
use crate::domain::human_bytes;

/// 从任务 files 字段(数组或 JSON 字符串)提取勾选的文件下标。
pub(crate) fn extract_selected(files_val: &serde_json::Value) -> Vec<usize> {
    let v = match files_val {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s).unwrap_or(json!([])),
        v => v.clone(),
    };
    v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            f.get("selected")
                .and_then(|b| b.as_bool())
                .filter(|b| *b)
                .map(|_| f.get("idx").and_then(|i| i.as_u64()).unwrap_or(0) as usize)
        })
        .collect()
}

pub(crate) fn extract_files_meta(files_val: &serde_json::Value) -> Vec<serde_json::Value> {
    let v = match files_val {
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s).unwrap_or(json!([])),
        v => v.clone(),
    };
    v.as_array().cloned().unwrap_or_default()
}

fn bt_files_json(info: &librqbit::ValidatedTorrentMetaV1Info<librqbit::ByteBufOwned>) -> (Option<String>, Vec<serde_json::Value>) {
    let name = info.name().map(|n| n.to_string());
    let files: Vec<serde_json::Value> = info
        .iter_file_details()
        .enumerate()
        .map(|(idx, d)| {
            json!({
                "idx": idx,
                "path": d.filename.to_string(),
                "size": d.len,
            })
        })
        .collect();
    (name, files)
}

async fn task_state(app: &App, task_id: i64) -> String {
    app.db
        .offline_task(task_id)
        .await
        .ok()
        .flatten()
        .and_then(|t| t.get("state").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

/// BT 会话:进程内只建一次(挂在 static,因为 librqbit 会话本身不可重建热替换)。
/// 这是引擎内部的资源句柄而非业务状态;取消 / 进度等仍走 Hub。
static BT_SESSION: tokio::sync::OnceCell<Arc<librqbit::Session>> = tokio::sync::OnceCell::const_new();

/// 获取(或初始化)BT 会话;初始化失败返回错误而非 panic——
/// 会话建不起来只影响离线下载,不该拖垮整个服务进程。
pub async fn session(app: &App) -> Result<&'static Arc<librqbit::Session>, String> {
    BT_SESSION
        .get_or_try_init(|| create_bt_session(app.clone()))
        .await
}

async fn create_bt_session(app: App) -> Result<Arc<librqbit::Session>, String> {
    if let Ok(limit) = librqbit::try_increase_nofile_limit() {
        tracing::info!("[离线下载] RLIMIT_NOFILE 提升至 {limit}");
    }
    let allow_seed = app.db.setting("offline_bt_seed").await.as_deref() == Some("1");
    let bt_dir = app.tmp_dir.join("bt");
    let _ = std::fs::create_dir_all(&bt_dir);
    let dht_bootstrap: Vec<String> = [
        "router.bittorrent.com:6881",
        "router.utorrent.com:6881",
        "dht.transmissionbt.com:6881",
        "dht.libtorrent.org:25401",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let build_opts = |port: u16| librqbit::SessionOptions {
        dht: Some(librqbit::DhtSessionConfig {
            bootstrap_addrs: Some(dht_bootstrap.clone()),
            persistence: Some(librqbit::dht::DhtPersistenceConfig {
                config_filename: Some(bt_dir.join("dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        }),
        listen: Some(librqbit::ListenerOptions {
            mode: librqbit::ListenerMode::TcpAndUtp,
            listen_addr: std::net::SocketAddr::from(([0, 0, 0, 0], port)),
            utp_opts: Some(librqbit::utp::SocketOpts {
                congestion: librqbit::utp::CongestionConfig {
                    kind: librqbit::utp::CongestionControllerKind::Ledbat,
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }),
        connect: Some(librqbit::ConnectionOptions {
            enable_tcp: true,
            peer_opts: Some(librqbit::PeerConnectionOptions {
                connect_timeout: Some(Duration::from_secs(10)),
                read_write_timeout: Some(Duration::from_secs(20)),
                handshake_timeout: Some(Duration::from_secs(10)),
                connect_rate: Some(30),
                ..Default::default()
            }),
            ..Default::default()
        }),
        peer_backoff: Some(librqbit::PeerBackoffConfig {
            min_delay: Duration::from_secs(5),
            factor: 4.0,
            max_delay: Duration::from_secs(600),
            total_delay: Some(Duration::from_secs(1800)),
        }),
        peer_limit: Some(512),
        global_peer_limit: Some(1024),
        mse_mode: librqbit::MseMode::Enabled,
        tracker_announce: librqbit::TrackerAnnounceOptions {
            announce_to_all_tiers: true,
            announce_to_all_trackers: true,
        },
        concurrent_init_limit: Some(3),
        disable_upload: !allow_seed,
        ..Default::default()
    };
    let mut last_err = None;
    // 常用 BT 端口优先,失败则 IPv4-only 再试一轮;最后回退随机端口。
    for port in 4200..=4210 {
        for v4_only in [false, true] {
            let opts = build_opts(port);
            match librqbit::Session::new_with_opts(bt_dir.clone(), librqbit::SessionOptions { ipv4_only: v4_only, ..opts }).await {
                Ok(s) => return Ok(s),
                Err(e) => last_err = Some(e),
            }
        }
    }
    librqbit::Session::new_with_opts(bt_dir.clone(), build_opts(0))
        .await
        .map_err(|e| {
            tracing::warn!(
                "[离线下载] BT 会话初始化失败(固定端口回退也不可用): {} / {e}",
                last_err.map(|e| e.to_string()).unwrap_or_default()
            );
            format!("BT 会话初始化失败: {e}")
        })
}

pub async fn run_torrent_task(app: App, task_id: i64, source: String, user_id: i64) {
    let _ = app.hub.register(task_id).await;
    // BT 会话初始化失败只判失败本任务,不影响服务与其它任务。
    let bt = match session(&app).await {
        Ok(s) => s,
        Err(e) => return fail(&app, user_id, task_id, e).await,
    };

    // 来源三形态:磁力链接(注入公共 tracker)/ http(s) .torrent 直链(引擎原生拉取)/
    // 上传的本地种子文件(读为内存字节后立即删除,后续阶段只用字节)。
    let is_magnet = source.to_ascii_lowercase().starts_with("magnet:");
    let (src_url, file_bytes) = if is_magnet {
        (Some(super::trackers::magnet_with_trackers(&app, &source).await), None)
    } else if source.starts_with("http://") || source.starts_with("https://") {
        (Some(source.clone()), None)
    } else {
        match tokio::fs::read(&source).await {
            Ok(b) => {
                let _ = tokio::fs::remove_file(&source).await;
                (None, Some(bytes::Bytes::from(b)))
            }
            Err(e) => return fail(&app, user_id, task_id, format!("读取上传的种子文件失败: {e}")).await,
        }
    };
    let build_add = || match (&src_url, &file_bytes) {
        (Some(u), _) => librqbit::AddTorrent::Url(u.clone().into()),
        (_, Some(b)) => librqbit::AddTorrent::TorrentFileBytes(b.clone()),
        _ => unreachable!("种子来源必为链接或字节"),
    };

    let out_dir = app.tmp_dir.join(format!("bt-{task_id}"));
    let tpath = app.tmp_dir.join(format!("bt-{task_id}.torrent"));

    // ── 第一阶段:只解析元数据,不下载 ──
    let opts = librqbit::AddTorrentOptions {
        list_only: true,
        paused: true,
        output_folder: Some(out_dir.display().to_string()),
        overwrite: true,
        ..Default::default()
    };
    let resp = match tokio::time::timeout(Duration::from_secs(120), bt.add_torrent(build_add(), Some(opts))).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return fail(&app, user_id, task_id, format!("种子解析失败: {e}")).await,
        Err(_) => {
            return fail(&app, user_id, task_id, "获取种子信息超时(做种节点太少或链接无效)".into()).await
        }
    };
    if app.hub.is_cancelled(task_id).await || task_state(&app, task_id).await == "cancelled" {
        let _ = tokio::fs::remove_dir_all(&out_dir).await;
        return;
    }
    let (torrent_name, files, have_bytes, seen_peers) = match &resp {
        librqbit::AddTorrentResponse::ListOnly(lr) => {
            let have = !lr.torrent_bytes.is_empty() && tokio::fs::write(&tpath, &lr.torrent_bytes).await.is_ok();
            let (n, f) = bt_files_json(&lr.info);
            (n, f, have, lr.seen_peers.clone())
        }
        librqbit::AddTorrentResponse::Added(_, h) | librqbit::AddTorrentResponse::AlreadyManaged(_, h) => {
            match h.with_metadata(|md| {
                let files = md
                    .file_infos
                    .iter()
                    .enumerate()
                    .map(|(idx, f)| {
                        json!({
                            "idx": idx,
                            "path": f.relative_filename.to_string_lossy(),
                            "size": f.len,
                        })
                    })
                    .collect();
                (md.info.name().map(|n| n.to_string()), files, false, Vec::new())
            }) {
                Ok(v) => v,
                Err(_) => return fail(&app, user_id, task_id, "种子元数据读取失败".into()).await,
            }
        }
    };
    let torrent_name = torrent_name.unwrap_or_else(|| format!("bt-{task_id}"));
    let total: i64 = files.iter().filter_map(|f| f.get("size").and_then(|v| v.as_i64())).sum();
    // 单文件种子无需勾选,自动开始。
    let auto_start = files.len() == 1;
    let files_payload = if auto_start {
        let mut marked = files.clone();
        if let Some(obj) = marked.first_mut().and_then(|v| v.as_object_mut()) {
            obj.insert("selected".into(), json!(true));
        }
        marked
    } else {
        files.clone()
    };
    app.hub
        .update(
            &app,
            user_id,
            task_id,
            json!({
                "state": if auto_start { "downloading" } else { "awaiting_select" },
                "name": torrent_name.clone(),
                "size": total,
                "files": files_payload,
            }),
        )
        .await;
    if auto_start {
        tracing::info!("[离线下载] 磁力仅含单个文件,自动开始: {torrent_name}");
    }

    // ── 第二阶段:等用户勾选(或单文件自动) ──
    let deadline = now_secs() + 24 * 3600;
    let selected: Vec<usize> = loop {
        let state = task_state(&app, task_id).await;
        if state == "cancelled" {
            let _ = tokio::fs::remove_dir_all(&out_dir).await;
            let _ = tokio::fs::remove_file(&tpath).await;
            return;
        }
        if app.db.offline_task(task_id).await.unwrap_or(None).is_none() {
            return;
        }
        if now_secs() >= deadline {
            let _ = tokio::fs::remove_dir_all(&out_dir).await;
            return fail(&app, user_id, task_id, "等待选择超时,任务已取消".into()).await;
        }
        if state != "downloading" {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let row = app.db.offline_task(task_id).await.unwrap_or(None).unwrap_or(json!({}));
        let selected = extract_selected(row.get("files").unwrap_or(&json!([])));
        if !selected.is_empty() {
            break selected;
        }
        tracing::warn!("[离线下载] 任务 {task_id} 选择结果丢失,退回等待选择");
        app.hub.update(&app, user_id, task_id, json!({"state": "awaiting_select"})).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    let limit = super::size_limit(&app).await;
    let sel: HashSet<usize> = selected.iter().copied().collect();
    let selected_total: i64 = files
        .iter()
        .filter(|f| sel.contains(&(f.get("idx").and_then(|v| v.as_u64()).unwrap_or(u64::MAX) as usize)))
        .filter_map(|f| f.get("size").and_then(|v| v.as_i64()))
        .sum();
    if selected_total > limit {
        return fail(&app, user_id, task_id, format!("选中内容超过单任务上限({})", human_bytes(limit))).await;
    }

    // ── 真实下载 ──
    let opts = librqbit::AddTorrentOptions {
        paused: false,
        overwrite: true,
        output_folder: Some(out_dir.display().to_string()),
        only_files: Some(selected.clone()),
        initial_peers: Some(seen_peers.clone()),
        ..Default::default()
    };
    let add = if have_bytes {
        match tokio::fs::read(&tpath).await {
            Ok(b) => librqbit::AddTorrent::TorrentFileBytes(bytes::Bytes::from(b)),
            Err(_) => build_add(),
        }
    } else {
        build_add()
    };
    let handle = match tokio::time::timeout(Duration::from_secs(120), bt.add_torrent(add, Some(opts))).await {
        Ok(Ok(librqbit::AddTorrentResponse::Added(_, h))) => h,
        Ok(Ok(librqbit::AddTorrentResponse::AlreadyManaged(_, h))) => {
            if let Err(e) = bt.update_only_files(&h, &sel).await {
                return fail(&app, user_id, task_id, format!("应用文件选择失败: {e}")).await;
            }
            // 上一个持有同一 info-hash 的任务在取消轮询间隔内尚未退出会话时,
            // 它仍是 live 态:只有处于暂停才需要 unpause,否则视为已在运行。
            if h.is_paused() {
                if let Err(e) = bt.unpause(&h).await {
                    return fail(&app, user_id, task_id, format!("启动下载失败: {e}")).await;
                }
            }
            h
        }
        Ok(Ok(_)) => return fail(&app, user_id, task_id, "任务启动异常".into()).await,
        Ok(Err(e)) => return fail(&app, user_id, task_id, format!("启动下载失败: {e}")).await,
        Err(_) => return fail(&app, user_id, task_id, "启动下载超时".into()).await,
    };

    loop {
        if app.hub.is_cancelled(task_id).await || task_state(&app, task_id).await == "cancelled" {
            let _ = bt.delete(handle.id().into(), true).await;
            let _ = tokio::fs::remove_dir_all(&out_dir).await;
            let _ = tokio::fs::remove_file(&tpath).await;
            return;
        }
        let stats = handle.stats();
        if stats.finished {
            break;
        }
        let downloaded: i64 = sel.iter().filter_map(|i| stats.file_progress.get(*i)).sum::<u64>() as i64;
        app.hub.progress(&app, user_id, task_id, downloaded, selected_total).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // ── 移入网盘目标目录 ──
    let row = app.db.offline_task(task_id).await.unwrap_or(None).unwrap_or(json!({}));
    let share_id = row.get("shareId").and_then(|v| v.as_i64()).unwrap_or(0);
    let rel_dir = row.get("relDir").and_then(|v| v.as_str()).unwrap_or("下载").to_string();
    let files_meta = extract_files_meta(row.get("files").unwrap_or(&json!([])));
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
    let bt_root = out_dir.clone();
    let mut moved = 0usize;
    for f in &files_meta {
        let idx = f.get("idx").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(usize::MAX);
        if !sel.contains(&idx) {
            continue;
        }
        let Some(rel_file) = f.get("path").and_then(|v| v.as_str()) else { continue };
        // 种子内相对路径逐段消毒:任一段非法(含 .. / 分隔符变体 / 保留名)即跳过该文件,
        // 文件留在临时目录由收尾的 remove_dir_all 回收,绝不写出下载树。
        let Some(rel_file) = super::safe_rel_path(rel_file) else { continue };
        let src = bt_root.join(&rel_file);
        let dst = target_dir.join(&rel_file);
        if let Some(parent) = dst.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let moved_ok = match tokio::fs::rename(&src, &dst).await {
            Ok(()) => true,
            Err(_) => match tokio::fs::copy(&src, &dst).await {
                Ok(_) => {
                    let _ = tokio::fs::remove_file(&src).await;
                    true
                }
                Err(_) => false,
            },
        };
        if moved_ok {
            let rel = if rel_dir.is_empty() { rel_file.to_string() } else { format!("{rel_dir}/{rel_file}") };
            let _ = app.db.set_meta_owner(share_id, &rel, user_id).await;
            app.spawn_replace(share_id, &rel, &dst);
            moved += 1;
        }
    }
    let _ = bt.delete(handle.id().into(), false).await;
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
    let _ = tokio::fs::remove_file(&tpath).await;
    app.hub
        .set_done(&app, user_id, task_id, json!({"name": torrent_name.clone(), "size": selected_total}))
        .await;
    tracing::info!("[离线下载] BT 任务完成: {torrent_name} 移入 {moved} 个文件");
}

async fn fail(app: &App, user_id: i64, task_id: i64, msg: String) {
    tracing::info!("[离线下载] BT 任务失败: {msg}");
    super::set_state(app, user_id, task_id, "failed", Some(msg)).await;
}

/// 上传种子的内容级校验:必须是可解析的 BT 元数据。返回展示名(优先
/// `name.utf-8`)与等价磁力链接(info-hash + dn + 种子自带 tracker),
/// 存进任务 url 列,使"重新下载"不依赖已删除的临时文件。
/// 真正的文件清单解析仍由任务第一阶段(list_only)完成。
pub fn parse_torrent_upload(data: &[u8]) -> Result<(String, String), String> {
    let meta = librqbit::torrent_from_bytes(data).map_err(|e| format!("不是有效的种子文件: {e}"))?;
    let name = meta
        .info
        .data
        .name_utf8
        .as_ref()
        .or(meta.info.data.name.as_ref())
        .and_then(|b| std::str::from_utf8(b.as_ref()).ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("种子任务")
        .to_string();
    let mut magnet = format!(
        "magnet:?xt=urn:btih:{}",
        meta.info_hash.0.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    magnet.push_str("&dn=");
    magnet.push_str(&url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>());
    for tier in meta.announce_tiers() {
        for tracker in tier {
            if let Ok(s) = std::str::from_utf8(tracker.as_ref()) {
                let s = s.trim();
                if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("udp://") || s.starts_with("ws://")
                {
                    magnet.push_str("&tr=");
                    magnet.push_str(&url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>());
                }
            }
        }
    }
    Ok((name, magnet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_torrent_upload_accepts_and_rejects() {
        // 最小合法单文件种子(name + length + piece length + pieces)。
        let mut t = Vec::new();
        t.extend_from_slice(b"d8:announce21:http://tr.example/ann4:infod6:lengthi5e4:name4:test12:piece lengthi16384e6:pieces20:");
        t.extend_from_slice(&[0u8; 20]);
        t.extend_from_slice(b"ee");
        let (name, magnet) = parse_torrent_upload(&t).unwrap();
        assert_eq!(name, "test");
        assert!(magnet.starts_with("magnet:?xt=urn:btih:"));
        assert!(magnet.contains("&dn=test"));
        assert!(magnet.contains("tr=http%3A%2F%2Ftr.example%2Fann"));
        assert!(parse_torrent_upload(b"not a torrent").is_err());
        assert!(parse_torrent_upload(b"d3:infode").is_err()); // 缺 info 字段
    }

    #[test]
    fn extract_selected_both_formats() {
        let files = json!([
            {"idx":0,"path":"a.mkv","size":1,"selected":true},
            {"idx":1,"path":"b.mkv","size":2}
        ]);
        assert_eq!(extract_selected(&files), vec![0]);
        let double = json!(serde_json::to_string(&files).unwrap());
        assert_eq!(extract_selected(&double), vec![0]);
        assert_eq!(extract_selected(&json!([])), Vec::<usize>::new());
    }

    #[test]
    fn extract_files_meta_both_formats() {
        let files = json!([{"idx":0,"path":"a","size":1}]);
        assert_eq!(extract_files_meta(&files).len(), 1);
        let double = json!(serde_json::to_string(&files).unwrap());
        assert_eq!(extract_files_meta(&double).len(), 1);
    }
}
