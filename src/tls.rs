//! HTTPS 运行时:监听守护任务,证书变化时热重建,不影响 HTTP 服务。
//!
//! rustls 0.23 要求显式选定加密后端:本项目的依赖树里 `ring`(reqwest 的
//! rustls-tls)与 `aws-lc-rs`(librqbit 引入)同时启用,自动探测会失败并在
//! 首次 TLS 握手时 panic。必须在进程启动时显式安装 provider。

use std::io::Cursor;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;

use crate::app::App;

/// 幂等安装 `ring` provider(main 与 build_config 都会调用)。
pub fn init_crypto() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // 已安装其它 provider 时返回 Err,忽略即可。
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn build_config(cert_pem: &str, key_pem: &str) -> Result<Arc<rustls::ServerConfig>, String> {
    init_crypto();
    let certs: Vec<rustls::pki_types::CertificateDer> = rustls_pemfile::certs(&mut Cursor::new(cert_pem.as_bytes()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("证书解析失败: {e}"))?;
    if certs.is_empty() {
        return Err("证书内容中没有找到 PEM 证书".into());
    }
    let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem.as_bytes()))
        .map_err(|e| format!("私钥解析失败: {e}"))?
        .ok_or_else(|| "私钥内容中没有找到 PEM 私钥".to_string())?;
    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map(Arc::new)
        .map_err(|e| format!("证书与私钥不匹配: {e}"))
}

/// HTTPS 守护循环:每次被 notify 后读库取证书;有证且未在服务 → 起监听,
/// 无证且在服务 → 停监听。
pub async fn run(app: App) {
    let mut serving: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let enabled = reload(&app).await;
        if enabled && serving.is_none() {
            match tokio::net::TcpListener::bind(&app.tls_listen).await {
                Ok(l) => {
                    *app.tls.port.lock().await = l.local_addr().ok().map(|a| a.port());
                    tracing::info!("HTTPS 已启用: https://{}", app.tls_listen);
                    let app2 = app.clone();
                    serving = Some(tokio::spawn(async move { accept_loop(app2, Arc::new(l)).await }));
                }
                Err(e) => tracing::warn!("HTTPS 监听 {} 失败: {e}", app.tls_listen),
            }
        } else if !enabled {
            if let Some(h) = serving.take() {
                h.abort();
                *app.tls.port.lock().await = None;
                tracing::info!("HTTPS 已停用");
            }
        }
        app.tls.changed.notified().await;
    }
}

async fn reload(app: &App) -> bool {
    let row = match app.db.tls_cert().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("读取证书失败: {e}");
            None
        }
    };
    let cfg = match &row {
        Some(c) => match build_config(&c.cert_pem, &c.key_pem) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!("证书无效,HTTPS 不可用: {e}");
                None
            }
        },
        None => None,
    };
    *app.tls.config.write().await = cfg.clone();
    cfg.is_some()
}

async fn accept_loop(app: App, listener: Arc<tokio::net::TcpListener>) {
    loop {
        tokio::select! {
            _ = app.tls.changed.notified() => break,
            res = listener.accept() => {
                match res {
                    Ok((tcp, _)) => {
                        let app = app.clone();
                        tokio::spawn(async move { serve_conn(app, tcp).await });
                    }
                    Err(e) => {
                        tracing::debug!("TLS accept 失败: {e}");
                        break;
                    }
                }
            }
        }
    }
}

async fn serve_conn(app: App, tcp: tokio::net::TcpStream) {
    let cfg = app.tls.config.read().await.clone();
    let Some(cfg) = cfg else { return };
    let tls = match TlsAcceptor::from(cfg).accept(tcp).await {
        Ok(v) => v,
        Err(_) => return,
    };
    let svc = hyper_util::service::TowerToHyperService::new(crate::web::router::build(app));
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(tls), svc)
        .await;
}
