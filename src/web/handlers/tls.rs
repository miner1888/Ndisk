//! HTTPS 证书管理(仅管理员):查看 / 上传(内容或路径)/ 删除。

use axum::extract::State;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::App;
use crate::error::{ok, ApiError, ApiResult};
use crate::tls;
use crate::web::identity::Identity;

pub async fn get(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    identity.require()?;
    let cert = app.db.tls_cert().await?;
    let port = *app.tls.port.lock().await;
    Ok(ok(json!({
        "enabled": port.is_some(),
        "port": port,
        "tls_listen": app.tls_listen,
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
    #[serde(default)]
    cert_path: Option<String>,
    #[serde(default)]
    key_path: Option<String>,
}

/// 上传证书:PEM 内容或服务器文件路径二选一。先本地解析校验,通过才落库。
pub async fn put(State(app): State<App>, identity: Identity, Json(req): Json<PutReq>) -> ApiResult<Response> {
    identity.require_admin()?;
    let (cert, key) = match (&req.cert_pem, &req.key_pem) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        _ => match (&req.cert_path, &req.key_path) {
            (Some(cp), Some(kp)) => {
                let read = |p: &str, what: &str| -> ApiResult<String> {
                    std::fs::read_to_string(p).map_err(|e| ApiError::bad_request(format!("读取{what}失败: {e}")))
                };
                (read(cp, "证书文件")?, read(kp, "私钥文件")?)
            }
            _ => return Err(ApiError::bad_request("请提供证书与私钥内容或路径")),
        },
    };
    let domain = req.domain.unwrap_or_default();
    tls::build_config(&cert, &key).map_err(ApiError::bad_request)?;
    app.db.tls_upsert(&domain, &cert, &key).await?;
    app.tls.changed.notify_one();
    Ok(ok(json!({ "ok": true })))
}

pub async fn delete(State(app): State<App>, identity: Identity) -> ApiResult<Response> {
    identity.require_admin()?;
    app.db.tls_delete().await?;
    app.tls.changed.notify_one();
    Ok(ok(json!({ "ok": true })))
}
