//! 人类可读格式化。全项目唯一的字节格式化实现(分享页、管理后台、
//! 离线下载上限提示共用)。

/// `1024 → "1.0 KB"`;小于 1024 原样 `"<n> B"`。
pub fn human_bytes(n: i64) -> String {
    let u = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < u.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", u[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1023), "1023.0 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(8 * 1024 * 1024), "8.0 MB");
        assert_eq!(human_bytes(20 * 1024 * 1024 * 1024), "20.0 GB");
    }
}
