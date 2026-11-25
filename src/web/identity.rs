//! 请求身份提取:Bearer 头优先,其次 `ndisk_token` Cookie。
//! 只负责"从请求里取 token → 查会话 → 得到可选用户",纯逻辑在 domain::auth。

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::app::App;
use crate::db::User;
use crate::error::ApiError;

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub user: Option<User>,
    pub token: Option<String>,
}

impl Identity {
    pub fn id(&self) -> i64 {
        self.user.as_ref().map(|u| u.id).unwrap_or(-1)
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.is_admin)
    }

    pub fn as_user(&self) -> Option<&User> {
        self.user.as_ref()
    }

    pub fn require(&self) -> Result<&User, ApiError> {
        self.user.as_ref().ok_or_else(|| ApiError::unauthorized("需要登录"))
    }

    pub fn require_admin(&self) -> Result<&User, ApiError> {
        let u = self.require()?;
        if !u.is_admin {
            return Err(ApiError::forbidden("需要管理员权限"));
        }
        Ok(u)
    }
}

fn bearer_or_cookie(parts: &Parts) -> Option<String> {
    let header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    header.or_else(|| {
        parts
            .headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|c| {
                c.split(';').find_map(|kv| {
                    let kv = kv.trim();
                    kv.strip_prefix("ndisk_token=").map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
                })
            })
    })
}

impl FromRequestParts<App> for Identity {
    type Rejection = axum::response::Response;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer_or_cookie(parts) else {
            return Ok(Identity::default());
        };
        match app.db.user_by_token(&token).await {
            Ok(Some(u)) => Ok(Identity { user: Some(u), token: Some(token) }),
            _ => Ok(Identity::default()),
        }
    }
}

/// 在响应上写出会话 Cookie(登录时 `ttl>0`,登出时 `ttl=0` 清除)。
pub fn session_cookie(resp: &mut axum::response::Response, token: &str, ttl: i64) {
    let cookie = if ttl > 0 {
        format!("ndisk_token={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ttl}")
    } else {
        "ndisk_token=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string()
    };
    if let Ok(v) = axum::http::header::HeaderValue::from_str(&cookie) {
        resp.headers_mut().append(axum::http::header::SET_COOKIE, v);
    }
}
