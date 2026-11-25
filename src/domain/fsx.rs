//! 文件系统原语:原子写入、递归统计、递归收集、时间换算。

use std::path::{Path, PathBuf};

/// 原子写入:先写同目录临时文件再 rename,失败时清理临时文件。
pub async fn atomic_write(full: &Path, data: &[u8]) -> Result<(), String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dir = full.parent().ok_or_else(|| "非法路径".to_string())?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0);
    let rnd: u32 = rand::random();    let tmp = dir.join(format!(".nd-save-{}-{ts}-{rnd:x}.tmp", std::process::id()));
    let write = async {
        tokio::fs::write(&tmp, data).await?;
        tokio::fs::rename(&tmp, full).await
    };
    if let Err(e) = write.await {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("保存失败: {e}"));
    }
    Ok(())
}

/// 递归遍历目录(跳过隐藏项与符号链接,深度不超过 `max_depth`)。
/// `on_file(rel, path, meta)` 处理普通文件;`on_dir(rel, path)` 处理子目录,
/// 返回 false 时跳过该子树。`rel` 为相对调用根的 `/` 分隔路径。
pub fn walk(
    dir: &Path,
    rel: &str,
    depth: u32,
    max_depth: u32,
    on_file: &mut dyn FnMut(&str, &Path, &std::fs::Metadata),
    on_dir: &mut dyn FnMut(&str, &Path) -> bool,
) {
    if depth > max_depth {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(de.path()) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if meta.is_dir() {
            if on_dir(&child_rel, &de.path()) {
                walk(&de.path(), &child_rel, depth + 1, max_depth, on_file, on_dir);
            }
        } else if meta.is_file() {
            on_file(&child_rel, &de.path(), &meta);
        }
    }
}

/// 递归统计目录大小 / 文件数 / 目录数。
pub fn stat_tree(dir: &Path, depth: u32, total: &mut u64, files: &mut u64, dirs: &mut u64) {
    walk(
        dir,
        "",
        depth,
        64,
        &mut |_rel, _path, meta| {
            *files += 1;
            *total += meta.len();
        },
        &mut |_rel, _path| {
            *dirs += 1;
            true
        },
    );
}

/// 递归收集普通文件为 `(相对路径, 绝对路径)` 列表。
pub fn collect_files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, depth: u32) {
    walk(
        dir,
        rel,
        depth,
        64,
        &mut |child_rel, path, _meta| out.push((child_rel.to_string(), path.to_path_buf())),
        &mut |_rel, _path| true,
    );
}

/// 递归统计条目总数(删除反馈用)。
pub fn count_entries(dir: &Path, n: &mut usize) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for de in rd.flatten() {
            *n += 1;
            if de.path().is_dir() {
                count_entries(&de.path(), n);
            }
        }
    }
}

/// SystemTime → UNIX 秒;取不到返回 0。
pub fn unix_secs(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_and_collect() {
        let base = std::env::temp_dir().join(format!("ndv2_fsx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("a.txt"), b"12345").unwrap();
        std::fs::write(base.join("sub/b.txt"), b"12345678").unwrap();
        std::fs::write(base.join("sub/.hidden"), b"x").unwrap();

        let (mut total, mut files, mut dirs) = (0u64, 0u64, 0u64);
        stat_tree(&base, 0, &mut total, &mut files, &mut dirs);
        assert_eq!((total, files, dirs), (13, 2, 1));

        let mut got = Vec::new();
        collect_files(&base, "", &mut got, 0);
        let mut names: Vec<&str> = got.iter().map(|(r, _)| r.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "sub/b.txt"]);

        let mut n = 0usize;
        count_entries(&base, &mut n);
        assert!(n >= 4); // a.txt, sub, sub/b.txt, sub/.hidden
        let _ = std::fs::remove_dir_all(&base);
    }
}
