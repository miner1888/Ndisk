use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rust_embed::RustEmbed;
use serde_json::json;

use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "web/"]
pub struct Assets;

pub async fn config(State(st): State<AppState>) -> Response {
    Json(json!({
        "chunk_size": st.chunk_size,
        "ffmpeg": st.ffmpeg.is_some(),
    }))
    .into_response()
}

pub async fn index() -> Response {
    serve_file("index.html").unwrap_or_else(|| (StatusCode::NOT_FOUND, "missing").into_response())
}

pub async fn static_file(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    match serve_file(path.trim_start_matches('/')) {
        Some(r) => r,
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn serve_file(name: &str) -> Option<Response> {
    let f = Assets::get(name)?;
    let mime = match name.rsplit('.').next().unwrap_or("") {
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
    };
    Some(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                // 静态资源随二进制分发,同版本内容不变,可长缓存;index 由调用方单独处理
                (header::CACHE_CONTROL, "no-cache"),
            ],
            f.data,
        )
            .into_response(),
    )
}

/// SPA 之外没有更多页面;未知路径统一 404 JSON,便于 API 调试。
pub async fn fallback() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "接口不存在" })),
    )
        .into_response()
}
