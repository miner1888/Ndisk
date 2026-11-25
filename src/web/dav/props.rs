//! WebDAV XML 属性组装与 HTTP-date 格式化(无第三方 XML / 时间依赖)。

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::app::now_secs;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// href 每个字符都百分号编码(与历史实现一致,客户端兼容性最好)。
pub fn href_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// WebDAV 资源 href:`/dav/<share>[/rel]`;共享根目录以 `/` 结尾。
pub fn href_of(share_name: &str, rel: &str) -> String {
    let mut h = format!("/dav/{}", href_encode(share_name));
    if !rel.is_empty() {
        h.push('/');
        h.push_str(&href_encode(rel));
    }
    if rel.is_empty() {
        h.push('/');
    }
    h
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// 自实现 RFC 7231 HTTP-date(1970-01-01 为周四),避免额外依赖。
pub fn http_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let weekday = ((days % 7 + 11) % 7) as usize;
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if d < dy {
            break;
        }
        d -= dy;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1usize;
    for md in mdays {
        if d < md {
            break;
        }
        d -= md;
        m += 1;
    }
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday],
        d + 1,
        MONTHS[m - 1],
        y,
        hh,
        mm,
        ss
    )
}

/// 一个 PROPFIND 条目(resourcetype / displayname / size / mtime / etag / 锁声明)。
pub fn prop_entry(href: &str, display: &str, is_dir: bool, size: u64, mtime: i64) -> String {
    let mtime = if mtime <= 0 { now_secs() } else { mtime };
    let rt = if is_dir {
        "<D:resourcetype><D:collection/></D:resourcetype>"
    } else {
        "<D:resourcetype/>"
    };
    let size_prop = if is_dir {
        String::new()
    } else {
        format!("<D:getcontentlength>{size}</D:getcontentlength>")
    };
    format!(
        "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{rt}\
         <D:displayname>{display}</D:displayname>{size_prop}\
         <D:getlastmodified>{}</D:getlastmodified>\
         <D:getetag>\"{}-{mtime}\"</D:getetag><D:supportedlock>\
         <D:lockentry><D:lockscope><D:exclusive/></D:lockscope>\
         <D:locktype><D:write/></D:locktype></D:lockentry></D:supportedlock>\
         </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        http_date(mtime),
        size,
        display = esc(display),
        href = esc(href),
    )
}

pub fn wrap_multistatus(entries: Vec<String>) -> Response {
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">{}</D:multistatus>",
            entries.join("")
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_date() {
        // 2000-01-01 00:00:00 UTC 是周六。
        assert_eq!(http_date(946684800), "Sat, 01 Jan 2000 00:00:00 GMT");
        // epoch 是周四。
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }

    #[test]
    fn href_layout() {
        assert_eq!(href_of("下载", ""), "/dav/%E4%B8%8B%E8%BD%BD/");
        assert_eq!(href_of("下载", "a b"), "/dav/%E4%B8%8B%E8%BD%BD/a%20b");
    }
}
