//! 分片上传核心:分片数学、已收分片探测、磁盘空间校验、合并落盘。
//! 全部函数以显式参数接收目录,不依赖应用状态。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::ApiError;

/// 一次分片上传会话(创建时构造,完成合并时使用)。
#[derive(Clone, Debug)]
pub struct UploadPlan {
    pub token: String,
    pub user_id: i64,
    pub share_id: i64,
    pub rel_path: String,
    pub filename: String,
    pub total_size: i64,
    pub chunk_size: i64,
    pub num_parts: i64,
}

impl UploadPlan {
    /// 第 `i` 片应有的字节数(最后一片是余数;大小为 0 的文件只有 1 片 0 字节)。
    pub fn part_len(&self, i: i64) -> i64 {
        if self.total_size == 0 {
            return 0;
        }
        if i + 1 == self.num_parts {
            let last = self.total_size % self.chunk_size;
            if last == 0 {
                self.chunk_size
            } else {
                last
            }
        } else {
            self.chunk_size
        }
    }
}

/// 分片暂存目录:`data/uploads/<token>/`。
pub fn part_dir(uploads_dir: &Path, token: &str) -> PathBuf {
    uploads_dir.join(token)
}

/// 列出磁盘上已收到的分片序号(升序)。
pub fn received_parts(uploads_dir: &Path, token: &str) -> Vec<i64> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(part_dir(uploads_dir, token)) {
        for de in rd.flatten() {
            if let Some(rest) = de.file_name().to_string_lossy().strip_prefix("part") {
                if let Ok(i) = rest.parse() {
                    v.push(i);
                }
            }
        }
    }
    v.sort();
    v
}

/// 目标路径所在卷的可用空间;获取失败时返回 `None`(调用方跳过检查)。
pub fn available_space(target: &Path) -> Option<u64> {
    fs2::available_space(target).ok()
}

/// 判断两路径是否在同一卷(决定合并产物走 rename 还是需要 copy)。
pub fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.dev() == mb.dev(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let prefix = |p: &Path| {
            p.canonicalize()
                .ok()
                .and_then(|c| {
                    c.to_string_lossy()
                        .split(['\\', '/'])
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_default()
        };
        !prefix(a).is_empty() && prefix(a) == prefix(b)
    }
}

const DISK_MARGIN: i64 = 256 * 1024 * 1024;

/// 预留 256MB 系统余量后再判断磁盘空间是否够用。
pub fn ensure_disk_space(target: &Path, size: i64) -> Result<(), ApiError> {
    if let Some(avail) = available_space(target) {
        let need = size + DISK_MARGIN;
        if (avail as i64) < need {
            let gb = crate::domain::human_bytes;
            return Err(ApiError::bad_request(format!(
                "服务器磁盘空间不足:剩余 {},本次上传需要 {}(文件 {} + 系统余量)",
                gb(avail as i64),
                gb(need),
                gb(size)
            )));
        }
    }
    Ok(())
}

/// 合并全部分片到目标文件:先写 `.nd-merge-<token>.tmp` 再 rename,
/// 全程校验总大小;失败时清理临时文件。
pub fn merge_and_finish(
    plan: &UploadPlan,
    uploads_dir: &Path,
    target: &Path,
) -> Result<(), ApiError> {
    let dir = part_dir(uploads_dir, &plan.token);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;
    }
    let tmp = target.with_file_name(format!(".nd-merge-{}.tmp", plan.token));
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::File::create(&tmp).map_err(|e| friendly_io("创建合并临时文件", e))?;
    for i in 0..plan.num_parts {
        let p = dir.join(format!("part{i}"));
        let mut f = std::fs::File::open(&p).map_err(|e| friendly_io(&format!("读取分片 part{i}"), e))?;
        std::io::copy(&mut f, &mut out).map_err(|e| friendly_io(&format!("合并分片 part{i}"), e))?;
        let _ = std::fs::remove_file(&p);
    }
    out.flush().map_err(|e| friendly_io("刷新合并产物", e))?;
    drop(out);
    let got = std::fs::metadata(&tmp).map(|m| m.len() as i64).unwrap_or(-1);
    if got != plan.total_size {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!("合并后大小 {got} 与预期 {} 不符", plan.total_size)));
    }
    std::fs::rename(&tmp, target).map_err(|e| friendly_io("落位目标文件", e))?;
    Ok(())
}

fn friendly_io(what: &str, e: std::io::Error) -> ApiError {
    if e.kind() == std::io::ErrorKind::StorageFull || e.raw_os_error() == Some(28) {
        return ApiError::internal(format!("{what}失败:服务器磁盘空间不足,请清理后重试"));
    }
    ApiError::internal(format!("{what}失败: {e}"))
}

/// 丢弃上传会话:删登记 + 删分片目录。
pub async fn abort_session(db: &crate::db::Db, uploads_dir: &Path, token: &str, id: i64) {
    let _ = db.delete_upload(id).await;
    let _ = std::fs::remove_dir_all(part_dir(uploads_dir, token));
}

/// 合并互斥锁:防止同一会话的并发 complete 交错写同一合并产物。
/// 锁文件位于 uploads_dir(token 同级),进程崩溃后的残留由
/// `cleanup_uploads` 的无主文件清扫回收。
pub struct MergeLock {
    _file: std::fs::File,
    path: PathBuf,
}

impl MergeLock {
    pub fn acquire(uploads_dir: &Path, token: &str) -> Result<MergeLock, ApiError> {
        use fs2::FileExt;
        let path = uploads_dir.join(format!("{token}.merge-lock"));
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| ApiError::internal(format!("打开合并锁失败: {e}")))?;
        f.try_lock_exclusive()
            .map_err(|_| ApiError::conflict("该上传会话正在合并,请勿重复提交"))?;
        Ok(MergeLock { _file: f, path })
    }
}

impl Drop for MergeLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(total: i64, chunk: i64) -> UploadPlan {
        UploadPlan {
            token: "tok".into(),
            user_id: 1,
            share_id: 1,
            rel_path: String::new(),
            filename: "f.bin".into(),
            total_size: total,
            chunk_size: chunk,
            num_parts: if total == 0 { 1 } else { (total + chunk - 1) / chunk },
        }
    }

    #[test]
    fn part_len_math() {
        let p = plan(10_000, 8_000_000);
        assert_eq!(p.part_len(0), 10_000);
        let p2 = plan(16_000_000, 8_000_000);
        assert_eq!(p2.part_len(0), 8_000_000);
        assert_eq!(p2.part_len(1), 8_000_000);
        // 恰好整除:最后一片也是整片。
        let p3 = plan(16_000_000, 8_000_000);
        assert_eq!(p3.num_parts, 2);
        let zero = plan(0, 8_000_000);
        assert_eq!(zero.part_len(0), 0);
    }

    #[test]
    fn merge_and_finish_roundtrip() {
        let base = std::env::temp_dir().join(format!("ndv2_up_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let uploads = base.join("uploads");
        let plan = plan(5, 2); // 3 片:2 + 2 + 1
        let dir = part_dir(&uploads, &plan.token);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("part0"), b"12").unwrap();
        std::fs::write(dir.join("part1"), b"34").unwrap();
        std::fs::write(dir.join("part2"), b"5").unwrap();
        let target = base.join("out").join("done.bin");
        merge_and_finish(&plan, &uploads, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"12345");
        // 分片文件被逐个删除;目录本身由调用方(complete)清理。
        assert!(dir.exists());
        assert!(!dir.join("part0").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
