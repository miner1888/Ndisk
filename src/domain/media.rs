use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;

const THUMB_WIDTH: u32 = 320;
const IMG_THUMB_WIDTH: u32 = 400;
pub const THUMB_FAIL_TTL: i64 = 300;
/// 转码任务视为僵死的超时:进程被杀 / 重启后允许重新拉起。
const TRANSCODE_STALE: i64 = 30 * 60;

/// ffmpeg 参数防注入:文件名允许以 `-` 开头,直接传会被当作选项。
fn arg(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if s.starts_with('-') {
        format!("./{s}")
    } else {
        s
    }
}

fn cache_key(share_id: i64, rel: &str) -> String {
    let mut h = Sha256::new();
    h.update(share_id.to_string().as_bytes());
    h.update(b"|");
    h.update(rel.as_bytes());
    h.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn transcode_out(st: &AppState, share_id: i64, rel: &str) -> PathBuf {
    st.media_dir.join(cache_key(share_id, rel)).join("f.mp4")
}
pub fn video_thumb(st: &AppState, share_id: i64, rel: &str) -> PathBuf {
    st.media_dir
        .join(cache_key(share_id, rel))
        .join("thumb.jpg")
}
pub fn image_thumb(st: &AppState, share_id: i64, rel: &str) -> PathBuf {
    st.media_dir.join(cache_key(share_id, rel)).join("img.jpg")
}

/// 保证 (share, rel) 有转码记录且任务在跑;完成且产物存在则跳过。
/// running 状态超过僵死超时视为进程已丢失,重新拉起。
pub async fn ensure_transcode(
    st: &AppState,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<(), ApiError> {
    let out = transcode_out(st, share_id, rel);
    if let Some((status, _out, updated)) = st.db.transcode(share_id, rel).await? {
        let running = status == "running" && crate::state::now_secs() - updated < TRANSCODE_STALE;
        let done = status == "done" && out.is_file();
        if running || done {
            return Ok(());
        }
    }
    let hash = cache_key(share_id, rel);
    st.db
        .transcode_start(
            share_id,
            rel,
            &hash,
            &out.to_string_lossy(),
            crate::state::now_secs(),
        )
        .await?;
    let st2 = st.clone();
    let input = input.to_path_buf();
    let rel2 = rel.to_string();
    tokio::spawn(async move {
        run_transcode(&st2, share_id, &rel2, &input).await;
    });
    Ok(())
}

async fn run_transcode(st: &AppState, share_id: i64, rel: &str, input: &Path) {
    let out = transcode_out(st, share_id, rel);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ok = match encode_with(st, input, &out, &cpu_encode_args()).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("转码失败 {rel}: {}", last_line(&e));
            false
        }
    };
    let _ = st
        .db
        .transcode_finish(share_id, rel, ok, crate::state::now_secs())
        .await;
}

/// CPU 转码参数(libx264,~CRF 23)
fn cpu_encode_args() -> Vec<String> {
    [
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-c:a",
        "aac", "-b:a", "128k",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 浏览器可直接播放、无需替换的容器格式
fn playable_ext(ext: &str) -> bool {
    matches!(ext, "mp4" | "webm" | "m3u8")
}

/// 内容嗅探:文件实际已是 MP4(ftyp)或 WebM 容器则无需再转。
/// 扩展名可能不可信(如 .avi 内容其实是 MP4),避免重复转码。
fn is_playable_container(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    use std::io::Read;
    let mut head = [0u8; 12];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    &head[4..8] == b"ftyp" || head[0..4] == [0x1A, 0x45, 0xDF, 0xA3]
}

/// 上传视频自动转码替换(后台开关开启时):把非 MP4/WebM 视频转成
/// H.264 MP4 后原子替换原文件;失败保留原文件,仅记录日志。
/// 串行执行(全局锁),避免多个大视频同时转码挤占 CPU。
pub async fn maybe_replace_transcode(st: &AppState, share_id: i64, rel: &str, target: &Path) {
    if st.db.setting("transcode_replace").await.as_deref() != Some("1") {
        return;
    }
    // 相对路径末段即文件名(domain 层不反向依赖 api 层)
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = crate::domain::paths::ext_of(name).to_lowercase();
    if kind_of(&ext) != Kind::Video || playable_ext(&ext) {
        return;
    }
    if st.ffmpeg.is_none() {
        tracing::warn!("自动转码已开启但未安装 ffmpeg,跳过: {rel}");
        return;
    }
    let _guard = st.transcode_lock.lock().await;
    // 拿到锁后再确认开关(等待期间可能被关闭)
    if st.db.setting("transcode_replace").await.as_deref() != Some("1") {
        return;
    }
    if !target.is_file() {
        return;
    }
    if is_playable_container(target) {
        return;
    }
    let tmp = target.with_extension("replace.mp4");
    tracing::info!("开始自动转码替换: {rel}");
    match encode_with(st, target, &tmp, &cpu_encode_args()).await {
        Ok(()) => {
            if tokio::fs::rename(&tmp, target).await.is_ok() {
                // 原文件已替换:旧缩略图 / 转码缓存全部作废
                purge(st, share_id, rel).await;
                tracing::info!("自动转码完成并已替换原视频: {rel}");
            } else {
                let _ = std::fs::remove_file(&tmp);
                tracing::warn!("自动转码产物替换失败,保留原视频: {rel}");
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("自动转码失败,保留原视频 {rel}: {}", last_line(&e));
        }
    }
}

/// 按给定编码参数跑一次 ffmpeg;成功且产物存在才返回 Ok
async fn encode_with(
    st: &AppState,
    input: &Path,
    out: &Path,
    enc: &[String],
) -> Result<(), String> {
    let Some(ffmpeg) = &st.ffmpeg else {
        return Err("未找到 ffmpeg".into());
    };
    let out_s = out.to_string_lossy().to_string();
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-nostdin".into(),
        "-i".into(),
        arg(input),
    ];
    args.extend(enc.iter().cloned());
    args.extend([
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-max_muxing_queue_size".to_string(),
        "2048".to_string(),
        arg(out),
    ]);
    let o = tokio::process::Command::new(ffmpeg)
        .args(&args)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if o.status.success() && out.is_file() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(&out_s);
        Err(String::from_utf8_lossy(&o.stderr).to_string())
    }
}

fn last_line(s: &str) -> String {
    s.lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 懒生成视频封面;并发由 pending 集合去重,失败窗口内不重复启动 ffmpeg。
pub async fn ensure_video_thumb(
    st: &AppState,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<PathBuf, ApiError> {
    ensure_media_asset(st, share_id, rel, input, Asset::VideoThumb).await
}

pub async fn ensure_image_thumb(
    st: &AppState,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<PathBuf, ApiError> {
    ensure_media_asset(st, share_id, rel, input, Asset::ImageThumb).await
}

#[derive(Clone, Copy, PartialEq)]
enum Asset {
    VideoThumb,
    ImageThumb,
}

impl Asset {
    fn out_path(self, st: &AppState, share_id: i64, rel: &str) -> PathBuf {
        match self {
            Asset::VideoThumb => video_thumb(st, share_id, rel),
            Asset::ImageThumb => image_thumb(st, share_id, rel),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Asset::VideoThumb => "封面",
            Asset::ImageThumb => "缩略图",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Asset::VideoThumb => "vthumb",
            Asset::ImageThumb => "ithumb",
        }
    }
}

async fn ensure_media_asset(
    st: &AppState,
    share_id: i64,
    rel: &str,
    input: &Path,
    what: Asset,
) -> Result<PathBuf, ApiError> {
    let out = what.out_path(st, share_id, rel);
    if out.is_file() {
        return Ok(out);
    }
    let key = (share_id, rel.to_string());
    {
        let failed = st.thumb_failed.lock().await;
        if let Some(t) = failed.get(&key) {
            if crate::state::now_secs() - *t < THUMB_FAIL_TTL {
                return Err(ApiError::not_found(format!(
                    "{}生成失败,请稍后重试",
                    what.label()
                )));
            }
        }
    }
    {
        let mut pending = st.thumb_pending.lock().await;
        if pending.contains(&key) {
            return Err(ApiError::not_found(format!(
                "{}正在生成,请稍后重试",
                what.label()
            )));
        }
        pending.insert(key.clone());
    }
    let result = if what == Asset::VideoThumb {
        gen_video_thumb(st, input, &out).await
    } else {
        gen_image_thumb(st, input, &out).await
    };
    st.thumb_pending.lock().await.remove(&key);
    match result {
        Ok(()) => {
            // 登记到 media_cache:既便于统计占用,也让清理能按“源文件是否还在”判断
            let _ = st
                .db
                .media_cache_put(
                    share_id,
                    rel,
                    what.kind(),
                    &out.to_string_lossy(),
                    crate::state::now_secs(),
                )
                .await;
            Ok(out)
        }
        Err(e) => {
            tracing::warn!("{}生成失败 ({share_id},{rel}): {e}", what.label());
            st.thumb_failed
                .lock()
                .await
                .insert(key, crate::state::now_secs());
            Err(ApiError::not_found(format!(
                "{}生成失败: {e}",
                what.label()
            )))
        }
    }
}

async fn run_ffmpeg(st: &AppState, args: &[&str]) -> Result<(), String> {
    let Some(ffmpeg) = &st.ffmpeg else {
        return Err("未找到 ffmpeg".into());
    };
    let status = tokio::process::Command::new(ffmpeg)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("ffmpeg 启动失败: {e}"))?;
    if status.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&status.stderr).to_string())
    }
}

async fn gen_video_thumb(st: &AppState, input: &Path, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out_s = out.to_string_lossy().to_string();
    let mut last = String::new();
    for seek in ["1", "0"] {
        let r = run_ffmpeg(
            st,
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                seek,
                "-i",
                &arg(input),
                "-frames:v",
                "1",
                "-q:v",
                "4",
                "-vf",
                &format!("scale='min({THUMB_WIDTH},iw)':-2"),
                "-pix_fmt",
                "yuvj420p",
                &arg(out),
            ],
        )
        .await;
        if r.is_ok() && out.is_file() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&out_s);
        last = r.err().unwrap_or_default();
    }
    Err(format!("封面抽帧失败: {last}"))
}

async fn gen_image_thumb(st: &AppState, input: &Path, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let r = run_ffmpeg(
        st,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-i",
            &arg(input),
            "-vf",
            &format!("scale='min({IMG_THUMB_WIDTH},iw)':-2"),
            "-frames:v",
            "1",
            "-q:v",
            "3",
            "-pix_fmt",
            "yuvj420p",
            &arg(out),
        ],
    )
    .await;
    if r.is_ok() && out.is_file() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(out);
        Err(format!("缩略图生成失败: {}", r.err().unwrap_or_default()))
    }
}

/// 删除 / 重命名 / 覆盖源文件时联动清理缓存记录与落盘产物。
pub async fn purge(st: &AppState, share_id: i64, rel: &str) {
    if let Ok(Some(out)) = st.db.transcode_take(share_id, rel).await {
        let _ = std::fs::remove_file(out);
    }
    if let Ok(paths) = st.db.media_cache_drop(share_id, rel).await {
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
    }
    // 兜底:按约定路径再删一次(处理早期没有索引记录的文件)
    let _ = std::fs::remove_file(video_thumb(st, share_id, rel));
    let _ = std::fs::remove_file(image_thumb(st, share_id, rel));
    prune_empty_dirs(st, share_id, rel);
}

/// 缓存目录按 (share, rel) 哈希分目录,产物删空后把空目录一并移除,避免 data/media 堆积空文件夹
fn prune_empty_dirs(st: &AppState, share_id: i64, rel: &str) {
    let dir = st.media_dir.join(cache_key(share_id, rel));
    if let Ok(rd) = std::fs::read_dir(&dir) {
        if rd.flatten().next().is_none() {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

/// 删除文件后顺手回收其空掉的缓存目录(目录非空时 remove_dir 会失败,安全)
fn prune_parent(path: &str) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

// ---------------- 缓存治理:统计 / 自动清理 / 手动清理 ----------------

/// 转码产物最长保留时间(天);到期即回收,播放时会按需重新转码
const CACHE_MAX_AGE_DAYS: i64 = 30;
/// 无索引记录的孤儿文件保留时间(秒),避免误删正在生成的产物
const ORPHAN_GRACE: i64 = 3600;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CacheStats {
    /// 派生文件总数(缩略图 + 转码产物)
    pub files: u64,
    /// 总占用字节
    pub bytes: u64,
    /// 缩略图数量 / 占用
    pub thumbs: u64,
    pub thumb_bytes: u64,
    /// 转码产物数量 / 占用
    pub transcodes: u64,
    pub transcode_bytes: u64,
    /// 磁盘上存在的缓存目录数
    pub dirs: u64,
}

/// 统计缓存占用(转码 + 缩略图 + 磁盘目录数)
pub async fn stats(st: &AppState) -> CacheStats {
    let mut s = CacheStats::default();
    if let Ok(rd) = std::fs::read_dir(&st.media_dir) {
        for de in rd.flatten() {
            if de.path().is_dir() {
                s.dirs += 1;
            }
        }
    }
    let add = |path: &str, is_transcode: bool, s: &mut CacheStats| {
        if let Ok(m) = std::fs::metadata(path) {
            if m.is_file() {
                s.files += 1;
                s.bytes += m.len();
                if is_transcode {
                    s.transcodes += 1;
                    s.transcode_bytes += m.len();
                } else {
                    s.thumbs += 1;
                    s.thumb_bytes += m.len();
                }
            }
        }
    };
    // 只按索引统计,避免把磁盘上尚未登记的临时产物算进来
    if let Ok(rows) = st.db.media_cache_list().await {
        for (_, _, _, out, _) in rows {
            add(&out, false, &mut s);
        }
    }
    if let Ok(rows) = st.db.transcode_list().await {
        for (_, _, _, out, _) in rows {
            add(&out, true, &mut s);
        }
    }
    s
}

/// 清空全部派生缓存(缩略图 + 转码),返回释放的字节数。播放时会按需重新生成。
pub async fn clear_all(st: &AppState) -> u64 {
    let before = stats(st).await.bytes;
    if let Ok(rows) = st.db.media_cache_list().await {
        for (_, _, _, out, _) in rows {
            let _ = std::fs::remove_file(out);
        }
    }
    if let Ok(rows) = st.db.transcode_list().await {
        for (_, _, _, out, _) in rows {
            let _ = std::fs::remove_file(out);
        }
    }
    let _ = st.db.media_cache_clear().await;
    let _ = st.db.transcode_clear().await;
    // 清掉残留的空目录 / 孤儿文件
    if let Ok(rd) = std::fs::read_dir(&st.media_dir) {
        for de in rd.flatten() {
            let p = de.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            }
        }
    }
    let after = stats(st).await.bytes;
    before.saturating_sub(after)
}

/// 定期清理:①源文件已不存在的派生文件 ②超过保留期的派生文件 ③磁盘上没有索引的孤儿缓存目录
pub async fn sweep_cache(st: &AppState) {
    let now = crate::state::now_secs();
    let max_age = CACHE_MAX_AGE_DAYS * 86400;
    let mut removed = 0usize;

    // 共享根目录缓存:(share_id → root),用于判断源文件是否还在
    let shares = st.db.all_shares().await.unwrap_or_default();

    // 缩略图索引
    if let Ok(rows) = st.db.media_cache_list().await {
        for (share_id, rel, kind, out, updated) in rows {
            let expired = now - updated > max_age;
            let src_gone = match shares.iter().find(|s| s.id == share_id) {
                Some(sh) => !std::path::Path::new(&sh.host_path).join(&rel).exists(),
                None => true,
            };
            if expired || src_gone {
                let _ = std::fs::remove_file(&out);
                prune_parent(&out);
                let _ = st.db.media_cache_delete(share_id, &rel, &kind).await;
                removed += 1;
            }
        }
    }

    // 转码记录:源文件没了 / 产物没了 / 记录过期,都清掉(下次播放重新生成)
    if let Ok(rows) = st.db.transcode_list().await {
        for (share_id, rel, status, out, updated) in rows {
            let src_gone = match shares.iter().find(|s| s.id == share_id) {
                Some(sh) => !std::path::Path::new(&sh.host_path).join(&rel).exists(),
                None => true,
            };
            let stale_failed = status == "failed" && now - updated > THUMB_FAIL_TTL;
            let expired = now - updated > max_age;
            let broken_done = status == "done" && !std::path::Path::new(&out).exists();
            if src_gone || stale_failed || expired || broken_done {
                let _ = std::fs::remove_file(&out);
                prune_parent(&out);
                let _ = st.db.transcode_delete(share_id, &rel).await;
                removed += 1;
            }
        }
    }

    // 磁盘孤儿:没有任何索引引用、且足够旧的缓存目录(崩溃 / 旧版本遗留)
    let known: std::collections::HashSet<String> = {
        let mut set = std::collections::HashSet::new();
        if let Ok(rows) = st.db.media_cache_list().await {
            for (_, _, _, out, _) in rows {
                if let Some(p) = std::path::Path::new(&out).parent() {
                    set.insert(p.to_string_lossy().to_string());
                }
            }
        }
        if let Ok(rows) = st.db.transcode_list().await {
            for (_, _, _, out, _) in rows {
                if let Some(p) = std::path::Path::new(&out).parent() {
                    set.insert(p.to_string_lossy().to_string());
                }
            }
        }
        set
    };
    if let Ok(rd) = std::fs::read_dir(&st.media_dir) {
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
                .map(|t| {
                    t.elapsed()
                        .map(|d| d.as_secs() as i64 > ORPHAN_GRACE)
                        .unwrap_or(false)
                })
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

pub fn mime_of(ext: &str) -> String {
    match ext {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "flv" => "video/x-flv",
        "wmv" => "video/x-ms-wmv",
        "mpg" | "mpeg" => "video/mpeg",
        "ts" | "m2ts" | "mts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "opus" => "audio/opus",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" | "yaml" | "yml" | "conf" | "ini" | "toml" | "csv" => {
            "text/plain; charset=utf-8"
        }
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Other,
}

pub fn kind_of(ext: &str) -> Kind {
    const IMAGE: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "tif", "tiff", "ico", "jfif",
        "heic", "heif",
    ];
    const VIDEO: &[&str] = &[
        "mp4", "webm", "ogv", "mov", "m4v", "avi", "mkv", "flv", "wmv", "mpg", "mpeg", "ts", "3gp",
        "3g2", "ogm", "m2ts", "mts", "vob", "asf", "rmvb", "f4v",
    ];
    const AUDIO: &[&str] = &[
        "mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", "wma", "amr",
    ];
    if IMAGE.contains(&ext) {
        Kind::Image
    } else if VIDEO.contains(&ext) {
        Kind::Video
    } else if AUDIO.contains(&ext) {
        Kind::Audio
    } else {
        Kind::Other
    }
}

/// 浏览器可直接播放的容器。
pub fn native_video(ext: &str) -> bool {
    matches!(ext, "mp4" | "m4v" | "webm" | "ogv" | "mov")
}

pub fn editable_text(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "log"
            | "md"
            | "markdown"
            | "yaml"
            | "yml"
            | "conf"
            | "ini"
            | "toml"
            | "json"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "js"
            | "mjs"
            | "ts"
            | "sh"
            | "bash"
            | "py"
            | "sql"
            | "properties"
            | "env"
    )
}

/// 表格类(SheetJS 在线读写);csv / tsv 走文本端点保存。
pub fn is_sheet(ext: &str) -> bool {
    matches!(ext, "xlsx" | "xlsm" | "xls" | "ods")
}

pub fn preview_inline(ext: &str) -> bool {
    !matches!(kind_of(ext), Kind::Other)
        || editable_text(ext)
        || is_sheet(ext)
        || matches!(
            ext,
            "pdf" | "docx" | "docm" | "pptx" | "doc" | "odt" | "ods" | "odp" | "rtf"
        )
}

pub fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Other => "other",
    }
}
