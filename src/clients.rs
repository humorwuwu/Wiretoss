//! 原生客户端的在线登记与定向事件通道。
//!
//! 浏览器用的 `/api/events` 是全局广播：每个标签页看到同样的事件，也没有身份。
//! 投送必须定向到某一台设备，所以这里给原生客户端单独开一条 SSE：连接时登记
//! 稳定设备 id，服务端拿着每条连接专属的 mpsc sender 发通知。连接断开时由
//! [`ClientGuard`] 负责注销，避免在线列表里留下幽灵设备。

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// 原生客户端在每次请求里带上自己的稳定设备 id。
pub const CLIENT_ID_HEADER: &str = "x-wiretoss-client-id";

/// 每条连接最多排队这么多条通知。投送是低频操作，16 条足够；
/// 真正占资源的是文件内容，不经过这条通道。
const CHANNEL_CAPACITY: usize = 16;

/// 服务端推给原生客户端的消息。
///
/// `Skip` 只为反序列化存在：客户端收到未来版本新增的 kind 时应该忽略，
/// 而不是整个事件流解析失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PushNotice {
    PushOffer {
        batch_id: String,
        name: String,
        total_files: u64,
        total_bytes: u64,
    },
    #[serde(other)]
    Skip,
}

#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub ip: IpAddr,
    pub connected_at: Instant,
}

pub struct ConnectedClient {
    pub info: ClientInfo,
    /// 本次连接专属的 id。同一设备重连时旧连接的 Drop 不能把新连接删掉。
    pub conn_id: String,
    pub tx: mpsc::Sender<PushNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Offline,
    Busy,
}

#[derive(Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub name: String,
}

/// 校验请求头里的客户端 id。限制成 ASCII 是因为它要放进 HTTP 头，
/// 同时避免把奇怪字符带回日志和界面。
pub fn parse_client_id(headers: &HeaderMap) -> AppResult<String> {
    let raw = headers
        .get(CLIENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::BadRequest("缺少客户端标识".into()))?;
    let id = raw.trim();
    if id.len() < 8
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::BadRequest("客户端标识格式无效".into()));
    }
    Ok(id.to_string())
}

/// 设备名会显示在服务端窗口里。去控制字符、限长，空值给个可读的占位。
pub fn normalize_client_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "未命名设备".to_string()
    } else {
        cleaned.to_string()
    }
}

pub fn require_client_id(headers: &HeaderMap, expected: &str) -> AppResult<()> {
    if parse_client_id(headers)? == expected {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// 把通知投进目标客户端的事件队列。队列满/连接已关都只影响这一次投送，
/// 调用方据此告诉用户「客户端离线/繁忙」。
pub fn send_notice(state: &AppState, client_id: &str, notice: PushNotice) -> Result<(), SendError> {
    let clients = state.clients.lock().map_err(|_| SendError::Offline)?;
    let Some(client) = clients.get(client_id) else {
        return Err(SendError::Offline);
    };
    match client.tx.try_send(notice) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) => Err(SendError::Busy),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::Offline),
    }
}

/// 发布一份新的在线客户端快照。watch 只保留最新值，GUI 不会因为处理慢而丢状态。
pub fn publish_clients(state: &AppState) {
    let mut infos: Vec<ClientInfo> = match state.clients.lock() {
        Ok(clients) => clients.values().map(|c| c.info.clone()).collect(),
        Err(_) => return,
    };
    infos.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    state.client_list.send_replace(infos);
}

/// 原生客户端的长连接。客户端拿到 `push_offer` 后凭 batch id 主动拉文件。
pub async fn events(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let client_id = parse_client_id(&headers)?;
    let name = normalize_client_name(&q.name);
    let conn_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<PushNotice>(CHANNEL_CAPACITY);

    {
        let mut clients = state
            .clients
            .lock()
            .map_err(|_| AppError::Internal("客户端表锁中毒".into()))?;
        // 同 id 重连时 insert 返回旧连接；旧 sender 在这里被 drop，
        // 旧 SSE 流随即结束。旧流的 guard 会因为 conn_id 不匹配而不删新条目。
        clients.insert(
            client_id.clone(),
            ConnectedClient {
                info: ClientInfo {
                    id: client_id.clone(),
                    name,
                    ip: addr.ip(),
                    connected_at: Instant::now(),
                },
                conn_id: conn_id.clone(),
                tx,
            },
        );
    }
    publish_clients(&state);

    // guard 必须和流放在一起：放在 handler 局部变量里会在返回 Response 时
    // 立刻 Drop，客户端还没收到任何事件就被注销了。
    let guard = ClientGuard {
        state: state.clone(),
        client_id: client_id.clone(),
        conn_id,
    };
    let stream = futures_util::stream::unfold((rx, guard), |(mut rx, guard)| async move {
        let notice = rx.recv().await?;
        let payload =
            serde_json::to_string(&notice).unwrap_or_else(|_| r#"{"kind":"skip"}"#.to_string());
        Some((
            Ok::<Event, Infallible>(Event::default().data(payload)),
            (rx, guard),
        ))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

struct ClientGuard {
    state: Arc<AppState>,
    client_id: String,
    conn_id: String,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        let removed = {
            // Drop 里不能 panic：锁中毒时直接把内部数据拿出来继续用。
            let mut clients = match self.state.clients.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let ours = clients
                .get(&self.client_id)
                .map(|c| c.conn_id == self.conn_id)
                .unwrap_or(false);
            if ours {
                clients.remove(&self.client_id);
                true
            } else {
                false
            }
        };
        if removed {
            publish_clients(&self.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(id: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CLIENT_ID_HEADER, HeaderValue::from_str(id).unwrap());
        headers
    }

    #[test]
    fn client_id_validation() {
        assert!(parse_client_id(&headers_with("abc-123_XYZ")).is_ok());
        assert!(parse_client_id(&headers_with("short")).is_err());
        assert!(parse_client_id(&headers_with("bad id with spaces")).is_err());
        assert!(parse_client_id(&headers_with(&"x".repeat(65))).is_err());
        assert!(parse_client_id(&HeaderMap::new()).is_err());
    }

    #[test]
    fn client_name_is_normalized() {
        assert_eq!(normalize_client_name("  MacBook  "), "MacBook");
        assert_eq!(normalize_client_name("a\u{0}b\nc"), "abc");
        assert_eq!(normalize_client_name("   "), "未命名设备");
        assert_eq!(
            normalize_client_name(&"设备".repeat(40)).chars().count(),
            64
        );
    }

    #[test]
    fn require_client_id_rejects_mismatch() {
        let headers = headers_with("client-aaa-111");
        assert!(require_client_id(&headers, "client-aaa-111").is_ok());
        assert!(matches!(
            require_client_id(&headers, "client-bbb-222"),
            Err(AppError::Forbidden)
        ));
    }
}
