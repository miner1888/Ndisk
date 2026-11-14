use std::path::PathBuf;

use crate::error::ApiError;
use crate::state::AppState;

/// 分片上传会话(与数据库 uploads 表对应)。
#[derive(Clone, Debug)]
pub struct Upload {
    pub token: String,
    pub user_id: i64,
    pub share_id: i64,
    pub rel_path: String,
    pub filename: String,
    pub total_size: i64,
    pub chunk_size: i64,
    pub num_parts: i64,
}

impl Upload {
    /// 第 i 片的期望字节数;空文件只有一个 0 字节分片。
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

pub fn part_dir(st: &AppState, token: &str) -> PathBuf {
    st.uploads_dir.join(token)
}

pub fn received_parts(st: &AppState, token: &str) -> Vec<i64> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(part_dir(st, token)) {
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

/// 目标目录所在文件系统的剩余可用空间(字节);查询失败返回 None(不阻塞上传)。
pub fn available_space(target: &std::path::Path) -> Option<u64> {
    fs2::available_space(target).ok()
}

/// 判断两个路径是否位于同一文件系统(卷)。
/// 同卷时流式合并"分片删除释放的空间"正好被"临时文件增长"抵消,净增 ≈ 0;
/// 跨卷时临时文件写入目标卷,需要目标卷有完整余量。
pub fn same_volume(a: &std::path::Path, b: &std::path::Path) -> bool {
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
        // Windows:比较盘符 / UNC 前缀;前缀不一致按跨卷处理(保守,触发预检)
        let prefix = |p: &std::path::Path| {
            p.canonicalize()
                .ok()
                .and_then(|c| {
                    c.to_string_lossy()
                        .splitn(4, ['\\', '/'])
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_default()
        };
        !prefix(a).is_empty() && prefix(a) == prefix(b)
    }
}

/// 上传前的磁盘余量预检:需要 文件体积 + 256MB 余量。
/// 提前给出明确报错,而不是分片传完才在合并阶段 ENOSPC。
pub fn ensure_disk_space(target: &std::path::Path, size: i64) -> Result<(), ApiError> {
    const MARGIN: i64 = 256 * 1024 * 1024;
    if let Some(avail) = available_space(target) {
        let need = size + MARGIN;
        if (avail as i64) < need {
            let gb = |b: i64| format!("{:.1} GB", b as f64 / 1073741824.0);
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

/// 流式合并:按序把分片追加写入**目标目录内的隐藏临时文件**,
/// 每耗尽一个分片立即删除,磁盘峰值占用 ≈ 文件体积(而不是分片 + 合并产物的 2 倍)。
/// 成功后原子 rename 为正式文件名;失败则清理临时文件与整个上传会话(分片已耗尽,无法续传)。
/// 返回 Err 时调用方必须终止会话。
pub fn merge_and_finish(
    upload: &Upload,
    st: &AppState,
    target: &std::path::Path,
) -> Result<(), ApiError> {
    use std::io::Write;
    let dir = part_dir(st, &upload.token);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;
    }
    let tmp = target.with_file_name(format!(".nd-merge-{}.tmp", upload.token));
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::File::create(&tmp).map_err(|e| friendly_io("创建合并临时文件", e))?;
    for i in 0..upload.num_parts {
        let p = dir.join(format!("part{i}"));
        let mut f =
            std::fs::File::open(&p).map_err(|e| friendly_io(&format!("读取分片 part{i}"), e))?;
        std::io::copy(&mut f, &mut out)
            .map_err(|e| friendly_io(&format!("合并分片 part{i}"), e))?;
        // 分片内容已耗尽:立即释放磁盘空间(删除后合并不可重试,失败即终止会话)
        let _ = std::fs::remove_file(&p);
    }
    out.flush().map_err(|e| friendly_io("刷新合并产物", e))?;
    drop(out);
    let got = std::fs::metadata(&tmp)
        .map(|m| m.len() as i64)
        .unwrap_or(-1);
    if got != upload.total_size {
        let _ = std::fs::remove_file(&tmp);
        return Err(ApiError::internal(format!(
            "合并后大小 {got} 与预期 {} 不符",
            upload.total_size
        )));
    }
    std::fs::rename(&tmp, target).map_err(|e| friendly_io("落位目标文件", e))?;
    Ok(())
}

/// 把常见 IO 错误翻译成用户能懂的提示(尤其磁盘空间不足)。
fn friendly_io(what: &str, e: std::io::Error) -> ApiError {
    if e.kind() == std::io::ErrorKind::StorageFull || e.raw_os_error() == Some(28) {
        return ApiError::internal(format!("{what}失败:服务器磁盘空间不足,请清理后重试"));
    }
    ApiError::internal(format!("{what}失败: {e}"))
}

/// 上传会话整体终止:删记录 + 删分片目录(合并失败 / 用户取消共用)。
pub async fn abort_session(st: &AppState, token: &str, id: i64) {
    let _ = st.db.delete_upload(id).await;
    let _ = std::fs::remove_dir_all(part_dir(st, token));
}
