//! BT tracker 聚合:内置公共 tracker + 每日订阅(24h 磁盘缓存)。
//! 磁盘缓存统一走 `domain::cached`。

use std::time::Duration;

use serde_json::json;

use crate::app::App;
use crate::domain::cached;

pub const TRACKER_SUB_KEY: &str = "bt_tracker_subscription";

const PUBLIC_TRACKERS: &[&str] = &[
    "udp://tracker.dler.com:6969/announce",
    "udp://admin.52ywp.com:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "https://tracker.moeblog.cn:443/announce",
    "http://nyaa.tracker.wf:7777/announce",
    "https://tr.zukizuki.org:443/announce",
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.dstud.io:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.tiny-vps.com:6969/announce",
    "udp://tracker.moeking.me:6969/announce",
    "udp://explodie.org:6969/announce",
    "udp://tracker1.bt.moack.co.kr:80/announce",
    "udp://tracker.theoks.net:6969/announce",
    "udp://p4p.arenabg.com:1337/announce",
    "https://tracker.gbitt.info:443/announce",
    "http://tracker.gbitt.info:80/announce",
];

const DEFAULT_SUB_URLS: &[&str] = &[
    "https://trackerslist.com/best.txt",
    "https://ngosang.github.io/trackerslist/trackers_best.txt",
];

const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

fn cache_path(app: &App) -> std::path::PathBuf {
    cached::cache_path(&app.data_dir, "bt-trackers.txt")
}

fn is_valid_tracker(s: &str) -> bool {
    s.starts_with("udp://") || s.starts_with("http://") || s.starts_with("https://") || s.starts_with("ws://")
}

async fn sub_urls(app: &App) -> Vec<String> {
    let raw = app.db.setting(TRACKER_SUB_KEY).await.unwrap_or_default();
    if raw.trim().is_empty() {
        DEFAULT_SUB_URLS.iter().map(|s| s.to_string()).collect()
    } else {
        raw.split([',', ';', '\n'])
            .map(str::trim)
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .map(|s| s.to_string())
            .collect()
    }
}

/// 内置 tracker + 订阅缓存合并去重,最多 60 条。
pub async fn merged_trackers(app: &App) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in PUBLIC_TRACKERS {
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    let cache = cache_path(app);
    let mut sub_texts: Vec<String> = Vec::new();
    if let Some(text) = cached::read_fresh_text(&cache, CACHE_TTL) {
        sub_texts.push(text);
    } else {
        // 各订阅地址并行拉取(单个失败返回空文本,不影响其余);仍按地址顺序合并。
        let texts = futures_util::future::join_all(sub_urls(app).await.into_iter().take(5).map(|url| async move {
            match tokio::time::timeout(Duration::from_secs(20), reqwest::get(url.as_str())).await {
                Ok(Ok(resp)) => resp.text().await.unwrap_or_default(),
                _ => String::new(),
            }
        }))
        .await;
        sub_texts.extend(texts.into_iter().filter(|t| !t.is_empty()));
        if !sub_texts.is_empty() {
            cached::write_cache(&cache, sub_texts.join("\n").as_bytes());
        }
    }
    for text in &sub_texts {
        for line in text.lines() {
            let t = line.trim();
            if is_valid_tracker(t) && seen.insert(t.to_string()) {
                out.push(t.to_string());
            }
        }
    }
    out.truncate(60);
    out
}

/// 管理后台展示字段(当前生效的订阅地址;自定义留空时为内置默认)。
pub async fn status_fields(app: &App) -> serde_json::Value {
    json!({
        "tracker_sub_url": app
            .db
            .setting(TRACKER_SUB_KEY)
            .await
            .unwrap_or_else(|| DEFAULT_SUB_URLS.join(",")),
    })
}

/// 把公共 tracker 注入 magnet 链接(已有的跳过)。
pub async fn magnet_with_trackers(app: &App, magnet: &str) -> String {
    if !magnet.to_ascii_lowercase().starts_with("magnet:?") {
        return magnet.to_string();
    }
    let trackers = merged_trackers(app).await;
    let mut out = magnet.to_owned();
    for t in trackers {
        let enc: String = url::form_urlencoded::byte_serialize(t.as_bytes()).collect();
        if out.contains(t.as_str()) || out.contains(enc.as_str()) {
            continue;
        }
        out.push_str(if out.contains('?') { "&tr=" } else { "?tr=" });
        out.push_str(&enc);
    }
    out
}
