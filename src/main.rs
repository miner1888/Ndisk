// 全局分配器:mimalloc 会把空闲内存归还操作系统,
// 长时间运行 + 海量小请求场景下,驻留内存远低于 glibc malloc 的碎片水位
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod api;
mod config;
mod db;
mod domain;
mod error;
mod state;
mod tlsrun;

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ndisk=info,tower_http=info")),
        )
        .init();
    let Some(cfg) = config::parse_args() else {
        std::process::exit(2);
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if let Err(e) = rt.block_on(run(cfg)) {
        tracing::error!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

async fn run(cfg: config::Config) -> Result<(), String> {
    let st = state::AppState::boot(&cfg).await?;
    let app = api::router::build(st.clone());
    let (http, addr) = bind_listener(&cfg).await?;
    tracing::info!("NDisk 已启动: http://{addr}");
    if cfg.tls_listen != format!("{}:{}", config::DEFAULT_HOST, config::DEFAULT_TLS_PORT) {
        tracing::info!("HTTPS 监听: https://{}(配置证书后生效)", cfg.tls_listen);
    }
    let tls_st = st.clone();
    let tls_task = tokio::spawn(async move { tlsrun::run(tls_st).await });
    let serve = axum::serve(http, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("收到退出信号");
    });
    if let Err(e) = serve.await {
        tracing::error!("HTTP 服务错误: {e}");
    }
    tls_task.abort();
    Ok(())
}

/// 绑定监听端口。显式指定端口时不自动改端口,只给出带排查建议的错误;
/// 使用默认端口(未显式指定)时若端口不可用,自动向后尝试 20 个端口,
/// 避开 Windows 上常见的系统保留端口段(Hyper-V / WSL / Docker 保留)。
async fn bind_listener(cfg: &config::Config) -> Result<(tokio::net::TcpListener, String), String> {
    match tokio::net::TcpListener::bind(&cfg.listen).await {
        Ok(l) => Ok((l, cfg.listen.clone())),
        Err(e) => {
            if !cfg.listen_explicit {
                let (host, port) = split_host_port(&cfg.listen);
                let end = (port as u32 + 20).min(65535) as u16;
                for p in (port + 1)..=end {
                    let addr = format!("{host}:{p}");
                    if let Ok(l) = tokio::net::TcpListener::bind(&addr).await {
                        tracing::warn!(
                            "端口 {port} 不可用({e}),已自动改用 {p};可用 --port 指定固定端口"
                        );
                        return Ok((l, addr));
                    }
                }
            }
            Err(format!(
                "监听 {} 失败: {e}{}",
                cfg.listen,
                bind_hint(e.raw_os_error())
            ))
        }
    }
}

fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(config::DEFAULT_PORT)),
        None => (config::DEFAULT_HOST.to_string(), config::DEFAULT_PORT),
    }
}

/// Windows 上端口绑定失败(尤其 10013)最常见的两类原因和对应处理
fn bind_hint(code: Option<i32>) -> &'static str {
    match code {
        Some(10013) => {
            "\n可能原因与处理(Windows 错误 10013 = 访问被拒绝):\
             \n  1) 端口落在系统保留段(Hyper-V / WSL / Docker 会保留端口):\
             \n     以管理员身份执行  netsh interface ipv4 show excludedportrange protocol=tcp  查看保留区间,然后换端口启动,例如:ndisk -p 18080\
             \n  2) 端口已被其它程序占用:  netstat -ano | findstr :9000  找到 PID 后结束对应程序\
             \n  3) 也可以只监听本机:  ndisk -H 127.0.0.1 -p 18080\
             \n  4) 临时释放保留端口(需管理员,重启后失效):  net stop winnat && net start winnat"
        }
        Some(10048) => "\n端口已被占用:换端口启动(ndisk -p 18080)或用 netstat -ano | findstr :9000 找到占用进程。",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_addr_roundtrip() {
        assert_eq!(split_host_port("0.0.0.0:9000"), ("0.0.0.0".into(), 9000));
        assert_eq!(
            split_host_port("127.0.0.1:19100"),
            ("127.0.0.1".into(), 19100)
        );
        assert_eq!(split_host_port("bad"), ("0.0.0.0".into(), 9000));
    }

    #[test]
    fn windows_bind_hint_mentions_fixes() {
        let h = bind_hint(Some(10013));
        assert!(h.contains("excludedportrange"), "{h}");
        assert!(h.contains("--port") || h.contains("-p "), "{h}");
        assert!(bind_hint(None).is_empty());
    }
}
