//! 首页、静态资源、运行配置、兜底 404。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::app::App;
use crate::web::assets::serve_file;

pub async fn config(State(app): State<App>) -> Response {
    Json(json!({
        "chunk_size": app.chunk_size,
        "ffmpeg": app.ffmpeg.is_some(),
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

pub async fn fallback() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "接口不存在" }))).into_response()
}
