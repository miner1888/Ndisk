//! 二进制入口:全局分配器 → 日志 → 参数解析 → 启动 HTTP(与可选 HTTPS)。

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tracing_subscriber::EnvFilter;

fn main() {
    // 先安装 rustls 加密后端(依赖树里 ring / aws-lc-rs 同时存在,必须显式选定)。
    ndisk::tls::init_crypto();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("ndisk=info,tower_http=info")),
        )
        .init();
    let Some(cfg) = ndisk::cli::parse_args() else {
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

async fn run(cfg: ndisk::cli::Config) -> Result<(), String> {
    use ndisk::{app::App, cli, tls};
    let app = App::boot(&cfg).await?;
    let router = ndisk::web::router::build(app.clone());
    let (http, addr) = bind_listener(&cfg).await?;
    tracing::info!("NDisk v{} 已启动: http://{addr}", env!("CARGO_PKG_VERSION"));
    if cfg.tls_listen != format!("{}:{}", cli::DEFAULT_HOST, cli::DEFAULT_TLS_PORT) {
        tracing::info!("HTTPS 监听: https://{}(配置证书后生效)", cfg.tls_listen);
    }
    let tls_app = app.clone();
    let tls_task = tokio::spawn(async move {
        use futures_util::FutureExt;
        // HTTPS 守护任务 panic 不应影响 HTTP 服务;记录后由运维决定是否重启。
        if std::panic::AssertUnwindSafe(tls::run(tls_app)).catch_unwind().await.is_err() {
            tracing::error!("HTTPS 守护任务异常(panic):HTTP 服务不受影响,证书热更新需重启恢复");
        }
    });
    let serve = axum::serve(http, router).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("收到退出信号");
    });
    if let Err(e) = serve.await {
        tracing::error!("HTTP 服务错误: {e}");
    }
    tls_task.abort();
    Ok(())
}

/// 绑定监听。未显式指定端口且默认端口被占时,自动向后找一个可用端口;
/// 显式指定则直接报错并给出排查建议。
async fn bind_listener(cfg: &ndisk::cli::Config) -> Result<(tokio::net::TcpListener, String), String> {
    match tokio::net::TcpListener::bind(&cfg.listen).await {
        Ok(l) => Ok((l, cfg.listen.clone())),
        Err(e) => {
            if !cfg.listen_explicit {
                let (host, port) = split_host_port(&cfg.listen);
                let end = (port as u32 + 20).min(65535) as u16;
                for p in (port + 1)..=end {
                    let addr = format!("{host}:{p}");
                    if let Ok(l) = tokio::net::TcpListener::bind(&addr).await {
                        tracing::warn!("端口 {port} 不可用({e}),已自动改用 {p};可用 --port 指定固定端口");
                        return Ok((l, addr));
                    }
                }
            }
            Err(format!("监听 {} 失败: {e}{}", cfg.listen, bind_hint(e.raw_os_error())))
        }
    }
}

fn split_host_port(addr: &str) -> (String, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(ndisk::cli::DEFAULT_PORT)),
        None => (
            ndisk::cli::DEFAULT_HOST.to_string(),
            ndisk::cli::DEFAULT_PORT,
        ),
    }
}

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
        assert_eq!(split_host_port("127.0.0.1:19100"), ("127.0.0.1".into(), 19100));
        assert_eq!(split_host_port("bad"), ("0.0.0.0".into(), 9000));
    }
}
