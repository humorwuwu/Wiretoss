use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("请先登录")]
    Unauthorized,
    /// 与 `Unauthorized` 分开：401 但前端要显示在登录框里，而不是把用户弹回首页。
    #[error("用户名或密码错误")]
    InvalidCredentials,
    #[error("无权访问")]
    Forbidden,
    #[error("找不到：{0}")]
    NotFound(String),
    #[error("请求无效：{0}")]
    BadRequest(String),
    #[error("已存在：{0}")]
    Conflict(String),
    #[error("登录尝试过于频繁，请 {0} 秒后重试")]
    TooManyRequests(u64),
    #[error("服务器内部错误：{0}")]
    Internal(String),
}

impl AppError {
    /// 把任意错误折叠成 500，同时保留可读的原因。用于 `map_err(AppError::internal)`。
    pub fn internal(e: impl std::fmt::Display) -> Self {
        AppError::Internal(e.to_string())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Unauthorized | AppError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// io::Error 的映射刻意区分开：文件不存在/无权限是客户端能理解的 4xx，
/// 而不是笼统的 500 —— 目录列举时误报 500 会让前端无法区分"空目录"和"服务端炸了"。
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => AppError::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => AppError::Forbidden,
            std::io::ErrorKind::AlreadyExists => AppError::Conflict(e.to_string()),
            _ => AppError::Internal(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::BadRequest(format!("JSON 解析失败：{e}"))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // 5xx 一定是我们的 bug，必须留下日志；4xx 是正常的客户端行为，不打日志免得刷屏。
        if status.is_server_error() {
            tracing::error!("{self}");
        }
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
