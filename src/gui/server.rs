//! GUI 背后的服务端生命周期管理。
//!
//! 界面线程和网络服务是彻底分开的：服务跑在一条独立线程的 tokio 运行时上，
//! 两边只通过两条 channel 说话（GUI 发指令、服务回报状态）。
//! 这样做的原因是 macOS 要求 GUI 必须在主线程跑，而 axum 需要自己的运行时，
//! 两者不能互相阻塞——服务一旦卡住，窗口就假死了。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::clients::ClientInfo;
use crate::config::Config;
use crate::events::TransferEvent;
use crate::push;
use crate::router;
use crate::state::AppState;

/// GUI -> 服务线程。
pub enum ServerCommand {
    Start(Box<ServerSettings>),
    Stop,
    PushFiles {
        client_id: String,
        paths: Vec<PathBuf>,
    },
}

pub struct ServerSettings {
    pub config: Config,
    /// 监听地址。默认 `0.0.0.0`，即局域网内都能访问。
    pub bind: String,
    pub advertise_mdns: bool,
}

/// 服务线程 -> GUI。
pub enum ServerEvent {
    Started {
        port: u16,
        /// 已经拼好的可访问地址，第一项是局域网地址（对端该用的那个）。
        urls: Vec<String>,
        /// HTTPS 地址；为空表示没有启用 HTTPS。
        https_urls: Vec<String>,
        mdns_ok: bool,
    },
    Stopped,
    /// 启动失败或运行中异常退出。字符串直接显示给用户。
    Failed(String),
    Transfer(TransferEvent),
    /// 在线原生客户端快照。
    Clients(Vec<ClientInfo>),
    /// 非致命错误（投送失败、没有目标等）。**不能**走 Failed，
    /// 否则界面会误以为整个服务已经停止。
    Notice {
        message: String,
        is_error: bool,
    },
}

/// 把事件送进 GUI 并叫醒它重绘。
///
/// egui 是即时模式，没人触发重绘它就一直睡；只把事件塞进 channel
/// 的话，界面要等到用户动一下鼠标才会更新——传文件时看起来就像卡住了。
#[derive(Clone)]
pub struct Bridge {
    tx: Sender<ServerEvent>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Bridge {
    pub fn send(&self, event: ServerEvent) {
        // GUI 已经关掉时 send 会失败，属正常情况。
        let _ = self.tx.send(event);
        (self.notify)();
    }
}

pub struct ServerHandle {
    cmd_tx: tokio_mpsc::UnboundedSender<ServerCommand>,
    pub events: Receiver<ServerEvent>,
}

impl ServerHandle {
    pub fn start(&self, settings: ServerSettings) {
        let _ = self.cmd_tx.send(ServerCommand::Start(Box::new(settings)));
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(ServerCommand::Stop);
    }

    pub fn push_files(&self, client_id: &str, paths: Vec<PathBuf>) {
        let _ = self.cmd_tx.send(ServerCommand::PushFiles {
            client_id: client_id.to_string(),
            paths,
        });
    }
}

/// 起一条后台线程托管服务。
///
/// `notify` 在每次有事件送达时被调用，GUI 用它来请求重绘。
pub fn spawn(notify: impl Fn() + Send + Sync + 'static) -> ServerHandle {
    let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel::<ServerCommand>();
    let (tx, events) = std::sync::mpsc::channel::<ServerEvent>();

    let bridge = Bridge {
        tx,
        notify: Arc::new(notify),
    };

    let thread_bridge = bridge.clone();
    thread::Builder::new()
        .name("wiretoss-server".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    thread_bridge.send(ServerEvent::Failed(format!("无法创建异步运行时：{e}")));
                    return;
                }
            };
            runtime.block_on(server_loop(&mut cmd_rx, thread_bridge));
        })
        .expect("无法创建服务线程");

    ServerHandle { cmd_tx, events }
}

/// 一次运行中的服务。把原先越滚越长的元组收进结构体，
/// 停止时也能明确知道每个后台任务该在哪里结束。
struct RunningServer {
    shutdown: oneshot::Sender<()>,
    serving: JoinHandle<()>,
    maintenance: JoinHandle<()>,
    /// 共享目录监视任务；停止服务时必须 abort，否则它会继续持有旧 AppState。
    watch: Option<JoinHandle<()>>,
    port: u16,
    urls: Vec<String>,
    https_urls: Vec<String>,
    https_shutdown: Option<axum_server::Handle<SocketAddr>>,
    https_serving: Option<JoinHandle<()>>,
    mdns_ok: bool,
    mdns: Option<mdns_sd::ServiceDaemon>,
    state: Arc<AppState>,
}

async fn server_loop(cmd_rx: &mut tokio_mpsc::UnboundedReceiver<ServerCommand>, bridge: Bridge) {
    let mut running: Option<RunningServer> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ServerCommand::Start(settings) => {
                if let Some(server) = running.as_ref() {
                    if !server.serving.is_finished() {
                        // 这是正常提示，不能发 Failed：界面收到 Failed 会把
                        // 「运行中」状态清掉，看起来像服务挂了。
                        bridge.send(ServerEvent::Notice {
                            message: "服务已经在运行了".into(),
                            is_error: false,
                        });
                        continue;
                    }
                }
                // 上一次的 serve 任务自己退出了（通常是运行期报错）。
                // 清掉残留的清理任务/mDNS 再重新启动。
                if let Some(server) = running.take() {
                    shutdown_server(server).await;
                }
                match start_server(*settings, &bridge).await {
                    Ok(server) => {
                        bridge.send(ServerEvent::Started {
                            port: server.port,
                            urls: server.urls.clone(),
                            https_urls: server.https_urls.clone(),
                            mdns_ok: server.mdns_ok,
                        });
                        running = Some(server);
                    }
                    Err(message) => bridge.send(ServerEvent::Failed(message)),
                }
            }
            ServerCommand::Stop => {
                if let Some(server) = running.take() {
                    shutdown_server(server).await;
                }
                bridge.send(ServerEvent::Stopped);
            }
            ServerCommand::PushFiles { client_id, paths } => {
                let Some(server) = &running else {
                    bridge.send(ServerEvent::Notice {
                        message: "服务未启动，无法投送".into(),
                        is_error: true,
                    });
                    continue;
                };
                let state = server.state.clone();
                let push_bridge = bridge.clone();
                tokio::spawn(async move {
                    if let Err(message) = push::create_batch(&state, &client_id, paths).await {
                        push_bridge.send(ServerEvent::Notice {
                            message: format!("投送失败：{message}"),
                            is_error: true,
                        });
                    }
                });
            }
        }
    }
}

/// 停止并回收一次服务运行。
///
/// SSE 是长连接，`with_graceful_shutdown` 会一直等它们关闭；不设上限的话
/// 「停止服务」会永久卡住。超过 3 秒直接 abort，确保监听端口立即释放。
async fn shutdown_server(server: RunningServer) {
    let RunningServer {
        shutdown,
        mut serving,
        maintenance,
        watch,
        https_shutdown,
        https_serving,
        mdns,
        ..
    } = server;

    let _ = shutdown.send(());
    if !serving.is_finished()
        && tokio::time::timeout(Duration::from_secs(3), &mut serving)
            .await
            .is_err()
    {
        tracing::warn!("服务未在 3 秒内退出，强制中止");
        serving.abort();
        let _ = serving.await;
    }

    maintenance.abort();
    let _ = maintenance.await;
    if let Some(watch) = watch {
        watch.abort();
        let _ = watch.await;
    }
    if let Some(handle) = https_shutdown {
        handle.graceful_shutdown(Some(Duration::from_secs(3)));
    }
    if let Some(mut https) = https_serving {
        if !https.is_finished()
            && tokio::time::timeout(Duration::from_secs(3), &mut https)
                .await
                .is_err()
        {
            tracing::warn!("HTTPS 服务未在 3 秒内退出，强制中止");
            https.abort();
            let _ = https.await;
        }
    }
    // drop 即注销 mDNS 广播。
    if mdns.is_some() {
        tracing::info!("已停止 mDNS 广播");
    }
    drop(mdns);
}

async fn start_server(settings: ServerSettings, bridge: &Bridge) -> Result<RunningServer, String> {
    let state = AppState::new(settings.config.clone()).map_err(|e| format!("初始化失败：{e}"))?;

    let addr: SocketAddr = format!("{}:{}", settings.bind, settings.config.port)
        .parse()
        .map_err(|e| format!("监听地址无效：{e}"))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("无法监听 {addr}：{e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("无法读取监听端口：{e}"))?
        .port();

    // 端口写 0 时由系统分配，把实际端口回填给界面显示。
    let (sd_tx, sd_rx) = oneshot::channel();

    // 服务端产生的传输事件转发给界面。界面上的「传输活动」就是这条线来的。
    // 必须处理 Lagged：广播缓冲区满时 recv 会返回 Err，旧写法 `while let Ok`
    // 会因此永久退出，之后再收不到任何事件。
    let mut subscription = state.events.subscribe();
    let event_bridge = bridge.clone();
    tokio::spawn(async move {
        loop {
            match subscription.recv().await {
                Ok(event) => event_bridge.send(ServerEvent::Transfer(event)),
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!("GUI 落后了 {n} 条传输事件，继续接收");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    // 在线客户端快照走单独的 watch 通道：设备名/IP 只给 GUI，
    // 不经过浏览器的全局 SSE。
    let mut client_rx = state.client_list.subscribe();
    let initial = client_rx.borrow_and_update().clone();
    let client_bridge = bridge.clone();
    client_bridge.send(ServerEvent::Clients(initial));
    tokio::spawn(async move {
        loop {
            if client_rx.changed().await.is_err() {
                break;
            }
            let snapshot = client_rx.borrow_and_update().clone();
            client_bridge.send(ServerEvent::Clients(snapshot));
        }
    });

    // GUI 模式原先没有清理任务：上传/下载券/投送批次会一直堆在内存里。
    // 用 Weak 而不是 Arc，避免任务本身把 AppState 永久吊住。
    let weak_state = Arc::downgrade(&state);
    let maintenance = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Some(state) = weak_state.upgrade() else {
                break;
            };
            state.purge_stale();
        }
    });

    let app = router::build_router(state.clone());
    let serve_bridge = bridge.clone();
    let handle = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = sd_rx.await;
        })
        .await;
        if let Err(e) = result {
            serve_bridge.send(ServerEvent::Failed(format!("服务异常退出：{e}")));
        }
    });

    let mdns = if settings.advertise_mdns {
        crate::net::advertise(port)
    } else {
        None
    };
    let mdns_ok = mdns.is_some();
    let urls = access_urls(port);
    let watch = crate::watch::spawn(state.clone());

    let mut https_urls = Vec::new();
    let mut https_shutdown = None;
    let mut https_serving = None;
    if settings.config.https {
        let https_port = settings.config.https_port;
        if https_port == 0 || https_port == port {
            return Err("HTTPS 端口无效：不能为 0，也不能和 HTTP 端口相同".into());
        }
        let tls_paths = crate::tls::ensure().map_err(|e| format!("初始化 HTTPS 失败：{e}"))?;
        crate::tls::install_crypto_provider();
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls_paths.cert, &tls_paths.key)
                .await
                .map_err(|e| format!("加载 HTTPS 证书失败：{e}"))?;
        let https_addr: SocketAddr = format!("{}:{}", settings.bind, https_port)
            .parse()
            .map_err(|_| "HTTPS 监听地址无效".to_string())?;

        let https_app = router::build_router(state.clone());
        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();
        let https_bridge = bridge.clone();
        let https_task = tokio::spawn(async move {
            let result = axum_server::bind_rustls(https_addr, tls_config)
                .handle(handle)
                .serve(https_app.into_make_service_with_connect_info::<SocketAddr>())
                .await;
            if let Err(e) = result {
                https_bridge.send(ServerEvent::Failed(format!("HTTPS 服务异常退出：{e}")));
            }
        });
        https_urls = https_access_urls(https_port);
        https_shutdown = Some(shutdown_handle);
        https_serving = Some(https_task);
    }

    Ok(RunningServer {
        shutdown: sd_tx,
        serving: handle,
        maintenance,
        watch,
        port,
        urls,
        https_urls,
        https_shutdown,
        https_serving,
        mdns_ok,
        mdns,
        state,
    })
}

/// 列出所有可用的访问地址。局域网地址排在最前面，因为对端该用的是它。
pub fn access_urls(port: u16) -> Vec<String> {
    let mut urls = vec![format!("http://localhost:{port}")];
    for ip in crate::net::lan_ipv4_addresses() {
        urls.push(format!("http://{ip}:{port}"));
    }
    urls.push(format!("http://{}.local:{port}", crate::net::hostname()));
    urls
}

pub fn https_access_urls(port: u16) -> Vec<String> {
    let mut urls = vec![format!("https://localhost:{port}")];
    for ip in crate::net::lan_ipv4_addresses() {
        urls.push(format!("https://{ip}:{port}"));
    }
    urls.push(format!("https://{}.local:{port}", crate::net::hostname()));
    urls
}
