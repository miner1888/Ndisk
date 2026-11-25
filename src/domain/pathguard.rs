//! 相对路径规范化、文件名校验、防符号链接穿越的安全拼接。

use std::path::{Path, PathBuf};

/// 规范化相对路径:去掉空段与 `.`,拒绝 `..`。
pub fn normalize(rel: &str) -> Result<String, String> {
    let mut out = Vec::new();
    for seg in rel.split('/') {
        let seg = seg.trim();
        match seg {
            "" | "." => {}
            ".." => return Err("非法路径".into()),
            _ => out.push(seg.to_string()),
        }
    }
    Ok(out.join("/"))
}

/// 校验单个文件名(新建 / 重命名 / 上传落盘共用)。
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return Err("非法文件名(不能以 . 开头或为空)".into());
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("文件名不能包含路径分隔符".into());
    }
    if name.contains([':', '*', '?', '"', '<', '>', '|']) {
        return Err("文件名包含非法字符".into());
    }
    if name.chars().count() > 255 {
        return Err("文件名过长".into());
    }
    if cfg!(windows) && reserved_on_windows(name) {
        return Err("该名称为 Windows 保留设备名".into());
    }
    Ok(name.to_string())
}

#[cfg(windows)]
fn reserved_on_windows(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    matches!(name.chars().last(), Some('.') | Some(' '))
}

#[cfg(not(windows))]
fn reserved_on_windows(_name: &str) -> bool {
    false
}

/// 把相对路径安全拼到根目录下:逐段 canonicalize,一旦真实路径逃出根即拒绝
/// (防符号链接把路径引到共享根之外)。
pub fn join_under(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| format!("根目录不可用: {e}"))?;
    let mut cur = root.clone();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        if seg == ".." {
            return Err("路径越界".into());
        }
        cur.push(seg);
        if let Ok(real) = cur.canonicalize() {
            if !real.starts_with(&root) {
                return Err("路径越界".into());
            }
            cur = real;
        }
    }
    Ok(cur)
}

pub fn ext_of(rel: &str) -> String {
    Path::new(rel)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// 去掉 Windows 扩展长度前缀(`\\?\` / `\\?\UNC\`),便于日志与前端展示。
pub fn pretty(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        let t = s.trim_start_matches(r"\\?\UNC\");
        if t.len() != s.len() {
            return format!(r"\\{t}");
        }
        s.trim_start_matches(r"\\?\").to_string()
    } else {
        s.to_string()
    }
}

/// Windows 盘符列表(服务器目录选择器用);其它平台为空。
pub fn list_drives() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    (b'A'..=b'Z')
        .map(|b| (b as char).to_string())
        .filter(|l| PathBuf::from(format!("{l}:\\")).is_dir())
        .map(|l| format!("{l}:"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_traversal() {
        assert_eq!(normalize("a//b/./c/").unwrap(), "a/b/c");
        assert!(normalize("../x").is_err());
        assert!(normalize("a/../../b").is_err());
        assert_eq!(normalize("").unwrap(), "");
    }

    #[test]
    fn check_name_rules() {
        assert!(check_name("ok.txt").is_ok());
        assert!(check_name(".env").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name("a:b").is_err());
        assert!(check_name("  spaced  ").unwrap() == "spaced");
    }

    #[test]
    fn join_under_contains() {
        let base = std::env::temp_dir().join(format!("ndv2_path_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        let got = join_under(&base, "a/b").unwrap();
        assert!(got.ends_with("a/b"));
        assert!(join_under(&base, "../x").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ext_is_lowercased() {
        assert_eq!(ext_of("a/b.TXT"), "txt");
        assert_eq!(ext_of("noext"), "");
    }
}
