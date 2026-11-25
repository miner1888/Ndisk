//! HMAC-SHA256 签名下载票据:免登录直链(/dl/...)与打包下载(/api/zip)。
//!
//! 密钥是进程内随机的 32 字节(不持久化):服务重启后旧票据失效,
//! "直链需重新复制"是既有语义,保持不变。

use sha2::{Digest, Sha256};

use crate::error::ApiError;

/// 票据默认有效期:7 天。
pub const LINK_TTL: i64 = 7 * 86400;

/// 签名密钥(启动时随机生成,由 App 持有)。
pub type Key = [u8; 32];

pub fn sign(key: &Key, kind: &str, share_id: i64, rel: &str, exp: i64) -> String {
    let msg = format!("{kind}|{share_id}|{rel}|{exp}");
    hex(&hmac_sha256(key, msg.as_bytes()))
}

pub fn verify(key: &Key, kind: &str, share_id: i64, rel: &str, exp: i64, sig: &str) -> Result<(), ApiError> {
    let now = crate::domain::now_secs();
    // 签名时 exp 总是 now + LINK_TTL;上限 *4 只是给"服务器与客户端时钟 slight skew"的容差。
    if exp < now || exp > now + LINK_TTL * 4 {
        return Err(ApiError::forbidden("下载链接已过期,请重新复制"));
    }
    let expect = sign(key, kind, share_id, rel, exp);
    if sig.len() == expect.len() && sig.eq_ignore_ascii_case(&expect) {
        Ok(())
    } else {
        Err(ApiError::forbidden("下载链接无效"))
    }
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = key.to_vec();
    if k.len() > BLOCK {
        k = Sha256::digest(&k).to_vec();
    }
    k.resize(BLOCK, 0);
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}

/// URL 路径段编码(保留 `.` `-` `_` `~`)。
pub fn urlencode(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'.')
        .remove(b'-')
        .remove(b'_')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

/// 相对路径按 `/` 分段编码后重组(每段独立编码)。
pub fn path_segments(rel: &str) -> String {
    rel.split('/').filter(|s| !s.is_empty()).map(urlencode).collect::<Vec<_>>().join("/")
}

/// 文件免登录直链。游客可访问的共享无需签名(短链),其余附带签名与有效期。
pub fn file_link(key: &Key, share_id: i64, rel: &str, signed: bool) -> String {
    let seg = path_segments(rel);
    if !signed {
        return format!("/dl/{share_id}/{seg}");
    }
    let exp = crate::domain::now_secs() + LINK_TTL;
    let sig = sign(key, "file", share_id, rel, exp);
    format!("/dl/{share_id}/{seg}?exp={exp}&sig={sig}")
}

/// 多选打包下载直链:签名覆盖 `path|name1,name2`(与 zip GET 校验严格一致)。
pub fn zip_link(key: &Key, share_id: i64, path: &str, names: &[String]) -> String {
    let exp = crate::domain::now_secs() + LINK_TTL;
    let payload = format!("{path}|{}", names.join(","));
    let sig = sign(key, "zip", share_id, &payload, exp);
    let encoded: Vec<String> = names.iter().map(|n| urlencode(n)).collect();
    format!(
        "/api/zip?share_id={share_id}&path={}&names={}&exp={exp}&sig={sig}",
        urlencode(path),
        encoded.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let key = [7u8; 32];
        let exp = crate::domain::now_secs() + 3600;
        let sig = sign(&key, "file", 3, "a/b.txt", exp);
        assert!(verify(&key, "file", 3, "a/b.txt", exp, &sig).is_ok());
        assert!(verify(&key, "file", 3, "a/b.txt", exp, &sig.to_uppercase()).is_ok());
        assert!(verify(&key, "file", 3, "a/b.txt", exp - LINK_TTL * 10, &sig).is_err());
        assert!(verify(&key, "file", 4, "a/b.txt", exp, &sig).is_err());
        assert!(verify(&key, "zip", 3, "a/b.txt", exp, &sig).is_err());
    }

    #[test]
    fn hmac_vectors() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?"
        let k = b"Jefe";
        let m = b"what do ya want for nothing?";
        assert_eq!(hex(&hmac_sha256(k, m)), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn path_segments_encoding() {
        assert_eq!(path_segments("a/b c/d.txt"), "a/b%20c/d.txt");
        assert_eq!(path_segments(""), "");
    }
}
