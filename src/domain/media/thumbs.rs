//! 缩略图生成:视频封面(抽帧,1s → 0s 回退)与图片缩略图。
//! 失败 300s 内不重试;同一文件同时只生成一次(并发去重)。
//!
//! 状态(进行中集合 / 失败时间表)由调用方以 `ThumbGuard` 持有并传入,
//! 本模块不持有任何全局可变状态。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::db::Db;
use crate::domain::now_secs;
use crate::error::ApiError;

use super::ffmpeg::{arg, run_ffmpeg};
use super::{image_thumb, video_thumb};

/// 生成失败后的冷却时间(秒);期间直接告知"稍后重试"。
pub const THUMB_FAIL_TTL: i64 = 300;
const THUMB_WIDTH: u32 = 320;
const IMG_THUMB_WIDTH: u32 = 400;

/// 缩略图并发守卫:一张状态表同时表达"正在生成"(并发去重)与
/// "上次失败时间"(失败退避),键为 `(share_id, rel)`。
#[derive(Default)]
pub struct ThumbGuard {
    inner: Mutex<HashMap<(i64, String), ThumbEntry>>,
}

#[derive(Clone, Copy, Default)]
struct ThumbEntry {
    running: bool,
    failed_at: i64,
}

impl ThumbGuard {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Asset {
    VideoThumb,
    ImageThumb,
}

impl Asset {
    fn out_path(self, media_dir: &Path, share_id: i64, rel: &str) -> PathBuf {
        match self {
            Asset::VideoThumb => video_thumb(media_dir, share_id, rel),
            Asset::ImageThumb => image_thumb(media_dir, share_id, rel),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Asset::VideoThumb => "封面",
            Asset::ImageThumb => "缩略图",
        }
    }
    /// 缓存登记的 kind。
    fn kind(self) -> &'static str {
        match self {
            Asset::VideoThumb => "vthumb",
            Asset::ImageThumb => "ithumb",
        }
    }
}

pub async fn ensure_video_thumb(
    db: &Db,
    media_dir: &Path,
    guard: &ThumbGuard,
    ffmpeg: &Path,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<PathBuf, ApiError> {
    let ctx = ThumbCtx { db, media_dir, guard, ffmpeg, share_id, rel, input };
    ensure_asset(ctx, Asset::VideoThumb).await
}

pub async fn ensure_image_thumb(
    db: &Db,
    media_dir: &Path,
    guard: &ThumbGuard,
    ffmpeg: &Path,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<PathBuf, ApiError> {
    let ctx = ThumbCtx { db, media_dir, guard, ffmpeg, share_id, rel, input };
    ensure_asset(ctx, Asset::ImageThumb).await
}

/// 一次缩略图 ensure 所需的全部环境,收敛 `ensure_asset` 的参数数量。
struct ThumbCtx<'a> {
    db: &'a Db,
    media_dir: &'a Path,
    guard: &'a ThumbGuard,
    ffmpeg: &'a Path,
    share_id: i64,
    rel: &'a str,
    input: &'a Path,
}

async fn ensure_asset(ctx: ThumbCtx<'_>, what: Asset) -> Result<PathBuf, ApiError> {
    let ThumbCtx { db, media_dir, guard, ffmpeg, share_id, rel, input } = ctx;
    let out = what.out_path(media_dir, share_id, rel);
    if out.is_file() {
        // 命中即续期:sweep 的 30 天按"未访问"计。
        let _ = db.media_cache_touch(share_id, rel, what.kind(), now_secs()).await;
        return Ok(out);
    }
    let key = (share_id, rel.to_string());
    {
        let m = guard.inner.lock().await;
        match m.get(&key) {
            Some(e) if e.running => {
                return Err(ApiError::not_found(format!("{}正在生成,请稍后重试", what.label())));
            }
            Some(e) if now_secs() - e.failed_at < THUMB_FAIL_TTL => {
                return Err(ApiError::not_found(format!("{}生成失败,请稍后重试", what.label())));
            }
            _ => {}
        }
    }
    guard.inner.lock().await.insert(key.clone(), ThumbEntry { running: true, failed_at: 0 });
    let result = if what == Asset::VideoThumb {
        gen_video_thumb(ffmpeg, input, &out).await
    } else {
        gen_image_thumb(ffmpeg, input, &out).await
    };
    {
        let mut m = guard.inner.lock().await;
        match result {
            Ok(()) => {
                m.remove(&key);
            }
            Err(e) => {
                m.insert(key, ThumbEntry { running: false, failed_at: now_secs() });
                tracing::warn!("{}生成失败 ({share_id},{rel}): {e}", what.label());
                return Err(ApiError::not_found(format!("{}生成失败: {e}", what.label())));
            }
        }
    }
    let _ = db
        .media_cache_put(share_id, rel, what.kind(), &out.to_string_lossy(), now_secs())
        .await;
    Ok(out)
}

async fn gen_video_thumb(ffmpeg: &Path, input: &Path, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let out_s = out.to_string_lossy().to_string();
    let mut last = String::new();
    // 先抽 1s 处;过短的片子退回 0s。
    for seek in ["1", "0"] {
        let r = run_ffmpeg(
            ffmpeg,
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

async fn gen_image_thumb(ffmpeg: &Path, input: &Path, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let r = run_ffmpeg(
        ffmpeg,
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

/// 清理过期的失败记录(每小时后台任务调用)。
pub async fn sweep_failures(guard: &ThumbGuard) {
    let now = now_secs();
    guard.inner.lock().await.retain(|_, e| e.running || now - e.failed_at < THUMB_FAIL_TTL);
}
