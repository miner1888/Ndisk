use axum::extract::State;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::domain::auth::Identity;
use crate::error::{ok, ApiError, ApiResult};
use crate::state::AppState;

pub async fn get(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    identity.require()?;
    let cert = st.db.tls_cert().await?;
    let port = *st.tls.port.lock().await;
    Ok(ok(json!({
        "enabled": port.is_some(),
        "port": port,
        "tls_listen": st.tls_listen,
        "cert": cert.map(|c| json!({ "domain": c.domain })),
    })))
}

#[derive(Deserialize)]
pub struct PutReq {
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    cert_pem: Option<String>,
    #[serde(default)]
    key_pem: Option<String>,
    /// 从服务器文件读取证书(路径模式)
    #[serde(default)]
    cert_path: Option<String>,
    #[serde(default)]
    key_path: Option<String>,
}

pub async fn put(
    State(st): State<AppState>,
    identity: Identity,
    Json(req): Json<PutReq>,
) -> ApiResult<Response> {
    identity.require_adminish()?;
    let (cert, key) = match (&req.cert_pem, &req.key_pem) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        _ => match (&req.cert_path, &req.key_path) {
            (Some(cp), Some(kp)) => {
                let read = |p: &str, what: &str| -> ApiResult<String> {
                    std::fs::read_to_string(p)
                        .map_err(|e| ApiError::bad_request(format!("读取{what}失败: {e}")))
                };
                (read(cp, "证书文件")?, read(kp, "私钥文件")?)
            }
            _ => return Err(ApiError::bad_request("请提供证书与私钥内容或路径")),
        },
    };
    let domain = req.domain.unwrap_or_default();
    // 先验证可用再入库,失败即拒绝
    crate::tlsrun::build_config(&cert, &key).map_err(ApiError::bad_request)?;
    st.db.tls_upsert(&domain, &cert, &key).await?;
    // 通知 TLS 管理任务重载(notify_one 带许可存储,不会丢唤醒)
    st.tls.changed.notify_one();
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(State(st): State<AppState>, identity: Identity) -> ApiResult<Response> {
    identity.require_adminish()?;
    st.db.tls_delete().await?;
    st.tls.changed.notify_one();
    Ok(ok(json!({ "ok": true })))
}
