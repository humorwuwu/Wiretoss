use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::state::{AppState, CHUNK_SIZE};
use crate::{assets, auth, clients, download, events, fs_api, push, upload};

/// 分片请求体的上限。留一点余量给协议开销，但**绝不能**直接 `disable()`——
/// 那样一个已登录客户端发个超大请求就能把内存打爆。
const MAX_CHUNK_BODY: usize = CHUNK_SIZE as usize + 64 * 1024;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // ---- 无需登录
        .route("/api/login", post(auth::login))
        // ---- 需要登录（由各 handler 上的 AuthUser 提取器强制）
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/list", get(fs_api::list))
        .route("/api/mkdir", post(fs_api::mkdir))
        .route("/api/delete", post(fs_api::delete))
        .route("/api/download", get(download::download_file))
        .route("/api/zip", post(download::create_zip_ticket))
        .route("/api/zip/{ticket}", get(download::download_zip))
        .route("/api/upload/init", post(upload::init))
        .route(
            "/api/upload/chunk/{id}/{index}",
            put(upload::put_chunk).layer(DefaultBodyLimit::max(MAX_CHUNK_BODY)),
        )
        .route("/api/upload/{id}/complete", post(upload::complete))
        .route("/api/upload/{id}/abort", post(upload::abort))
        .route("/api/events", get(events::handler))
        // ── 原生客户端定向事件与投送 ──
        .route("/api/client/events", get(clients::events))
        .route("/api/push/pending", get(push::pending))
        .route("/api/push/{batch}/manifest", get(push::manifest))
        .route("/api/push/{batch}/file/{index}", get(push::file))
        .route("/api/push/{batch}/progress", post(push::progress))
        .route("/api/push/{batch}/complete", post(push::complete))
        // 没匹配上的 /api/* 必须返回 JSON 404。掉进前端 fallback 的话，
        // 前端 fetch 会收到一个 200 的 HTML，然后抛一句莫名其妙的
        // "Unexpected token '<'"，排查成本极高。
        .route("/api/{*rest}", any(api_not_found))
        .fallback(assets::serve)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "接口不存在" })),
    )
}
