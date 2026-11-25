//! 媒体子系统:类型判定、ffmpeg 封装、缩略图、转码、缓存登记与过期清理。
//!
//! 缓存目录布局(兼容性不变量):
//! `data/media/<sha256("<share_id>|<rel>") 前 8 字节 hex>/{f.mp4,thumb.jpg,img.jpg}`

pub mod ffmpeg;
pub mod kinds;
pub mod thumbs;
pub mod transcode;

use sha2::{Digest, Sha256};

use crate::db::Db;
use crate::domain::now_secs;

pub use kinds::*;
pub use thumbs::{ensure_image_thumb, ensure_video_thumb, THUMB_FAIL_TTL};
pub use transcode::{ensure_transcode, maybe_replace_transcode};

/// 缓存键:`sha256("<share_id>|<rel>")` 的前 8 字节 hex。
/// 源文件改名 / 移动后键随之改变,旧缓存由 sweep 回收。
pub fn cache_key(share_id: i64, rel: &str) -> String {
    let mut h = Sha256::new();
    h.update(share_id.to_string().as_bytes());
    h.update(b"|");
    h.update(rel.as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub fn transcode_out(media_dir: &std::path::Path, share_id: i64, rel: &str) -> std::path::PathBuf {
    media_dir.join(cache_key(share_id, rel)).join("f.mp4")
}

pub fn video_thumb(media_dir: &std::path::Path, share_id: i64, rel: &str) -> std::path::PathBuf {
    media_dir.join(cache_key(share_id, rel)).join("thumb.jpg")
}

pub fn image_thumb(media_dir: &std::path::Path, share_id: i64, rel: &str) -> std::path::PathBuf {
    media_dir.join(cache_key(share_id, rel)).join("img.jpg")
}

/// 源文件被删除 / 重命名 / 覆盖时回收其全部派生缓存(登记 + 磁盘)。
pub async fn purge(db: &Db, media_dir: &std::path::Path, share_id: i64, rel: &str) {
    if let Ok(Some(out)) = db.transcode_take(share_id, rel).await {
        let _ = std::fs::remove_file(out);
    }
    if let Ok(paths) = db.media_cache_drop(share_id, rel).await {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }
    let _ = std::fs::remove_file(video_thumb(media_dir, share_id, rel));
    let _ = std::fs::remove_file(image_thumb(media_dir, share_id, rel));
    prune_empty_dirs(media_dir, share_id, rel);
}

fn prune_empty_dirs(media_dir: &std::path::Path, share_id: i64, rel: &str) {
    let dir = media_dir.join(cache_key(share_id, rel));
    if let Ok(rd) = std::fs::read_dir(&dir) {
        if rd.flatten().next().is_none() {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

fn prune_parent(path: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

const CACHE_MAX_AGE_DAYS: i64 = 30;
const ORPHAN_GRACE_SECS: i64 = 3600;

/// 管理后台展示的缓存统计。
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CacheStats {
    pub files: u64,
    pub bytes: u64,
    pub thumbs: u64,
    pub thumb_bytes: u64,
    pub transcodes: u64,
    pub transcode_bytes: u64,
    pub dirs: u64,
}

impl CacheStats {
    fn add(&mut self, path: &str, is_transcode: bool) {
        if let Ok(m) = std::fs::metadata(path) {
            if m.is_file() {
                self.files += 1;
                self.bytes += m.len();
                if is_transcode {
                    self.transcodes += 1;
                    self.transcode_bytes += m.len();
                } else {
                    self.thumbs += 1;
                    self.thumb_bytes += m.len();
                }
            }
        }
    }
}

pub async fn stats(db: &Db, media_dir: &std::path::Path) -> CacheStats {
    let mut s = CacheStats::default();
    if let Ok(rd) = std::fs::read_dir(media_dir) {
        for de in rd.flatten() {
            if de.path().is_dir() {
                s.dirs += 1;
            }
        }
    }
    if let Ok(rows) = db.media_cache_list().await {
        for (_, _, _, out, _) in rows {
            s.add(&out, false);
        }
    }
    if let Ok(rows) = db.transcode_list().await {
        for (_, _, _, out, _) in rows {
            s.add(&out, true);
        }
    }
    s
}

/// 一键清理全部派生缓存,返回释放的字节数。
pub async fn clear_all(db: &Db, media_dir: &std::path::Path) -> u64 {
    let before = stats(db, media_dir).await.bytes;
    if let Ok(rows) = db.media_cache_list().await {
        for (_, _, _, out, _) in rows {
            let _ = std::fs::remove_file(out);
        }
    }
    if let Ok(rows) = db.transcode_list().await {
        for (_, _, _, out, _) in rows {
            let _ = std::fs::remove_file(out);
        }
    }
    let _ = db.media_cache_clear().await;
    let _ = db.transcode_clear().await;
    if let Ok(rd) = std::fs::read_dir(media_dir) {
        for de in rd.flatten() {
            let p = de.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            }
        }
    }
    let after = stats(db, media_dir).await.bytes;
    before.saturating_sub(after)
}

/// 启动时与每小时执行:清理源已删除 / 超期 / 失败 / 损坏的派生缓存与孤儿目录。
pub async fn sweep_cache(db: &Db, media_dir: &std::path::Path) {
    let now = now_secs();
    let max_age = CACHE_MAX_AGE_DAYS * 86400;
    let mut removed = 0usize;

    let shares = db.all_shares().await.unwrap_or_default();
    let source_exists = |share_id: i64, rel: &str| -> bool {
        shares
            .iter()
            .find(|s| s.id == share_id)
            .map(|sh| std::path::Path::new(&sh.host_path).join(rel).exists())
            .unwrap_or(false)
    };

    if let Ok(rows) = db.media_cache_list().await {
        for (share_id, rel, kind, out, updated) in rows {
            let expired = now - updated > max_age;
            if expired || !source_exists(share_id, &rel) {
                let _ = std::fs::remove_file(&out);
                prune_parent(&out);
                let _ = db.media_cache_delete(share_id, &rel, &kind).await;
                removed += 1;
            }
        }
    }

    if let Ok(rows) = db.transcode_list().await {
        for (share_id, rel, status, out, updated) in rows {
            let stale_failed = status == "failed" && now - updated > THUMB_FAIL_TTL;
            let expired = now - updated > max_age;
            let broken_done = status == "done" && !std::path::Path::new(&out).exists();
            if !source_exists(share_id, &rel) || stale_failed || expired || broken_done {
                let _ = std::fs::remove_file(&out);
                prune_parent(&out);
                let _ = db.transcode_delete(share_id, &rel).await;
                removed += 1;
            }
        }
    }

    // 孤儿目录:登记表里已经没有任何条目引用、且 1 小时未被触碰。
    let known: std::collections::HashSet<String> = {
        let mut set = std::collections::HashSet::new();
        if let Ok(rows) = db.media_cache_list().await {
            for (_, _, _, out, _) in rows {
                if let Some(p) = std::path::Path::new(&out).parent() {
                    set.insert(p.to_string_lossy().to_string());
                }
            }
        }
        if let Ok(rows) = db.transcode_list().await {
            for (_, _, _, out, _) in rows {
                if let Some(p) = std::path::Path::new(&out).parent() {
                    set.insert(p.to_string_lossy().to_string());
                }
            }
        }
        set
    };
    if let Ok(rd) = std::fs::read_dir(media_dir) {
        for de in rd.flatten() {
            let p = de.path();
            if !p.is_dir() {
                continue;
            }
            let key = p.to_string_lossy().to_string();
            if known.contains(&key) {
                continue;
            }
            let old = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .map(|t| now_secs() - crate::domain::fsx::unix_secs(Ok(t)) > ORPHAN_GRACE_SECS)
                .unwrap_or(false);
            if old {
                let _ = std::fs::remove_dir_all(&p);
                removed += 1;
            }
        }
    }

    if removed > 0 {
        tracing::info!("媒体缓存清理完成:回收 {removed} 项(源文件已删除 / 超期 / 孤儿)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_8bytes() {
        let a = cache_key(3, "a/b.png");
        assert_eq!(a.len(), 16);
        assert_eq!(a, cache_key(3, "a/b.png"));
        assert_ne!(a, cache_key(4, "a/b.png"));
        assert_ne!(a, cache_key(3, "a/b.txt"));
    }

    /// 契约测试:sweep_cache 只处理派生缓存,用户文件(个人空间 / 共享目录)
    /// 无论如何都不会被缓存清理触碰。
    #[tokio::test]
    async fn sweep_never_touches_user_files() {
        let base = std::env::temp_dir().join(format!("ndv2_sweep_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let db = Db::open(&base.join("t.sqlite3")).await.unwrap();
        let media = base.join("media");
        let users = base.join("users/u1");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::create_dir_all(&users).unwrap();

        // 用户的视频:31 天前创建,但它是用户文件,绝不能被删。
        let user_file = users.join("movie.avi");
        std::fs::write(&user_file, b"real user data").unwrap();

        // 派生缓存登记:31 天前(超期)。
        let cache_dir = media.join(cache_key(1, "movie.avi"));
        std::fs::create_dir_all(&cache_dir).unwrap();
        let cached_out = cache_dir.join("f.mp4");
        std::fs::write(&cached_out, b"transcoded").unwrap();
        let old = now_secs() - 31 * 86400;
        db.media_cache_put(1, "movie.avi", "vthumb", &cached_out.to_string_lossy(), old).await.unwrap();

        sweep_cache(&db, &media).await;

        // 超期缓存已回收;用户文件原封不动。
        assert!(!cached_out.exists(), "超期缓存应被回收");
        assert!(user_file.exists(), "用户文件绝不能被缓存清理删除");
        assert_eq!(std::fs::read(&user_file).unwrap(), b"real user data");
        let _ = std::fs::remove_dir_all(&base);
    }
}
