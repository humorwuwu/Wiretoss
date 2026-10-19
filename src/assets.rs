//! 把前端资源编译进二进制。
//!
//! 这是"零安装"承诺的技术基础：产物是一个可执行文件，
//! 拷到对端双击就跑，不需要附带任何目录、也不依赖对方装了什么。

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = Assets::get(path) {
        return asset_response(file);
    }

    // 前端路由（/login 之类）在服务端没有对应文件，回落到 index.html 自己处理，
    // 否则用户刷新一次页面就 404 了。
    if !path.contains('.') {
        if let Some(file) = Assets::get("index.html") {
            return asset_response(file);
        }
    }

    (StatusCode::NOT_FOUND, "404 Not Found").into_response()
}

fn asset_response(file: rust_embed::EmbeddedFile) -> Response {
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
            // 不缓存：这个工具会随版本升级换掉内嵌的界面，
            // 强缓存会让用户对着旧 UI 排查一个已经修好的 bug。
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        file.data,
    )
        .into_response()
}
