//! 服务端事件广播（SSE）。
//!
//! 存在的理由：传输是**两端的事**。A 往 B 拖文件时，B 的浏览器如果只能靠手动刷新
//! 才知道有文件进来了，这个工具就还是半成品。用 SSE 而不是 WebSocket，
//! 是因为数据流只有一个方向（服务端 -> 浏览器），SSE 自带断线重连、代码量少一个数量级。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use serde::Serialize;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use axum::extract::State;

use crate::auth::AuthUser;
use crate::state::AppState;

impl TransferEvent {
    /// 投送活动只给原生 GUI 看。设备名、文件数和进度没必要广播给
    /// 所有浏览器标签页——浏览器的全局 SSE 只负责共享目录的变化。
    fn visible_to_browsers(&self) -> bool {
        !matches!(
            self,
            TransferEvent::PushStarted { .. }
                | TransferEvent::PushProgress { .. }
                | TransferEvent::PushFinished { .. }
                | TransferEvent::PushFailed { .. }
        )
    }
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream(state)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransferEvent {
    /// 某个上传开始（含断点续传的恢复）。
    UploadStarted {
        id: String,
        name: String,
        size: u64,
        /// 本次是恢复而非新建，前端可以提示"继续上次未完成的传输"。
        resumed: bool,
    },
    UploadProgress {
        id: String,
        name: String,
        /// 已落盘字节数，直接来自已确认分片的累加，不用估算。
        bytes: u64,
        size: u64,
    },
    UploadFinished {
        id: String,
        name: String,
        /// 相对共享根的最终路径。
        path: String,
    },
    UploadFailed {
        id: String,
        name: String,
        error: String,
    },
    /// 共享目录发生了变化，所有浏览器都该刷新当前列表。
    FilesChanged {
        dir: String,
    },

    // ── 投送给原生客户端（浏览器会忽略这些未知 kind）──
    PushStarted {
        batch_id: String,
        name: String,
        client_name: String,
        file_count: u64,
        total_bytes: u64,
    },
    PushProgress {
        batch_id: String,
        done: u64,
        total_bytes: u64,
    },
    PushFinished {
        batch_id: String,
        bytes: u64,
    },
    PushFailed {
        batch_id: String,
        error: String,
        done: u64,
    },
}

/// 给浏览器的事件流。每个已登录的标签页持有一条。
pub fn stream(state: Arc<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            Ok(ev) => {
                if !ev.visible_to_browsers() {
                    return None;
                }
                let payload = serde_json::to_string(&ev).ok()?;
                Some(Ok(Event::default().data(payload)))
            }
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                // 慢客户端落后于广播缓冲区。断连会让它错过更多，
                // 所以发一个"你落后了"的信号，让前端主动全量刷新一次。
                tracing::debug!("SSE 客户端落后了 {n} 条事件");
                Some(Ok(Event::default().event("lagged").data(n.to_string())))
            }
        }
    });

    // 没有心跳的话，中间的 NAT/防火墙会在静默一段时间后掐掉连接，
    // 而浏览器要等到下次写入失败才发现——表现就是"传完了但界面不动"。
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
