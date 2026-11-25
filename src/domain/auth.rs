//! 认证原语:argon2 口令哈希、随机 token、凭据强度校验。
//! 纯逻辑,不依赖任何框架类型。

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;

/// 会话有效期:48 小时。
pub const SESSION_TTL: i64 = 48 * 3600;

pub fn hash_password(password: &str) -> Result<String, String> {
    // argon2 0.6:hash_password 内部自动生成 16 字节随机盐(getrandom),返回 PHC 字符串。
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    // PasswordVerifier<str>:直接以 PHC 字符串校验,串格式非法即 Err(false)。
    Argon2::default().verify_password(password.as_bytes(), hash).is_ok()
}

/// 用户名 2-32 字符、密码至少 6 位(创建用户与修改密码共用)。
pub fn validate_credentials(name: &str, password: &str) -> Result<(), crate::error::ApiError> {
    let n = name.trim().chars().count();
    if !(2..=32).contains(&n) {
        return Err(crate::error::ApiError::bad_request("用户名长度需在 2-32 个字符之间"));
    }
    if password.chars().count() < 6 {
        return Err(crate::error::ApiError::bad_request("密码至少需要 6 位"));
    }
    Ok(())
}

/// 32 字节随机 URL-safe token(会话 / 验证码 / 分享链接)。
pub fn new_token() -> String {
    random_b64(32)
}

/// 16 字节随机 URL-safe token(上传会话)。
pub fn new_upload_token() -> String {
    random_b64(16)
}

fn random_b64(n: usize) -> String {
    use base64::Engine;
    let mut buf = vec![0u8; n];
    rand::fill(&mut buf[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let h = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[test]
    fn token_is_urlsafe_and_unique() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert!(!a.contains('+') && !a.contains('/') && !a.contains('='));
        assert_eq!(new_upload_token().len(), 22); // 16 字节 → 22 个 base64url 字符
    }

    #[test]
    fn credential_rules() {
        use crate::error::ApiError;
        assert!(validate_credentials("alice", "123456").is_ok());
        assert!(matches!(
            validate_credentials("a", "123456"),
            Err(ApiError { .. })
        ));
        assert!(matches!(
            validate_credentials("alice", "12345"),
            Err(ApiError { .. })
        ));
    }
}
