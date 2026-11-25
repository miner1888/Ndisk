//! 应用组装根:`App` 是全项目**唯一**的应用级状态持有者。
//! 数据库、数据目录、任务中心、TLS 运行时、验证码表、缩略图守卫
//! 全部挂在这里;不存在其它全局可变单例。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::cli::Config;
use crate::db::Db;
use crate::domain::auth;
use crate::domain::media::thumbs::ThumbGuard;
use crate::engine::hub::Hub;

pub const CAPTCHA_TTL: i64 = 600;
pub const UPLOAD_TTL: i64 = 48 * 3600;
pub const DB_FILE: &str = "app.sqlite3";
/// 个人空间在共享列表里的显示名。
pub const HOME_NAME: &str = "个人空间";

pub use crate::domain::now_secs;

#[derive(Clone)]
pub struct App {
    pub db: Db,
    pub data_dir: PathBuf,
    pub uploads_dir: PathBuf,
    pub users_dir: PathBuf,
    pub media_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub chunk_size: i64,
    pub tls_listen: String,
    pub tls: Arc<TlsRuntime>,
    pub dl_key: crate::domain::ticket::Key,
    pub hub: Hub,
    captchas: Arc<Mutex<HashMap<String, (String, i64)>>>,
    pub thumbs: Arc<ThumbGuard>,
    pub transcode_lock: Arc<Mutex<()>>,
}

/// HTTPS 运行时状态:证书变化时热重建监听,不影响 HTTP 服务。
#[derive(Default)]
pub struct TlsRuntime {
    pub config: tokio::sync::RwLock<Option<Arc<rustls::ServerConfig>>>,
    /// 当前 HTTPS 监听端口(未启用为 None)。
    pub port: Mutex<Option<u16>>,
    /// 证书变化通知(accept loop 每次收到通知后重新读库)。
    pub changed: tokio::sync::Notify,
}

fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NDISK_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in [&exe, "ffmpeg"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

impl App {
    /// 启动:建目录 → 开库 → 探测 ffmpeg → 清理陈旧数据 → 引导管理员 → 起后台任务。
    pub async fn boot(cfg: &Config) -> Result<Self, String> {
        let data_dir = cfg.data_dir.clone();
        let uploads_dir = data_dir.join("uploads");
        let tmp_dir = data_dir.join("tmp");
        let users_dir = data_dir.join("users");
        let media_dir = data_dir.join("media");
        for d in [&data_dir, &uploads_dir, &users_dir, &media_dir, &tmp_dir] {
            std::fs::create_dir_all(d).map_err(|e| format!("创建数据目录失败: {e}"))?;
        }
        let db = Db::open(&data_dir.join(DB_FILE)).await?;
        let ffmpeg = find_ffmpeg();
        match &ffmpeg {
            Some(p) => tracing::info!("检测到 ffmpeg: {}", p.display()),
            None => tracing::warn!("未找到 ffmpeg:视频转码与缩略图不可用(可设置环境变量 NDISK_FFMPEG)"),
        }
        let mut dl_key = [0u8; 32];
        rand::fill(&mut dl_key[..]);
        let app = Self {
            db,
            data_dir,
            uploads_dir,
            tmp_dir,
            users_dir,
            media_dir,
            ffmpeg,
            chunk_size: if cfg.chunk_size > 0 { cfg.chunk_size } else { crate::cli::DEFAULT_CHUNK },
            tls_listen: cfg.tls_listen.clone(),
            tls: Arc::new(TlsRuntime::default()),
            dl_key,
            hub: Hub::new(),
            captchas: Arc::new(Mutex::new(HashMap::new())),
            thumbs: ThumbGuard::new(),
            transcode_lock: Arc::new(Mutex::new(())),
        };
        app.cleanup_uploads().await;
        // 崩溃 / 强杀后遗留的 running 任务置为 failed(worker 已随进程消亡)。
        let _ = app.db.fail_running_offline_tasks().await;
        crate::domain::media::sweep_cache(&app.db, &app.media_dir).await;
        app.bootstrap_admin(&cfg.admin_user, &cfg.admin_pass).await;
        let jan = app.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                // 单轮 panic 不能终止清理循环(否则缓存 / 临时文件会无限堆积)。
                let one = jan.clone();
                if std::panic::AssertUnwindSafe(async move { one.janitor_round().await })
                    .catch_unwind()
                    .await
                    .is_err()
                {
                    tracing::error!("后台维护任务异常(panic),本轮跳过,下一轮继续");
                }
            }
        });
        Ok(app)
    }

    /// 后台维护的一轮清理(启动后每小时执行;单点失败不影响服务与下一轮)。
    async fn janitor_round(&self) {
        self.cleanup_uploads().await;
        sweep_share_temps(self).await;
        sweep_tmp_files(self).await;
        let _ = self.db.prune_old_offline_tasks(now_secs() - 7 * 86400).await;
        crate::domain::media::sweep_cache(&self.db, &self.media_dir).await;
        let now = now_secs();
        self.captchas.lock().await.retain(|_, (_, t)| now - *t < CAPTCHA_TTL);
        crate::domain::media::thumbs::sweep_failures(&self.thumbs).await;
    }

    /// 清理过期上传会话与磁盘上的无主分片目录。
    pub async fn cleanup_uploads(&self) {
        let _ = self.db.prune_uploads(now_secs() - UPLOAD_TTL).await;
        let live: HashSet<String> = match self.db.live_upload_tokens().await {
            Ok(t) => t.into_iter().collect(),
            Err(_) => return,
        };
        if let Ok(rd) = std::fs::read_dir(&self.uploads_dir) {
            for de in rd.flatten() {
                let name = de.file_name().to_string_lossy().to_string();
                let keep = live.contains(&name)
                    || (name.ends_with(".merge") && live.contains(name.trim_end_matches(".merge")));
                if !keep {
                    let p = de.path();
                    if p.is_dir() {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
    }

    async fn bootstrap_admin(&self, user: &str, pass: &str) {
        let Ok(users) = self.db.all_users().await else { return };
        if !users.is_empty() {
            return;
        }
        match auth::hash_password(pass) {
            Ok(hash) => {
                let id = match self.db.create_user(user, &hash, true, [true, true, true, true]).await {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!("创建管理员失败: {e}");
                        return;
                    }
                };
                let dir = self.home_dir(user);
                let _ = std::fs::create_dir_all(&dir);
                let _ = self
                    .db
                    .create_share(HOME_NAME, &crate::domain::pathguard::pretty(&dir), Some(id), "home")
                    .await;
                tracing::warn!("已创建初始管理员: {user} / {pass}(请尽快修改密码)");
            }
            Err(e) => tracing::error!("创建管理员失败: {e}"),
        }
    }

    /// 某用户的个人空间目录:`data/users/<用户名>`。
    pub fn home_dir(&self, username: &str) -> PathBuf {
        self.users_dir.join(username)
    }

    /// 签发 4 位验证码(一次性消费,TTL 内有效)。
    pub async fn issue_captcha(&self) -> (String, String) {
        use rand::RngExt;
        let code = {
            let mut rng = rand::rng();
            let alpha: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
            (0..4)
                .map(|_| alpha[rng.random_range(0..alpha.len())] as char)
                .collect::<String>()
        };
        let id = auth::new_token();
        {
            let mut m = self.captchas.lock().await;
            m.retain(|_, (_, t)| now_secs() - *t < CAPTCHA_TTL);
            m.insert(id.clone(), (code.clone(), now_secs()));
        }
        (id, code)
    }

    /// 校验并消费验证码(大小写不敏感)。
    pub async fn check_captcha(&self, id: &str, answer: &str) -> bool {
        match self.captchas.lock().await.remove(id) {
            Some((want, t)) => now_secs() - t < CAPTCHA_TTL && want.eq_ignore_ascii_case(answer.trim()),
            None => false,
        }
    }

    /// 后台触发"上传后自动转码替换"(网页上传 / WebDAV / 离线下载完成后的统一钩子)。
    pub fn spawn_replace(&self, share_id: i64, rel: &str, target: &std::path::Path) {
        use futures_util::FutureExt;
        let db = self.db.clone();
        let media_dir = self.media_dir.clone();
        let ffmpeg = self.ffmpeg.clone();
        let lock = self.transcode_lock.clone();
        let rel = rel.to_string();
        let rel_log = rel.clone();
        let target = target.to_path_buf();
        tokio::spawn(async move {
            let task = crate::domain::media::maybe_replace_transcode(
                &db, &media_dir, ffmpeg.as_deref(), &lock, share_id, &rel, &target,
            );
            if std::panic::AssertUnwindSafe(task).catch_unwind().await.is_err() {
                tracing::error!("自动转码替换异常(panic): share={share_id} rel={rel_log}(原文件保留)");
            }
        });
    }
}

/// 清理共享根目录下残留的落盘临时文件。命名约定(各写入方统一遵守):
/// `.nd-merge-*.tmp`(分片合并)、`.nd-dav-*.tmp`(WebDAV PUT)、
/// `.nd-direct-*.tmp`(小文件直传)、`.nd-save-*.tmp`(原子写)。
async fn sweep_share_temps(app: &App) {
    const PREFIXES: [&str; 4] = [".nd-merge-", ".nd-dav-", ".nd-direct-", ".nd-save-"];
    let shares = app.db.all_shares().await.unwrap_or_default();
    if shares.is_empty() {
        return;
    }
    // 临时文件可能落在任意子目录(原子写与目标同目录),整树递归;阻塞遍历放阻塞线程。
    // 注意不能用 fsx::walk —— 它跳过隐藏项,而 .nd- 前缀正是隐藏名。
    let jobs: Vec<std::path::PathBuf> = shares.iter().map(|s| std::path::PathBuf::from(&s.host_path)).collect();
    tokio::task::spawn_blocking(move || {
        for root in jobs {
            sweep_temps_in(&root, &PREFIXES, 0);
        }
    })
    .await
    .ok();
}

fn sweep_temps_in(dir: &std::path::Path, prefixes: &[&str], depth: u32) {
    if depth > 16 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for de in rd.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(de.path()) else { continue };
        if meta.is_symlink() {
            continue;
        }
        let name = de.file_name().to_string_lossy().to_string();
        if meta.is_dir() {
            sweep_temps_in(&de.path(), prefixes, depth + 1);
        } else if name.ends_with(".tmp") && prefixes.iter().any(|p| name.starts_with(p)) {
            let _ = std::fs::remove_file(de.path());
        }
    }
}

/// 清理 `tmp/` 下超过 24 小时未改动的临时文件(离线下载分片、打包 ZIP 等)。
/// 只清"陈旧"文件,活跃任务正在写的临时文件不能删;顺手清理过期分享链接。
async fn sweep_tmp_files(app: &App) {
    let cutoff = now_secs() - 24 * 3600;
    if let Ok(rd) = std::fs::read_dir(&app.tmp_dir) {
        for de in rd.flatten() {
            let Ok(meta) = de.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if secs >= cutoff {
                continue;
            }
            if meta.is_dir() {
                // 崩溃遗留的 BT 下载目录(bt-<任务id>,正常路径由任务收尾删除)。
                if de.file_name().to_string_lossy().starts_with("bt-") {
                    let _ = std::fs::remove_dir_all(de.path());
                }
                continue;
            }
            let _ = std::fs::remove_file(de.path());
        }
    }
    let _ = app.db.prune_expired_links(now_secs()).await;
}
