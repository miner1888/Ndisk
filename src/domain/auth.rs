use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiError;

/// 密码使用 Argon2id 加盐哈希;返回 PHC 字符串入库。
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// 新建 / 重置账号的统一口令策略:用户名 2-32 字符、密码至少 6 位。
pub fn validate_credentials(name: &str, password: &str) -> Result<(), ApiError> {
    let n = name.trim().chars().count();
    if !(2..=32).contains(&n) {
        return Err(ApiError::bad_request("用户名长度需在 2-32 个字符之间"));
    }
    if password.chars().count() < 6 {
        return Err(ApiError::bad_request("密码至少需要 6 位"));
    }
    Ok(())
}

pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut buf);
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn new_upload_token() -> String {
    let mut buf = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut buf);
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// 一次请求的身份:Bearer 头或会话 Cookie(user 为 None 即游客)。
#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub user: Option<crate::db::User>,
    pub token: Option<String>,
}

impl Identity {
    pub fn id(&self) -> i64 {
        self.user.as_ref().map(|u| u.id).unwrap_or(-1)
    }
    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.is_admin)
    }
    pub fn require(&self) -> Result<&crate::db::User, ApiError> {
        self.user
            .as_ref()
            .ok_or_else(|| ApiError::unauthorized("需要登录"))
    }
    pub fn require_adminish(&self) -> Result<&crate::db::User, ApiError> {
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
                    kv.strip_prefix("ndisk_token=")
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_string)
                })
            })
    })
}

/// axum 提取器:从请求中恢复登录身份(查库校验会话,顺带清理过期会话)。
/// Router 使用 AppState 作为状态类型,这里直接针对其实现。
impl FromRequestParts<crate::state::AppState> for Identity {
    type Rejection = axum::response::Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &crate::state::AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer_or_cookie(parts) else {
            return Ok(Identity::default());
        };
        match state.db.user_by_token(&token).await {
            Ok(Some(u)) => Ok(Identity {
                user: Some(u),
                token: Some(token),
            }),
            _ => Ok(Identity {
                user: None,
                token: None,
            }),
        }
    }
}

/// 登录 / 登出时写 HttpOnly 会话 Cookie;`ttl <= 0` 表示清除。
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

/// 会话有效期(秒):48 小时。
pub const SESSION_TTL: i64 = 48 * 3600;
