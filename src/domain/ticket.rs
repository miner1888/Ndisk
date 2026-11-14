use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::state::AppState;

/// 下载直链签名:HMAC-SHA256(进程级随机密钥),payload = "{kind}|{share}|{rel}|{exp}"。
/// 重启后密钥更换,旧直链自然失效。
pub const LINK_TTL: i64 = 7 * 86400;

pub fn sign(st: &AppState, kind: &str, share_id: i64, rel: &str, exp: i64) -> String {
    let msg = format!("{kind}|{share_id}|{rel}|{exp}");
    hex(&hmac_sha256(&st.dl_key, msg.as_bytes()))
}

pub fn verify(
    st: &AppState,
    kind: &str,
    share_id: i64,
    rel: &str,
    exp: i64,
    sig: &str,
) -> Result<(), ApiError> {
    let now = crate::state::now_secs();
    if exp < now || exp > now + LINK_TTL * 4 {
        return Err(ApiError::forbidden("下载链接已过期,请重新复制"));
    }
    let expect = sign(st, kind, share_id, rel, exp);
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

/// URL 编码:仅保留 RFC 3986 unreserved 字符。
pub fn urlencode(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'.')
        .remove(b'-')
        .remove(b'_')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

/// 相对路径 → URL 路径段(逐段编码,保证链接以真实文件名结尾)。
pub fn path_segments(rel: &str) -> String {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .map(urlencode)
        .collect::<Vec<_>>()
        .join("/")
}

/// 单文件下载直链;`signed=false` 时对游客可见文件使用短链。
pub fn file_link(st: &AppState, share_id: i64, rel: &str, signed: bool) -> String {
    let seg = path_segments(rel);
    if !signed {
        return format!("/dl/{share_id}/{seg}");
    }
    let exp = crate::state::now_secs() + LINK_TTL;
    let sig = sign(st, "file", share_id, rel, exp);
    format!("/dl/{share_id}/{seg}?exp={exp}&sig={sig}")
}

/// 批量打包下载直链:names 逐个编码后以 `,` 连接(服务端按原始串切分再逐个解码)。
pub fn zip_link(st: &AppState, share_id: i64, path: &str, names: &[String]) -> String {
    let exp = crate::state::now_secs() + LINK_TTL;
    let payload = format!("{path}|{}", names.join(","));
    let sig = sign(st, "zip", share_id, &payload, exp);
    let encoded: Vec<String> = names.iter().map(|n| urlencode(n)).collect();
    format!(
        "/api/zip?share_id={share_id}&path={}&names={}&exp={exp}&sig={sig}",
        urlencode(path),
        encoded.join(",")
    )
}
