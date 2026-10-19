//! 客户端模式的运行时。
//!
//! 和 `gui::server` 同构：界面线程只发命令、收事件，网络和磁盘操作全部在
//! 独立线程的 tokio 运行时里。连接断了会退避重连；手动断开则中止任务。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::task::JoinHandle;

use crate::client::{self, ClientError, ClientSession, SseParser};
use crate::clients::PushNotice;

pub struct ClientRunSettings {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub client_id: String,
    pub display_name: String,
    pub receive_dir: PathBuf,
}

pub enum ClientCommand {
    Connect(Box<ClientRunSettings>),
    Disconnect,
}

#[derive(Clone, Debug)]
pub enum ClientStatus {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, error: String },
    Disconnected,
    Failed(String),
}

#[derive(Debug)]
pub enum ClientEvent {
    Status(ClientStatus),
    BatchStarted {
        batch_id: String,
        name: String,
        total_files: u64,
        total_bytes: u64,
    },
    BatchProgress {
        batch_id: String,
        done: u64,
        total_bytes: u64,
    },
    BatchFinished {
        batch_id: String,
        bytes: u64,
    },
    BatchFailed {
        batch_id: String,
        error: String,
        done: u64,
    },
}

#[derive(Clone)]
struct Bridge {
    tx: Sender<ClientEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Bridge {
    fn send(&self, event: ClientEvent) {
        let _ = self.tx.send(event);
        (self.notify)();
    }
}

pub struct ClientHandle {
    cmd_tx: tokio_mpsc::UnboundedSender<ClientCommand>,
    pub events: Receiver<ClientEvent>,
}

impl ClientHandle {
    pub fn connect(&self, settings: ClientRunSettings) {
        let _ = self.cmd_tx.send(ClientCommand::Connect(Box::new(settings)));
    }

    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(ClientCommand::Disconnect);
    }
}

pub fn spawn(notify: impl Fn() + Send + Sync + 'static) -> ClientHandle {
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<ClientCommand>();
    let (tx, events) = std::sync::mpsc::channel::<ClientEvent>();
    let bridge = Bridge {
        tx,
        notify: Arc::new(notify),
    };

    let thread_bridge = bridge.clone();
    thread::Builder::new()
        .name("wiretoss-client".into())
        .spawn(move || {
            // 客户端只有一条 SSE + 一个下载 worker，单线程运行时足够，
            // 也避免在服务端模式下白养一整组 worker 线程。
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    thread_bridge.send(ClientEvent::Status(ClientStatus::Failed(format!(
                        "无法创建异步运行时：{e}"
                    ))));
                    return;
                }
            };
            runtime.block_on(client_loop(cmd_rx, thread_bridge));
        })
        .expect("无法创建客户端线程");

    ClientHandle { cmd_tx, events }
}

async fn client_loop(mut cmd_rx: tokio_mpsc::UnboundedReceiver<ClientCommand>, bridge: Bridge) {
    let mut session: Option<JoinHandle<()>> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ClientCommand::Connect(settings) => {
                if let Some(handle) = session.take() {
                    handle.abort();
                    let _ = handle.await;
                }
                let session_bridge = bridge.clone();
                session = Some(tokio::spawn(async move {
                    run_session(*settings, session_bridge).await;
                }));
            }
            ClientCommand::Disconnect => {
                if let Some(handle) = session.take() {
                    handle.abort();
                    let _ = handle.await;
                }
                bridge.send(ClientEvent::Status(ClientStatus::Disconnected));
            }
        }
    }
}

async fn run_session(settings: ClientRunSettings, bridge: Bridge) {
    bridge.send(ClientEvent::Status(ClientStatus::Connecting));

    if let Err(e) = tokio::fs::create_dir_all(&settings.receive_dir).await {
        bridge.send(ClientEvent::Status(ClientStatus::Failed(format!(
            "无法创建接收目录：{e}"
        ))));
        return;
    }
    let receive_dir = match tokio::fs::canonicalize(&settings.receive_dir).await {
        Ok(path) => path,
        Err(e) => {
            bridge.send(ClientEvent::Status(ClientStatus::Failed(format!(
                "接收目录不可用：{e}"
            ))));
            return;
        }
    };

    let session = match ClientSession::new(
        &settings.base_url,
        settings.client_id.clone(),
        settings.display_name.clone(),
    ) {
        Ok(session) => session,
        Err(e) => {
            bridge.send(ClientEvent::Status(ClientStatus::Failed(e.to_string())));
            return;
        }
    };

    // 下载 worker 跨重连存活：SSE 断了不代表正在下载的文件要作废。
    let (batch_tx, batch_rx) = tokio_mpsc::channel::<String>(64);
    let _worker = AbortOnDrop(tokio::spawn(download_worker(
        session.clone(),
        batch_rx,
        receive_dir,
        bridge.clone(),
    )));

    let mut attempt: u32 = 0;
    loop {
        match session.login(&settings.username, &settings.password).await {
            Ok(()) => {}
            Err(ClientError::Auth) => {
                bridge.send(ClientEvent::Status(ClientStatus::Failed(
                    "用户名或密码错误".into(),
                )));
                return;
            }
            Err(ClientError::Http(429, _)) => {
                bridge.send(ClientEvent::Status(ClientStatus::Failed(
                    "登录尝试过于频繁，请稍后再试".into(),
                )));
                return;
            }
            Err(e) => {
                attempt = attempt.saturating_add(1);
                bridge.send(ClientEvent::Status(ClientStatus::Reconnecting {
                    attempt,
                    error: e.to_string(),
                }));
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
        }

        bridge.send(ClientEvent::Status(ClientStatus::Connected));
        attempt = 0;

        let error = match stream_events(&session, &batch_tx).await {
            Ok(()) => "服务器关闭了连接".to_string(),
            Err(e) => e.to_string(),
        };

        attempt = attempt.saturating_add(1);
        bridge.send(ClientEvent::Status(ClientStatus::Reconnecting {
            attempt,
            error,
        }));
        tokio::time::sleep(backoff(attempt)).await;
    }
}

/// SSE 读循环。断线、45 秒没有心跳（服务端每 15 秒一次）都视为需要重连。
async fn stream_events(
    session: &ClientSession,
    batch_tx: &tokio_mpsc::Sender<String>,
) -> Result<(), ClientError> {
    let mut stream = session.events().await?;

    // SSE 建好后再补一次待接收批次：断线期间新投送的 offer 可能已经错过了。
    match session.pending().await {
        Ok(batches) => {
            for batch in batches {
                if batch_tx.send(batch.batch_id).await.is_err() {
                    return Err(ClientError::Other("下载任务已停止".into()));
                }
            }
        }
        Err(e) => tracing::warn!("拉取待接收批次失败：{e}"),
    }

    let mut parser = SseParser::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(45), stream.next()).await;
        match next {
            Err(_) => return Err(ClientError::Network("服务器心跳超时".into())),
            Ok(None) => return Err(ClientError::Network("服务器关闭了连接".into())),
            Ok(Some(Err(e))) => return Err(ClientError::Network(e.to_string())),
            Ok(Some(Ok(bytes))) => {
                for notice in parser.push(&bytes) {
                    if let PushNotice::PushOffer { batch_id, .. } = notice {
                        if batch_tx.send(batch_id).await.is_err() {
                            return Err(ClientError::Other("下载任务已停止".into()));
                        }
                    }
                }
            }
        }
    }
}

async fn download_worker(
    session: ClientSession,
    mut rx: tokio_mpsc::Receiver<String>,
    receive_dir: PathBuf,
    bridge: Bridge,
) {
    // 同一次连接里 SSE offer 和 pending 可能把同一个批次送进来两次。
    // 失败且没有成功 complete 的批次要从集合里移除，重连后的 pending 才能重试。
    let mut seen: HashSet<String> = HashSet::new();

    while let Some(batch_id) = rx.recv().await {
        if !seen.insert(batch_id.clone()) {
            continue;
        }

        let manifest = match session.fetch_manifest(&batch_id).await {
            Ok(manifest) if manifest.batch_id == batch_id => manifest,
            Ok(_) => {
                seen.remove(&batch_id);
                bridge.send(ClientEvent::BatchFailed {
                    batch_id,
                    error: "服务器返回了不匹配的批次".into(),
                    done: 0,
                });
                continue;
            }
            Err(e) => {
                seen.remove(&batch_id);
                bridge.send(ClientEvent::BatchFailed {
                    batch_id,
                    error: e.to_string(),
                    done: 0,
                });
                continue;
            }
        };

        bridge.send(ClientEvent::BatchStarted {
            batch_id: batch_id.clone(),
            name: manifest.name.clone(),
            total_files: manifest.total_files,
            total_bytes: manifest.total_bytes,
        });

        let progress_bridge = bridge.clone();
        let progress_id = batch_id.clone();
        let total_bytes = manifest.total_bytes;
        let on_progress = move |done: u64| {
            progress_bridge.send(ClientEvent::BatchProgress {
                batch_id: progress_id.clone(),
                done,
                total_bytes,
            });
        };

        match client::run_batch(&session, &manifest, &receive_dir, &on_progress).await {
            Ok(outcome) if outcome.errors.is_empty() => {
                bridge.send(ClientEvent::BatchFinished {
                    batch_id,
                    bytes: outcome.bytes,
                });
            }
            Ok(outcome) => {
                bridge.send(ClientEvent::BatchFailed {
                    batch_id,
                    error: outcome
                        .errors
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("；"),
                    done: outcome.bytes,
                });
            }
            Err(e) => {
                seen.remove(&batch_id);
                bridge.send(ClientEvent::BatchFailed {
                    batch_id,
                    error: e.to_string(),
                    done: 0,
                });
            }
        }
    }
}

/// 只 drop `JoinHandle` 会分离任务，不会停止它。断开连接时必须显式 abort。
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << (attempt.saturating_sub(1)).min(4);
    Duration::from_secs(secs.min(30))
}
