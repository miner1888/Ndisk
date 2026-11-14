use std::io::Cursor;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;

use crate::state::AppState;

/// 把 PEM 证书链 + 私钥解析为 rustls 配置;任一步失败即拒绝。
pub fn build_config(cert_pem: &str, key_pem: &str) -> Result<Arc<rustls::ServerConfig>, String> {
    let certs: Vec<rustls::pki_types::CertificateDer> =
        rustls_pemfile::certs(&mut Cursor::new(cert_pem.as_bytes()))
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

/// HTTPS 管理任务:证书变化时启停监听。notify_one 带许可存储,不会丢唤醒。
pub async fn run(st: AppState) {
    let mut serving: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let enabled = reload(&st).await;
        if enabled && serving.is_none() {
            match tokio::net::TcpListener::bind(&st.tls_listen).await {
                Ok(l) => {
                    *st.tls.port.lock().await = l.local_addr().ok().map(|a| a.port());
                    tracing::info!("HTTPS 已启用: https://{}", st.tls_listen);
                    let st2 = st.clone();
                    serving = Some(tokio::spawn(
                        async move { accept_loop(st2, Arc::new(l)).await },
                    ));
                }
                Err(e) => tracing::warn!("HTTPS 监听 {} 失败: {e}", st.tls_listen),
            }
        } else if !enabled {
            if let Some(h) = serving.take() {
                h.abort();
                *st.tls.port.lock().await = None;
                tracing::info!("HTTPS 已停用");
            }
        }
        st.tls.changed.notified().await;
    }
}

async fn reload(st: &AppState) -> bool {
    let row = match st.db.tls_cert().await {
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
    *st.tls.config.write().await = cfg.clone();
    cfg.is_some()
}

async fn accept_loop(st: AppState, listener: Arc<tokio::net::TcpListener>) {
    loop {
        tokio::select! {
            _ = st.tls.changed.notified() => break,
            res = listener.accept() => {
                match res {
                    Ok((tcp, _)) => {
                        let st = st.clone();
                        tokio::spawn(async move { serve_conn(st, tcp).await });
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

async fn serve_conn(st: AppState, tcp: tokio::net::TcpStream) {
    let cfg = st.tls.config.read().await.clone();
    let Some(cfg) = cfg else { return };
    let tls = match TlsAcceptor::from(cfg).accept(tcp).await {
        Ok(v) => v,
        Err(_) => return,
    };
    // Router 实现 tower::Service(Error=Infallible),用 TowerToHyperService 桥接给 hyper
    let hyper_app = hyper_util::service::TowerToHyperService::new(crate::api::router::build(st));
    let _ = hyper::server::conn::http1::Builder::new()
        .serve_connection(hyper_util::rt::TokioIo::new(tls), hyper_app)
        .await;
}
