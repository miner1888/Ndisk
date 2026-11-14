use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// 统一 API 错误:业务码 + 用户可读消息,一处转换为 HTTP 响应。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: m.into(),
        }
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: m.into(),
        }
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: m.into(),
        }
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: m.into(),
        }
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: m.into(),
        }
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: m.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.message });
        (self.status, axum::Json(body)).into_response()
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

/// 2xx JSON 响应(统一包一层,便于扩展)。
pub fn ok<T: serde::Serialize>(v: T) -> Response {
    (StatusCode::OK, axum::Json(v)).into_response()
}

/// 非 2xx JSON 响应(用于无 State 上下文的简单分支)。
pub fn err(status: StatusCode, message: &str) -> Response {
    ApiError {
        status,
        message: message.to_string(),
    }
    .into_response()
}
