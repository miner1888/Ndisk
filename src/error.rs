//! 统一 API 错误:所有失败响应都是 `{"error": "..."}`,状态码语义化。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: m.into() }
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self { status: StatusCode::UNAUTHORIZED, message: m.into() }
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: m.into() }
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: m.into() }
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self { status: StatusCode::CONFLICT, message: m.into() }
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: m.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(json!({ "error": self.message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::internal(format!("数据库错误: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::internal(format!("文件系统错误: {e}"))
    }
}

/// 200 + JSON 成功响应。
pub fn ok<T: serde::Serialize>(v: T) -> Response {
    (StatusCode::OK, axum::Json(v)).into_response()
}

/// 指定状态码 + `{"error": ...}`。
pub fn err(status: StatusCode, message: &str) -> Response {
    ApiError { status, message: message.to_string() }.into_response()
}
