//! 带磁盘缓存与 TTL 的远程资源订阅(tracker 列表、eD2K server.met、KAD nodes.dat)。
//!
//! 统一一份实现替代三处各写一遍的"缓存文件 + 24 小时过期"逻辑:
//! `read_fresh(path, ttl)` 命中则直接用;未命中由调用方拉取后
//! `write_cache` 落盘,下次 24h 内不再联网。

use std::path::{Path, PathBuf};
use std::time::Duration;

/// 缓存文件在 TTL 内存在且可读时返回内容。
pub fn read_fresh(path: &Path, ttl: Duration) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    let age = meta.modified().ok()?.elapsed().ok()?;
    if age >= ttl {
        return None;
    }
    std::fs::read(path).ok()
}

pub fn write_cache(path: &Path, data: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, data);
}

/// 与 `read_fresh` 相同,但供字符串缓存使用(按行解析由调用方负责)。
pub fn read_fresh_text(path: &Path, ttl: Duration) -> Option<String> {
    read_fresh(path, ttl).and_then(|b| String::from_utf8(b).ok())
}

/// 数据目录根下的缓存文件路径约定:`data/<name>`(与 tmp 目录平级,
/// 随数据目录一起部署与清理)。
pub fn cache_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ndv2_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("cache.txt");
        assert!(read_fresh_text(&p, Duration::from_secs(3600)).is_none());
        write_cache(&p, b"hello");
        assert_eq!(read_fresh_text(&p, Duration::from_secs(3600)).as_deref(), Some("hello"));
        // 读取原始字节接口同样命中。
        assert_eq!(read_fresh(&p, Duration::from_secs(3600)).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
