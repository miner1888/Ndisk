//! 内嵌静态资源(`web/` 目录随二进制分发,单文件部署)。

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/"]
pub struct Assets;

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

pub fn serve_file(name: &str) -> Option<Response> {
    let f = Assets::get(name)?;
    Some((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type(name)),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        f.data,
    )
        .into_response())
}
