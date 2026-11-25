//! 命令行解析与运行时配置。
//!
//! ⚠ 兼容性不变量:参数集合、取值形式(短参数紧贴 `-p8080`、长参数 `--port=8080`)
//! 与默认值必须与历史版本完全一致。

use std::path::PathBuf;

pub const DEFAULT_CHUNK: i64 = 8 * 1024 * 1024;
pub const DEFAULT_HOST: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 9000;
pub const DEFAULT_TLS_PORT: u16 = 443;

const USAGE: &str = "\
NDisk — 多用户网盘

用法: ndisk [选项]
  短参数可以紧贴取值(-p8080)或分开写(-p 8080);长参数同样支持 --port=8080

监听
  -l, --listen <addr>     监听地址,可写 host:port 或只写端口 (默认 0.0.0.0:9000)
  -p, --port <port>       监听端口 (默认 9000)
  -H, --host <addr>       监听网卡地址 (默认 0.0.0.0;只允许本机访问用 127.0.0.1)
  -t, --tls-listen <addr> HTTPS 监听地址 (默认 0.0.0.0:443,配置证书后启用)
      --tls-port <port>   HTTPS 端口 (默认 443)

其它
  -d, --data <dir>        数据目录 (默认 ./data)
  -u, --admin-user <name> 初始管理员用户名 (默认 admin)
  -w, --admin-pass <pw>   初始管理员密码 (默认 admin123)
  -c, --chunk-size <size> 上传分片大小,支持 8M / 8MB / 8388608 (默认 8M)
  -h, --help              显示帮助
  -V, --version           显示版本

示例
  ndisk -p 18080                    换端口启动
  ndisk -l 127.0.0.1:8080 -d D:\\data  只监听本机,数据放 D:\\data
";

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: String,
    pub tls_listen: String,
    pub data_dir: PathBuf,
    pub admin_user: String,
    pub admin_pass: String,
    pub chunk_size: i64,
    /// 是否显式指定了监听端口(决定端口被占时是报错还是自动后移)。
    pub listen_explicit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            listen: format!("{DEFAULT_HOST}:{DEFAULT_PORT}"),
            tls_listen: format!("{DEFAULT_HOST}:{DEFAULT_TLS_PORT}"),
            data_dir: PathBuf::from("./data"),
            admin_user: "admin".into(),
            admin_pass: "admin123".into(),
            chunk_size: DEFAULT_CHUNK,
            listen_explicit: false,
        }
    }
}

fn fail(what: &str) -> Option<Config> {
    eprintln!("错误: {what}\n\n{USAGE}");
    None
}

/// `host:port` / `port` / `[v6]:port` → (可选 host, port)。
pub fn split_addr(v: &str) -> Result<(Option<String>, u16), String> {
    let v = v.trim();
    if v.is_empty() {
        return Err("监听地址不能为空".into());
    }
    if let Ok(p) = v.parse::<u16>() {
        return Ok((None, p));
    }
    let (host, port) = match v.rsplit_once(':') {
        Some((h, p)) => (h.trim_matches(['[', ']']).to_string(), p),
        None => return Err(format!("无法识别的监听地址: {v}(应形如 0.0.0.0:9000 或 9000)")),
    };
    let port: u16 = port.trim().parse().map_err(|_| format!("端口非法: {v}"))?;
    Ok((if host.is_empty() { None } else { Some(host) }, port))
}

pub fn parse_port(v: &str) -> Result<u16, String> {
    let p: u32 = v.trim().parse().map_err(|_| format!("端口非法: {v}"))?;
    if p == 0 || p > 65535 {
        return Err(format!("端口需在 1-65535 之间: {v}"));
    }
    Ok(p as u16)
}

/// `8M` / `8MB` / `8388608` → 字节数。
pub fn parse_size(v: &str) -> Result<i64, String> {
    let s = v.trim().to_ascii_lowercase();
    let (num, mul) = if let Some(n) = s.strip_suffix("kb").or_else(|| s.strip_suffix('k')) {
        (n, 1024i64)
    } else if let Some(n) = s.strip_suffix("mb").or_else(|| s.strip_suffix('m')) {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("gb").or_else(|| s.strip_suffix('g')) {
        (n, 1024 * 1024 * 1024)
    } else {
        (s.as_str(), 1)
    };
    let n: i64 = num.trim().parse().map_err(|_| format!("分片大小非法: {v}"))?;
    if n < 0 {
        return Err(format!("分片大小不能为负: {v}"));
    }
    Ok(n.saturating_mul(mul))
}

fn value_of(args: &[String], i: &mut usize, inline: Option<String>, flag: &str) -> Result<String, String> {
    if let Some(v) = inline {
        if !v.is_empty() {
            return Ok(v);
        }
        return Err(format!("{flag} 缺少参数值"));
    }
    if *i < args.len() {
        let v = args[*i].clone();
        *i += 1;
        Ok(v)
    } else {
        Err(format!("{flag} 缺少参数值"))
    }
}

const SHORT_WITH_VALUE: [char; 8] = ['l', 'p', 'H', 't', 'd', 'u', 'w', 'c'];

pub fn parse_args() -> Option<Config> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = Config::default();
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    let mut tls_host = DEFAULT_HOST.to_string();
    let mut tls_port = DEFAULT_TLS_PORT;
    let mut i = 0;
    while i < args.len() {
        let raw = args[i].clone();
        i += 1;
        let (key, inline) = match raw.split_once('=') {
            Some((k, v)) if k.starts_with('-') => (k.to_string(), Some(v.to_string())),
            _ => (raw.clone(), None),
        };
        // 短参数紧贴取值:-p8080 → ("-p", "8080")
        let (key, inline) = if inline.is_none()
            && key.starts_with('-')
            && !key.starts_with("--")
            && key.chars().count() > 2
        {
            match SHORT_WITH_VALUE.iter().find(|c| key.starts_with(&format!("-{c}"))) {
                Some(c) => (format!("-{c}"), Some(key[2..].to_string())),
                None => (key, inline),
            }
        } else {
            (key, inline)
        };
        macro_rules! value {
            () => {
                match value_of(&args, &mut i, inline.clone(), &key) {
                    Ok(v) => v,
                    Err(e) => return fail(&e),
                }
            };
        }
        macro_rules! bad {
            ($e:expr) => {
                return fail(&$e)
            };
        }
        match key.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return None;
            }
            "-V" | "--version" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return None;
            }
            "-l" | "--listen" => match split_addr(&value!()) {
                Ok((h, p)) => {
                    if let Some(h) = h {
                        host = h;
                    }
                    port = p;
                    cfg.listen_explicit = true;
                }
                Err(e) => bad!(e),
            },
            "-p" | "--port" => match parse_port(&value!()) {
                Ok(p) => {
                    port = p;
                    cfg.listen_explicit = true;
                }
                Err(e) => bad!(e),
            },
            "-H" | "--host" => host = value!(),
            "-t" | "--tls-listen" => match split_addr(&value!()) {
                Ok((h, p)) => {
                    if let Some(h) = h {
                        tls_host = h;
                    }
                    tls_port = p;
                }
                Err(e) => bad!(e),
            },
            "--tls-port" => match parse_port(&value!()) {
                Ok(p) => tls_port = p,
                Err(e) => bad!(e),
            },
            "-d" | "--data" => cfg.data_dir = PathBuf::from(value!()),
            "-u" | "--admin-user" => cfg.admin_user = value!(),
            "-w" | "--admin-pass" => cfg.admin_pass = value!(),
            "-c" | "--chunk-size" => match parse_size(&value!()) {
                Ok(n) => cfg.chunk_size = n,
                Err(e) => bad!(e),
            },
            other => return fail(&format!("未知参数: {other}(用 --help 查看用法)")),
        }
    }
    if cfg.admin_user.trim().is_empty() {
        return fail("--admin-user 不能为空");
    }
    cfg.listen = format!("{host}:{port}");
    cfg.tls_listen = format!("{tls_host}:{tls_port}");
    Some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_forms() {
        assert_eq!(split_addr("9000").unwrap(), (None, 9000));
        assert_eq!(split_addr("127.0.0.1:8080").unwrap(), (Some("127.0.0.1".into()), 8080));
        assert_eq!(split_addr(":7000").unwrap(), (None, 7000));
        assert_eq!(split_addr("[::1]:7000").unwrap(), (Some("::1".into()), 7000));
        assert!(split_addr("no-port").is_err());
    }

    #[test]
    fn size_suffix() {
        assert_eq!(parse_size("8388608").unwrap(), 8388608);
        assert_eq!(parse_size("8M").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_size("16mb").unwrap(), 16 * 1024 * 1024);
        assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("0").unwrap(), 0);
        assert!(parse_size("-1").is_err());
        assert!(parse_size("8x").is_err());
    }

    #[test]
    fn port_range() {
        assert_eq!(parse_port("1").unwrap(), 1);
        assert!(parse_port("0").is_err());
        assert!(parse_port("70000").is_err());
    }
}
