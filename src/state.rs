use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::{self, Config};
use crate::db::Db;
use crate::domain::auth;

pub const CAPTCHA_TTL: i64 = 600;
const UPLOAD_TTL: i64 = 48 * 3600;
pub const DB_FILE: &str = "app.sqlite3";
/// 个人空间的显示名:不在界面上暴露账号名。
pub const HOME_NAME: &str = "个人空间";

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 运行期状态:数据库、数据目录、ffmpeg、分片大小、验证码表、媒体任务去重集合。
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub uploads_dir: PathBuf,
    pub users_dir: PathBuf,
    pub media_dir: PathBuf,
    /// 打包下载等临时文件目录(定期清理)
    pub tmp_dir: PathBuf,
    pub ffmpeg: Option<PathBuf>,
    pub chunk_size: i64,
    pub tls_listen: String,
    pub tls: Arc<TlsState>,
    pub dl_key: [u8; 32],
    captchas: Arc<Mutex<HashMap<String, (String, i64)>>>,
    pub thumb_pending: Arc<Mutex<HashSet<(i64, String)>>>,
    pub thumb_failed: Arc<Mutex<HashMap<(i64, String), i64>>>,
    /// 上传视频自动转码替换的串行执行锁
    pub transcode_lock: Arc<tokio::sync::Mutex<()>>,
}

/// TLS 运行态:当前生效配置 + 实际端口;changed 用于通知管理任务重载。
#[derive(Default)]
pub struct TlsState {
    pub config: tokio::sync::RwLock<Option<Arc<rustls::ServerConfig>>>,
    pub port: Mutex<Option<u16>>,
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

impl AppState {
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
            None => {
                tracing::warn!("未找到 ffmpeg:视频转码与缩略图不可用(可设置环境变量 NDISK_FFMPEG)")
            }
        }
        let mut dl_key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut dl_key);
        let st = AppState {
            db,
            uploads_dir,
            tmp_dir,
            users_dir,
            media_dir,
            ffmpeg,
            chunk_size: if cfg.chunk_size > 0 {
                cfg.chunk_size
            } else {
                config::DEFAULT_CHUNK
            },
            tls_listen: cfg.tls_listen.clone(),
            tls: Arc::new(TlsState::default()),
            dl_key,
            captchas: Arc::new(Mutex::new(HashMap::new())),
            thumb_pending: Arc::new(Mutex::new(HashSet::new())),
            thumb_failed: Arc::new(Mutex::new(HashMap::new())),
            transcode_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        st.cleanup_uploads().await;
        crate::domain::media::sweep_cache(&st).await;
        st.bootstrap_admin(&cfg.admin_user, &cfg.admin_pass).await;
        // 看护任务:周期清理过期上传 / 无主分片 / 过期验证码与缩略图失败记录,
        // 保证长期运行不累积(首次 tick 立即执行,与启动清理幂等)
        let jan = st.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                jan.cleanup_uploads().await;
                sweep_merge_temps(&jan).await;
                sweep_tmp_files(&jan).await;
                crate::domain::media::sweep_cache(&jan).await;
                let now = now_secs();
                jan.captchas
                    .lock()
                    .await
                    .retain(|_, (_, t)| now - *t < CAPTCHA_TTL);
                jan.thumb_failed
                    .lock()
                    .await
                    .retain(|_, t| now - *t < crate::domain::media::THUMB_FAIL_TTL);
            }
        });
        Ok(st)
    }

    /// 清理陈旧上传:先删过期记录,再删磁盘上无主的分片目录 / 合并残留。
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

    /// 无用户时创建初始管理员(同样拥有自己的独立个人空间)。
    async fn bootstrap_admin(&self, user: &str, pass: &str) {
        let Ok(users) = self.db.all_users().await else {
            return;
        };
        if !users.is_empty() {
            return;
        }
        match auth::hash_password(pass) {
            Ok(hash) => {
                let id = match self
                    .db
                    .create_user(user, &hash, true, [true, true, true, true])
                    .await
                {
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
                    .create_share(
                        HOME_NAME,
                        &crate::domain::paths::pretty(&dir),
                        Some(id),
                        "home",
                    )
                    .await;
                tracing::warn!("已创建初始管理员: {user} / {pass}(请尽快修改密码)");
            }
            Err(e) => tracing::error!("创建管理员失败: {e}"),
        }
    }

    /// 账号的个人空间根目录:固定为 data/users/<用户名>(所有账号一致,包括管理员)。
    pub fn home_dir(&self, username: &str) -> PathBuf {
        self.users_dir.join(username)
    }

    /// 签发验证码;答案只出现在服务端渲染的 SVG 中,10 分钟有效、一次性。
    pub async fn issue_captcha(&self) -> (String, String) {
        use rand::Rng;
        // ThreadRng 非 Send:随机数生成放在独立作用域,不跨越 await
        let code = {
            let mut rng = rand::thread_rng();
            let alpha: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
            (0..4)
                .map(|_| alpha[rng.gen_range(0..alpha.len())] as char)
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

    pub async fn check_captcha(&self, id: &str, answer: &str) -> bool {
        match self.captchas.lock().await.remove(id) {
            Some((want, t)) => {
                now_secs() - t < CAPTCHA_TTL && want.eq_ignore_ascii_case(answer.trim())
            }
            None => false,
        }
    }
}

/// 清扫各共享目录里残留的流式合并临时文件(服务器崩溃时可能遗留;隐藏文件,不影响浏览)。
async fn sweep_merge_temps(st: &AppState) {
    if let Ok(shares) = st.db.all_shares().await {
        for s in shares {
            let rd = std::fs::read_dir(&s.host_path);
            let Ok(rd) = rd else { continue };
            for de in rd.flatten() {
                let name = de.file_name().to_string_lossy().to_string();
                if name.starts_with(".nd-merge-") && name.ends_with(".tmp") {
                    let _ = std::fs::remove_file(de.path());
                }
            }
        }
    }
}

/// 清理 zip 打包等遗留临时文件(超过 24 小时的),防止异常退出后堆积。
async fn sweep_tmp_files(st: &AppState) {
    let cutoff = now_secs() - 24 * 3600;
    if let Ok(rd) = std::fs::read_dir(&st.tmp_dir) {
        for de in rd.flatten() {
            let Ok(meta) = de.metadata() else { continue };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let secs = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if secs < cutoff {
                continue;
            }
            let _ = std::fs::remove_file(de.path());
        }
    }
    // 过期分享链接行也顺带清理(定时兜底)
    let _ = st.db.prune_expired_links(now_secs()).await;
}
