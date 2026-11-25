//! 文件类型判定:MIME、媒体大类、可编辑文本 / 表格、内联预览范围。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Other,
}

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "avif", "tif", "tiff", "ico", "jfif", "heic",
    "heif",
];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "webm", "ogv", "mov", "m4v", "avi", "mkv", "flv", "wmv", "mpg", "mpeg", "ts", "3gp",
    "3g2", "ogm", "m2ts", "mts", "vob", "asf", "rmvb", "f4v",
];
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", "wma", "amr"];

pub fn kind_of(ext: &str) -> Kind {
    if IMAGE_EXTS.contains(&ext) {
        Kind::Image
    } else if VIDEO_EXTS.contains(&ext) {
        Kind::Video
    } else if AUDIO_EXTS.contains(&ext) {
        Kind::Audio
    } else {
        Kind::Other
    }
}

/// 浏览器可原生播放、无需转码的视频容器。
pub fn native_video(ext: &str) -> bool {
    matches!(ext, "mp4" | "m4v" | "webm" | "ogv" | "mov")
}

pub fn editable_text(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "log"
            | "md"
            | "markdown"
            | "yaml"
            | "yml"
            | "conf"
            | "ini"
            | "toml"
            | "json"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "js"
            | "mjs"
            | "ts"
            | "sh"
            | "bash"
            | "py"
            | "sql"
            | "properties"
            | "env"
    )
}

/// 支持"写回 xlsx/xls/ods 二进制"的表格类型。
pub fn is_sheet(ext: &str) -> bool {
    matches!(ext, "xlsx" | "xlsm" | "xls" | "ods")
}

pub fn preview_inline(ext: &str) -> bool {
    !matches!(kind_of(ext), Kind::Other)
        || editable_text(ext)
        || is_sheet(ext)
        || matches!(ext, "pdf" | "docx" | "docm" | "pptx" | "doc" | "odt" | "ods" | "odp" | "rtf")
}

pub fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Other => "other",
    }
}

pub fn mime_of(ext: &str) -> String {
    match ext {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "flv" => "video/x-flv",
        "wmv" => "video/x-ms-wmv",
        "mpg" | "mpeg" => "video/mpeg",
        "ts" | "m2ts" | "mts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "opus" => "audio/opus",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" | "yaml" | "yml" | "conf" | "ini" | "toml" | "csv" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("jpg"), Kind::Image);
        assert_eq!(kind_of("mkv"), Kind::Video);
        assert_eq!(kind_of("flac"), Kind::Audio);
        assert_eq!(kind_of("zip"), Kind::Other);
        assert!(native_video("mp4"));
        assert!(!native_video("mkv"));
        assert!(editable_text("json"));
        assert!(preview_inline("pdf"));
        assert!(!preview_inline("exe"));
    }

    #[test]
    fn mimes() {
        assert_eq!(mime_of("mp4"), "video/mp4");
        assert_eq!(mime_of("nope"), "application/octet-stream");
        assert_eq!(mime_of("csv"), "text/plain; charset=utf-8");
    }
}
