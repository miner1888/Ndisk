//! 视频转码:按需转 H.264 MP4(缓存,幂等),以及"上传后自动转码替换原文件"。

use std::path::Path;

use crate::db::Db;
use crate::domain::now_secs;
use crate::error::ApiError;

use super::ffmpeg::{cpu_encode_args, encode_with, last_line};
use super::transcode_out;

/// 转码任务 30 分钟无更新视为僵死,允许重新发起。
const TRANSCODE_STALE: i64 = 30 * 60;

fn playable_ext(ext: &str) -> bool {
    matches!(ext, "mp4" | "webm" | "m3u8")
}

/// 读文件头判断是否已是浏览器可播容器(MP4 `ftyp` / Matroska EBML)。
fn is_playable_container(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut head = [0u8; 12];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    &head[4..8] == b"ftyp" || head[0..4] == [0x1A, 0x45, 0xDF, 0xA3]
}

/// 请求转码(幂等):已在跑或已完成则直接返回,否则登记并后台启动。
pub async fn ensure_transcode(
    db: &Db,
    media_dir: &Path,
    ffmpeg: &Path,
    share_id: i64,
    rel: &str,
    input: &Path,
) -> Result<(), ApiError> {
    let out = transcode_out(media_dir, share_id, rel);
    if let Some(row) = db.transcode(share_id, rel).await? {
        let running = row.status == "running" && now_secs() - row.updated_at < TRANSCODE_STALE;
        let done = row.status == "done" && out.is_file();
        if running || done {
            if done {
                // 命中即续期:sweep 的 30 天按"未访问"计,热播视频不回炉。
                let _ = db.transcode_touch(share_id, rel, now_secs()).await;
            }
            return Ok(());
        }
    }
    let hash = super::cache_key(share_id, rel);
    db.transcode_start(share_id, rel, &hash, &out.to_string_lossy(), now_secs()).await?;
    let db2 = db.clone();
    let media2 = media_dir.to_path_buf();
    let ffmpeg2 = ffmpeg.to_path_buf();
    let input = input.to_path_buf();
    let rel2 = rel.to_string();
    tokio::spawn(async move {
        use futures_util::FutureExt;
        let task = run_transcode(&db2, &media2, &ffmpeg2, share_id, &rel2, &input);
        if std::panic::AssertUnwindSafe(task).catch_unwind().await.is_err() {
            // 转码任务 panic:登记置 failed,避免永远停在 running(300s 后由 sweep 回收)。
            tracing::error!("转码任务异常(panic): {rel2}");
            let _ = db2.transcode_finish(share_id, &rel2, false, now_secs()).await;
        }
    });
    Ok(())
}

async fn run_transcode(db: &Db, media_dir: &Path, ffmpeg: &Path, share_id: i64, rel: &str, input: &Path) {
    let out = transcode_out(media_dir, share_id, rel);
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ok = match encode_with(ffmpeg, input, &out, &cpu_encode_args()).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("转码失败 {rel}: {}", last_line(&e));
            false
        }
    };
    let _ = db.transcode_finish(share_id, rel, ok, now_secs()).await;
}

/// 上传完成后(开关开启时)把非 MP4/WebM 视频转码并原地替换。
/// `replace_lock` 由调用方持有(逐个排队,避免并发转码打满 CPU)。
pub async fn maybe_replace_transcode(
    db: &Db,
    media_dir: &Path,
    ffmpeg: Option<&Path>,
    replace_lock: &tokio::sync::Mutex<()>,
    share_id: i64,
    rel: &str,
    target: &Path,
) {
    // 默认关闭,由用户在后台决定是否开启(开启后:非 MP4/WebM 视频上传/下载完成
    // 即自动转 MP4 并替换原文件,失败保留原件)。
    if db.setting("transcode_replace").await.as_deref() != Some("1") {
        return;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = crate::domain::pathguard::ext_of(name);
    if super::kind_of(&ext) != super::Kind::Video || playable_ext(&ext) {
        return;
    }
    let Some(ffmpeg) = ffmpeg else {
        tracing::warn!("自动转码已开启但未安装 ffmpeg,跳过: {rel}");
        return;
    };
    let _guard = replace_lock.lock().await;
    // 等锁期间设置可能被修改;二次确认。
    if db.setting("transcode_replace").await.as_deref() != Some("1") {
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
    match encode_with(ffmpeg, target, &tmp, &cpu_encode_args()).await {
        Ok(()) => {
            if tokio::fs::rename(&tmp, target).await.is_ok() {
                super::purge(db, media_dir, share_id, rel).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_detection() {
        let dir = std::env::temp_dir().join(format!("ndv2_tc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mp4 = dir.join("x.mp4");
        let mut data = vec![0u8; 16];
        data[4..8].copy_from_slice(b"ftyp");
        std::fs::write(&mp4, &data).unwrap();
        assert!(is_playable_container(&mp4));

        let mkv = dir.join("x.mkv");
        // EBML 魔数 + 8 字节填充(凑足 12 字节以便整头读取)。
        let mut ebml = vec![0x1Au8, 0x45, 0xDF, 0xA3];
        ebml.extend_from_slice(&[0; 8]);
        std::fs::write(&mkv, ebml).unwrap();
        assert!(is_playable_container(&mkv));

        let avi = dir.join("x.avi");
        std::fs::write(&avi, b"RIFF....AVI LIST").unwrap();
        assert!(!is_playable_container(&avi));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playable_extensions() {
        assert!(playable_ext("mp4"));
        assert!(!playable_ext("avi"));
    }
}
