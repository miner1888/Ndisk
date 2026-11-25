//! 离线链接解析:迅雷解码、协议归类、内网地址校验、去重命名。

use base64::Engine;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::app::App;

pub async fn allow_private(app: &App) -> bool {
    app.db.setting("offline_allow_private").await.as_deref() != Some("0")
}

/// 迅雷链接(`thunder://base64(AA...ZZ)`)解码。
pub fn decode_thunder(url: &str) -> Option<String> {
    let enc = url.strip_prefix("thunder://")?;
    let raw = base64::engine::general_purpose::STANDARD.decode(enc.trim()).ok()?;
    let s = String::from_utf8(raw).ok()?;
    let body = s.strip_prefix("AA")?.strip_suffix("ZZ")?;
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// http(s) 链接的路径以 .torrent 结尾 → BT 种子直链,交给 BT 引擎拉取解析,
/// 而不是把种子文件本身当作普通文件下载。
fn is_torrent_url(url: &str) -> bool {
    url::Url::parse(url)
        .map(|u| u.path().to_ascii_lowercase().ends_with(".torrent"))
        .unwrap_or(false)
}

/// 返回 `(规范化后的链接, 类型)`,类型 ∈ {http, bt, ed2k}。
pub fn normalize_url(url: &str) -> Result<(String, &'static str), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("链接为空".into());
    }
    if url.strip_prefix("thunder://").is_some() {
        let real = decode_thunder(url).ok_or("迅雷链接解码失败")?;
        let lower = real.to_lowercase();
        if lower.starts_with("magnet:") {
            return Ok((real, "bt"));
        }
        if lower.starts_with("http://") || lower.starts_with("https://") {
            let kind = if is_torrent_url(&real) { "bt" } else { "http" };
            return Ok((real, kind));
        }
        return Err(format!(
            "该迅雷链接指向的协议({})暂不支持",
            lower.split(':').next().unwrap_or("?")
        ));
    }
    let lower = url.to_lowercase();
    if lower.starts_with("ed2k://") {
        return Ok((url.to_string(), "ed2k"));
    }
    if lower.starts_with("magnet:") {
        return Ok((url.to_string(), "bt"));
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok((url.to_string(), if is_torrent_url(url) { "bt" } else { "http" }));
    }
    if lower.starts_with("ftp://") {
        return Err("FTP 链接暂不支持".into());
    }
    Err("无法识别的链接类型".into())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 127
                || o[0] == 10
                || o[0] == 0
                || (o[0] == 172 && (o[1] & 0xF0) == 16)
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || (v6.segments()[0] & 0xFE00) == 0xFC00 || v6.is_unspecified()
        }
    }
}

/// 拒绝非 http(s) 协议与(未开启内网访问时的)内网地址。
pub async fn validate_url(app: &App, url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("链接格式错误: {e}"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("不支持的协议: {scheme}"));
    }
    let Some(host) = parsed.host_str() else {
        return Err("链接缺少主机名".into());
    };
    if !allow_private(app).await {
        let host_for_resolve = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:0")
        } else {
            format!("{host}:0")
        };
        let addrs = tokio::net::lookup_host(&host_for_resolve)
            .await
            .map_err(|e| format!("无法解析主机 {host}: {e}"))?;
        for a in addrs {
            if is_private_ip(a.ip()) {
                return Err("禁止访问内网地址(可在后台系统维护中放开)".into());
            }
        }
    }
    Ok(())
}

/// 目标目录内生成不冲突的文件名:`name`, `name (1)`, `name (2)` ...
pub fn unique_name(dir: &Path, name: &str) -> PathBuf {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let ext = Path::new(name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let mut candidate = dir.join(name);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_links() {
        assert_eq!(normalize_url("https://a/b").unwrap().1, "http");
        assert_eq!(normalize_url("magnet:?xt=urn:btih:x").unwrap().1, "bt");
        assert_eq!(normalize_url("ed2k://|file|a|1|AA|/").unwrap().1, "ed2k");
        assert!(normalize_url("ftp://a/b").is_err());
        assert!(normalize_url("garbage").is_err());
        assert!(normalize_url("").is_err());
    }

    #[test]
    fn thunder_roundtrip() {
        use base64::Engine as _;
        let inner = "AAhttps://example.com/f.binZZ";
        let enc = base64::engine::general_purpose::STANDARD.encode(inner);
        let url = format!("thunder://{enc}");
        assert_eq!(decode_thunder(&url).unwrap(), "https://example.com/f.bin");
        assert!(decode_thunder("thunder://!!!").is_none());
    }

    #[test]
    fn torrent_url_recognition() {
        // .torrent 直链(含大写扩展名与查询串)归为 BT;普通文件仍是 http。
        assert_eq!(normalize_url("https://a.org/x.torrent").unwrap(), ("https://a.org/x.torrent".into(), "bt"));
        assert_eq!(normalize_url("http://a.org/x.TORRENT?k=v").unwrap().1, "bt");
        assert_eq!(normalize_url("https://a.org/x.torrent2").unwrap().1, "http");
        assert_eq!(normalize_url("https://a.org/dir/x.bin").unwrap().1, "http");
        // 迅雷包着的种子直链同样识别。
        use base64::Engine as _;
        let enc = base64::engine::general_purpose::STANDARD.encode("AAhttps://a.org/x.torrentZZ");
        assert_eq!(normalize_url(&format!("thunder://{enc}")).unwrap().1, "bt");
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("ndv2_uniq_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), b"").unwrap();
        assert!(unique_name(&dir, "f.txt").ends_with("f (1).txt"));
        assert!(unique_name(&dir, "g.txt").ends_with("g.txt"));
        assert!(unique_name(&dir, "noext").ends_with("noext"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
