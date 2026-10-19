use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::{AppState, Session};

pub const SESSION_COOKIE: &str = "wiretoss_session";

/// 所有会改变服务端状态的请求都必须带上这个头。
///
/// `SameSite=Lax` 已经挡住了跨站表单提交，但那依赖浏览器实现；
/// 自定义头是**跨站无法伪造**的（要带它就必须先过 CORS 预检），
/// 两道加起来才够。前端每个 mutating fetch 都会带上它。
pub const CSRF_HEADER: &str = "x-wiretoss";

pub fn require_csrf(headers: &HeaderMap) -> AppResult<()> {
    match headers.get(CSRF_HEADER) {
        Some(_) => Ok(()),
        None => Err(AppError::BadRequest(
            "缺少 X-Wiretoss 请求头（可能来自跨站请求）".into(),
        )),
    }
}

fn new_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("操作系统随机数不可用");
    hex::encode(buf)
}

/// 已登录用户的提取器。挂在需要认证的路由上即可，
/// 未登录会在进入 handler 之前就返回 401。
pub struct AuthUser {
    pub username: String,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(SESSION_COOKIE)
            .map(|c| c.value().to_string())
            .ok_or(AppError::Unauthorized)?;

        let sessions = state
            .sessions
            .lock()
            .map_err(|_| AppError::Internal("会话表锁中毒".into()))?;
        let session = sessions.get(&token).ok_or(AppError::Unauthorized)?;

        if session.expires_at <= Instant::now() {
            return Err(AppError::Unauthorized);
        }
        Ok(AuthUser {
            username: session.username.clone(),
        })
    }
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    let ip = addr.ip();

    if let Some(secs) = blocked_for(&state, ip) {
        return Err(AppError::TooManyRequests(secs));
    }

    // 复制一份配置再验密码：Argon2 是刻意设计得慢的（默认约 50-100ms），
    // 一边持锁一边算会把配置写操作堵住。
    let cfg = state
        .config
        .read()
        .map_err(|_| AppError::Internal("配置锁中毒".into()))?
        .clone();

    let username = req.username.clone();
    // 放到阻塞线程池：Argon2 是纯 CPU 计算，直接在 async worker 上跑会占住执行器。
    let ok = tokio::task::spawn_blocking(move || {
        // 用户名和密码都要验，且失败时对外表现一致，不泄露"用户名是否存在"。
        username == cfg.username && cfg.verify_password(&req.password)
    })
    .await
    .map_err(AppError::internal)?;

    if !ok {
        register_failure(&state, ip);
        tracing::warn!("来自 {ip} 的登录失败");
        return Err(AppError::InvalidCredentials);
    }

    // 登录成功顺手清掉这个 IP 的失败记录，以及表里所有过期会话。
    if let Ok(mut guard) = state.login_guard.lock() {
        guard.remove(&ip);
    }
    state.purge_expired_sessions();

    let token = new_token();
    let hours = cfg_session_hours(&state);
    let expires_at = Instant::now() + Duration::from_secs(hours * 3600);
    state
        .sessions
        .lock()
        .map_err(|_| AppError::Internal("会话表锁中毒".into()))?
        .insert(
            token.clone(),
            Session {
                username: req.username.clone(),
                expires_at,
            },
        );

    let cookie = Cookie::build((SESSION_COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::hours(hours as i64))
        .build();

    tracing::info!("{ip} 登录成功（{hours} 小时有效）");
    Ok((
        jar.add(cookie),
        Json(json!({ "username": req.username, "sessionHours": hours })),
    ))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
) -> AppResult<impl IntoResponse> {
    require_csrf(&headers)?;
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.remove(cookie.value());
        }
    }
    Ok((
        jar.remove(Cookie::from(SESSION_COOKIE)),
        Json(json!({ "ok": true })),
    ))
}

/// 前端启动时用它判断"要不要显示登录页"。
pub async fn me(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> AppResult<impl IntoResponse> {
    let cfg = state
        .config
        .read()
        .map_err(|_| AppError::Internal("配置锁中毒".into()))?;
    Ok(Json(json!({
        "username": user.username,
        "rootName": cfg.root.file_name().map(|n| n.to_string_lossy().to_string()),
        "rootPath": cfg.root.display().to_string(),
    })))
}

fn cfg_session_hours(state: &Arc<AppState>) -> u64 {
    state.config.read().map(|c| c.session_hours).unwrap_or(12)
}

fn blocked_for(state: &Arc<AppState>, ip: IpAddr) -> Option<u64> {
    let guard = state.login_guard.lock().ok()?;
    guard.get(&ip).and_then(|a| a.blocked_for())
}

fn register_failure(state: &Arc<AppState>, ip: IpAddr) {
    if let Ok(mut guard) = state.login_guard.lock() {
        guard.entry(ip).or_default().register_failure();
    }
}
