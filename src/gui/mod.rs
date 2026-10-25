//! 桌面控制面板。
//!
//! 界面只负责「配置 + 状态 + 观察」：启停服务、设账号密码、选共享目录、
//! 显示访问地址和二维码、看实时传输进度。**传文件本身仍然在浏览器里做** ——
//! 对端本来就只有浏览器，把传输界面搬到原生窗口里对那台机器毫无帮助。

mod client;
mod logging;
mod server;
mod theme;
mod tray;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use eframe::egui;

use crate::clients::ClientInfo;
use crate::config::{self, Config, StartMode};
use crate::events::TransferEvent;

use client::{ClientEvent, ClientStatus};
use server::{ServerEvent, ServerHandle, ServerSettings};
use tray::TrayCommand;

pub fn run(initial_mode: Option<StartMode>) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Wiretoss")
        .with_app_id("dev.wiretoss.app")
        .with_inner_size([1000.0, 780.0])
        .with_min_inner_size([560.0, 560.0])
        // 服务端要接收 OS 拖放；显式打开，避免某些平台默认关闭。
        .with_drag_and_drop(true);

    // eframe 默认会把应用图标设成 egui 自带的那张。在 macOS 上这会调用
    // NSApplication.setApplicationIconImage，Dock 图标因此变成 egui 的黑色图标，
    // 而 Finder 里显示的仍是 .app 包里的 AppIcon.icns —— 两边看起来不一样。
    // IconData::default() 是 egui 文档里约定的「用系统默认图标」：macOS 保留
    // bundle 图标，Windows / Linux 用下面这张和打包图标同源的 PNG。
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_icon(Arc::new(egui::IconData::default()));
    #[cfg(not(target_os = "macos"))]
    let viewport = viewport.with_icon(Arc::new(app_icon()));

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Wiretoss",
        options,
        Box::new(move |cc| Ok(Box::new(GuiApp::new(cc, initial_mode)))),
    )
}

/// Windows / Linux 的窗口与任务栏图标。
///
/// 由 `packaging/icon.svg` 导出（`sips -s format png` 渲染后缩到 256px），和
/// macOS 打包脚本生成 AppIcon.icns 用的是同一份图稿；改图稿时两张一起更新。
/// `IconData::default()` 虽然能「不设置图标」，但在这些平台上系统默认图标
/// 就是 exe 的白纸/空白图标，所以这里必须给一张真的。
#[cfg(not(target_os = "macos"))]
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../packaging/icon-256.png"))
        .expect("packaging/icon-256.png 是随源码提交的图标，不应解码失败")
}

// ─────────────────────────── 传输活动 ───────────────────────────

#[derive(PartialEq)]
enum ActivityState {
    Running,
    Done,
    Failed(String),
}

struct Activity {
    /// 上传用 upload id，投送用 batch id。按 id 匹配，同名文件才不会串进度。
    id: String,
    name: String,
    done: u64,
    total: u64,
    state: ActivityState,
    /// true = 发送（↑），false = 接收（↓）。
    outgoing: bool,
    /// 投送/接收的对端名字。
    peer: Option<String>,
}

// ─────────────────────────── 应用状态 ───────────────────────────

struct Running {
    port: u16,
    urls: Vec<String>,
    https_urls: Vec<String>,
    mdns_ok: bool,
}

struct GuiApp {
    server: ServerHandle,
    running: Option<Running>,
    mode: StartMode,

    // ── 在线客户端与投送 ──
    clients: Vec<ClientInfo>,
    /// 点选中的接收方。
    push_target: Option<String>,
    /// 未选中目标且多客户端在线时，等待用户选择接收方的拖入文件。
    pending_drop: Option<Vec<PathBuf>>,

    // ── 客户端模式 ──
    client: client::ClientHandle,
    client_status: ClientStatus,
    client_server_url: String,
    client_username: String,
    client_password: String,
    client_password_visible: bool,
    client_display_name: String,
    client_receive_dir_input: String,

    // ── 设置表单 ──
    /// 共享目录在界面上是个文本框，所以留一份字符串镜像。
    /// 直接给 TextEdit 传 `&mut self.root.display().to_string()` 是编译不过的
    /// （那是临时值），而路径本身又不是 UTF-8 保证的，只能这么过渡一下。
    root_input: String,
    username: String,
    /// 留空表示「不改动已保存的密码」。
    /// 配置里只存 Argon2 哈希，所以界面**无法**把已有密码回显出来。
    password_input: String,
    password_visible: bool,
    port: u16,
    bind: String,
    advertise_mdns: bool,
    /// 配置里是否已经有密码了。没有的话「启动」按钮要禁用（除非开了访客只读）。
    has_password: bool,
    /// 允许没有账号的访客只读浏览/下载；写操作仍需登录。
    allow_guest: bool,
    /// 是否同时启动 HTTPS 监听。桌面浏览器保存到本机目录需要安全上下文。
    https: bool,
    https_port: u16,

    // ── 显示状态 ──
    activity: Vec<Activity>,
    log: Vec<String>,
    /// 服务端 tracing 日志的入口。GUI 模式下 tracing 不再往终端写，
    /// 而是走这条 channel 进到窗口的日志面板里。
    log_rx: Receiver<String>,
    banner: Option<(String, bool)>, // (文案, 是否错误)
    tray: Option<tray::Tray>,
    /// 用户从托盘菜单点了退出，下一次关闭请求要放行。
    quitting: bool,
}

impl GuiApp {
    fn new(cc: &eframe::CreationContext<'_>, initial_mode: Option<StartMode>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        theme::install(&cc.egui_ctx);

        // 配置不存在时直接在 GUI 里建一份空壳，等用户在窗口里设密码。
        // 这里绝不能自动生成一个随机密码：用户没见过那串东西，
        // 界面却会因此认为"已经设过了"而自动开服，谁也登不进去。
        let config = match Config::load() {
            Ok(cfg) => cfg,
            Err(_) => Config::create_without_password(config::DEFAULT_USERNAME)
                .or_else(|_| Config::load())
                .unwrap_or_else(|_| Config {
                    version: 1,
                    username: config::DEFAULT_USERNAME.to_string(),
                    password_hash: String::new(),
                    root: std::env::temp_dir().join("Wiretoss"),
                    port: 8080,
                    session_hours: 12,
                    client: config::ClientSettings::default(),
                    start_mode: config::StartMode::Server,
                    allow_guest: false,
                    https: false,
                    https_port: config::DEFAULT_HTTPS_PORT,
                }),
        };

        let mode = initial_mode.unwrap_or(config.start_mode);

        // 日志先接好，后面服务线程产生的 tracing 输出才有地方去。
        let (log_tx, log_rx) = std::sync::mpsc::channel::<String>();
        let ctx_for_log = cc.egui_ctx.clone();
        logging::init(log_tx, Arc::new(move || ctx_for_log.request_repaint()));

        let ctx = cc.egui_ctx.clone();
        let server = server::spawn(move || ctx.request_repaint());

        let ctx_for_client = cc.egui_ctx.clone();
        let client = client::spawn(move || ctx_for_client.request_repaint());

        let client_display_name = if config.client.display_name.trim().is_empty() {
            crate::net::hostname()
        } else {
            config.client.display_name.clone()
        };
        let client_receive_dir = config
            .client
            .receive_dir
            .clone()
            .unwrap_or_else(config::default_receive_dir);

        let mut app = Self {
            server,
            running: None,
            mode,
            clients: Vec::new(),
            push_target: None,
            pending_drop: None,
            client,
            client_status: ClientStatus::Disconnected,
            client_server_url: config.client.server_url.clone(),
            client_username: config.client.username.clone(),
            client_password: String::new(),
            client_password_visible: false,
            client_display_name,
            client_receive_dir_input: client_receive_dir.display().to_string(),
            root_input: config.root.display().to_string(),
            username: config.username.clone(),
            password_input: String::new(),
            password_visible: false,
            port: config.port,
            bind: "0.0.0.0".to_string(),
            advertise_mdns: true,
            has_password: !config.password_hash.is_empty(),
            allow_guest: config.allow_guest,
            https: config.https,
            https_port: config.https_port,
            activity: Vec::new(),
            log: Vec::new(),
            log_rx,
            banner: None,
            tray: None,
            quitting: false,
        };

        app.tray = tray::create(&cc.egui_ctx);
        if app.tray.is_some() {
            app.log("菜单栏图标已就绪：关闭窗口只会隐藏，服务继续运行");
        } else {
            // 托盘建不起来时关闭窗口就是真退出，否则窗口藏起来之后
            // 用户再也没有办法把它叫回来。
            app.log("未能创建菜单栏图标，关闭窗口将直接退出程序");
        }

        if app.mode == StartMode::Server {
            app.log(format!("共享目录 {}", app.root_input));
            // 已经设过密码就直接开服：这个程序存在的意义就是"能被连上"，
            // 每次打开还要手点一下「启动服务」纯属多余。
            // 首次运行没密码，只能等用户设完再启动。
            if app.has_password || app.allow_guest {
                app.start();
            } else {
                app.banner = Some((
                    "首次使用：请设置访问密码，或开启「允许无账号只读访问」".into(),
                    false,
                ));
            }
        } else {
            app.log("客户端模式：填写服务器信息后点「连接」");
        }

        app
    }

    /// 记一条日志。
    ///
    /// 走 tracing 而不是直接 push 到 self.log：这样窗口面板和日志文件会
    /// 同时拿到，时间戳也由 tracing 统一加，不用自己格式化。
    fn log(&mut self, message: impl Into<String>) {
        tracing::info!("{}", message.into());
    }

    fn push_log_line(&mut self, line: String) {
        self.log.push(line);
        // 日志不设上限的话，跑几天就会吃掉几百 MB。
        if self.log.len() > 500 {
            self.log.drain(..self.log.len() - 500);
        }
    }

    fn primary_url(&self) -> Option<String> {
        let running = self.running.as_ref()?;
        // 第一项是 localhost，对端用不了；优先给局域网地址。
        running
            .urls
            .iter()
            .find(|u| !u.contains("localhost"))
            .or_else(|| running.urls.first())
            .cloned()
    }

    fn start(&mut self) {
        self.banner = None;

        // 没设过密码又没在输入框里填，也不开访客只读，就不让启动 ——
        // 否则会开出一个任何人都进不来的服务。
        if !self.has_password && self.password_input.trim().is_empty() && !self.allow_guest {
            self.banner = Some(("请先设置一个访问密码，或开启无账号只读访问".into(), true));
            return;
        }
        if self.https && (self.https_port == 0 || self.https_port == self.port) {
            self.banner = Some(("HTTPS 端口不能为 0，也不能和 HTTP 端口相同".into(), true));
            return;
        }

        let mut cfg = match Config::load() {
            Ok(cfg) => cfg,
            Err(e) => {
                self.banner = Some((format!("读取配置失败：{e}"), true));
                return;
            }
        };

        let root = PathBuf::from(self.root_input.trim());
        if root.as_os_str().is_empty() {
            self.banner = Some(("请先选择共享目录".into(), true));
            return;
        }
        // 目录不存在就建出来，省得用户自己先去 Finder 里新建一个。
        if let Err(e) = std::fs::create_dir_all(&root) {
            self.banner = Some((format!("无法创建共享目录 {}：{e}", root.display()), true));
            return;
        }

        cfg.root = root;
        cfg.username = self.username.trim().to_string();
        cfg.port = self.port;
        cfg.allow_guest = self.allow_guest;
        cfg.https = self.https;
        cfg.https_port = self.https_port;

        if cfg.username.is_empty() {
            self.banner = Some(("用户名不能为空".into(), true));
            return;
        }

        if !self.password_input.trim().is_empty() {
            if let Err(e) = cfg.set_password(self.password_input.trim()) {
                self.banner = Some((format!("保存密码失败：{e}"), true));
                return;
            }
            self.has_password = true;
        } else if let Err(e) = cfg.save() {
            self.banner = Some((format!("保存配置失败：{e}"), true));
            return;
        }

        self.password_input.clear();
        self.password_visible = false;
        self.log(format!("正在启动服务，共享目录 {}", cfg.root.display()));

        self.server.start(ServerSettings {
            config: cfg,
            bind: self.bind.clone(),
            advertise_mdns: self.advertise_mdns,
        });
    }

    fn stop(&mut self) {
        self.log("正在停止服务…");
        self.server.stop();
    }

    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Started {
                port,
                urls,
                https_urls,
                mdns_ok,
            } => {
                self.log(format!("服务已启动，监听端口 {port}"));
                if !https_urls.is_empty() {
                    self.log("HTTPS 已启用（桌面浏览器保存到本机目录）");
                }
                self.running = Some(Running {
                    port,
                    urls,
                    https_urls,
                    mdns_ok,
                });
            }
            ServerEvent::Stopped => {
                self.log("服务已停止");
                self.running = None;
                self.clients.clear();
                self.push_target = None;
                self.pending_drop = None;
            }
            ServerEvent::Failed(message) => {
                self.log(format!("错误：{message}"));
                self.banner = Some((message, true));
                self.running = None;
                self.clients.clear();
                self.push_target = None;
                self.pending_drop = None;
                // 让后台线程回收残留的清理任务/mDNS，并回到未运行状态。
                // 「已经在运行」现在走 Notice，不会误触发这里。
                self.server.stop();
            }
            ServerEvent::Transfer(event) => self.apply_transfer(event),
            ServerEvent::Clients(clients) => {
                self.clients = clients;
                self.normalize_push_target();
            }
            ServerEvent::Notice { message, is_error } => {
                self.log(message.clone());
                self.banner = Some((message, is_error));
            }
        }
    }

    fn apply_transfer(&mut self, event: TransferEvent) {
        match event {
            TransferEvent::UploadStarted {
                id,
                name,
                size,
                resumed,
            } => {
                if resumed {
                    self.log(format!("继续接收「{name}」"));
                } else {
                    self.log(format!("开始接收「{name}」（{}）", human_size(size)));
                }
                self.activity_start(id, name, size, false, None);
            }
            TransferEvent::UploadProgress {
                id, bytes, size, ..
            } => {
                self.activity_progress(&id, bytes.min(size), size);
            }
            TransferEvent::UploadFinished { id, name, path } => {
                self.log(format!("接收完成：{path}"));
                let _ = name;
                self.activity_finish(&id, None);
            }
            TransferEvent::UploadFailed {
                id, name, error, ..
            } => {
                self.log(format!("「{name}」失败：{error}"));
                self.activity_fail(&id, error, None, false);
            }
            TransferEvent::PushStarted {
                batch_id,
                name,
                client_name,
                file_count,
                total_bytes,
            } => {
                self.log(format!(
                    "开始投送「{name}」给 {client_name}（{file_count} 个文件，{}）",
                    human_size(total_bytes)
                ));
                self.activity_start(batch_id, name, total_bytes, true, Some(client_name));
            }
            TransferEvent::PushProgress {
                batch_id,
                done,
                total_bytes,
            } => {
                self.activity_progress(&batch_id, done, total_bytes);
            }
            TransferEvent::PushFinished { batch_id, bytes } => {
                self.log(format!("投送完成（{}）", human_size(bytes)));
                self.activity_finish(&batch_id, Some(bytes));
            }
            TransferEvent::PushFailed {
                batch_id,
                error,
                done,
            } => {
                self.log(format!("投送失败：{error}"));
                self.activity_fail(&batch_id, error, Some(done), true);
            }
            // 目录变化只对浏览器有意义，控制面板不关心。
            TransferEvent::FilesChanged { .. } => {}
        }

        if self.activity.len() > 50 {
            self.activity.drain(..self.activity.len() - 50);
        }
    }

    fn activity_start(
        &mut self,
        id: String,
        name: String,
        total: u64,
        outgoing: bool,
        peer: Option<String>,
    ) {
        let activity = Activity {
            id: id.clone(),
            name,
            done: 0,
            total,
            state: ActivityState::Running,
            outgoing,
            peer,
        };
        if let Some(existing) = self.activity.iter_mut().find(|a| a.id == id) {
            *existing = activity;
        } else {
            self.activity.push(activity);
        }
    }

    fn activity_progress(&mut self, id: &str, done: u64, total: u64) {
        if let Some(row) = self.activity.iter_mut().rev().find(|a| a.id == id) {
            row.done = if total > 0 { done.min(total) } else { done };
            row.total = total.max(row.total);
            // 上一次失败后客户端可能重试；进度重新出现时恢复为运行中。
            if row.state != ActivityState::Running {
                row.state = ActivityState::Running;
            }
        } else {
            self.activity.push(Activity {
                id: id.to_string(),
                name: "投送".into(),
                done,
                total,
                state: ActivityState::Running,
                outgoing: false,
                peer: None,
            });
        }
        if self.activity.len() > 50 {
            self.activity.drain(..self.activity.len() - 50);
        }
    }

    fn activity_finish(&mut self, id: &str, bytes: Option<u64>) {
        if let Some(row) = self.activity.iter_mut().rev().find(|a| a.id == id) {
            match bytes {
                Some(bytes) => {
                    row.done = bytes;
                    row.total = row.total.max(bytes);
                }
                None => row.done = row.total,
            }
            row.state = ActivityState::Done;
        }
    }

    fn activity_fail(&mut self, id: &str, error: String, done: Option<u64>, outgoing: bool) {
        if let Some(row) = self.activity.iter_mut().rev().find(|a| a.id == id) {
            if let Some(done) = done {
                row.done = done;
            }
            row.state = ActivityState::Failed(error);
        } else {
            self.activity.push(Activity {
                id: id.to_string(),
                name: "投送".into(),
                done: done.unwrap_or(0),
                total: 0,
                state: ActivityState::Failed(error),
                outgoing,
                peer: None,
            });
        }
    }

    fn normalize_push_target(&mut self) {
        if let Some(id) = &self.push_target {
            if !self.clients.iter().any(|c| &c.id == id) {
                self.push_target = None;
            }
        }
        // 只有一个客户端时省掉一次点击；多个客户端保持「未选中」，
        // 拖入后弹窗让用户明确选择。
        if self.push_target.is_none() && self.clients.len() == 1 {
            self.push_target = Some(self.clients[0].id.clone());
        }
    }

    fn switch_mode(&mut self, mode: StartMode) {
        if mode == self.mode {
            return;
        }
        match mode {
            StartMode::Client => {
                if self.running.is_some() {
                    self.stop();
                }
                self.log("已切换到客户端模式");
            }
            StartMode::Server => {
                self.client.disconnect();
                self.client_status = ClientStatus::Disconnected;
                self.log("已切换到服务端模式");
            }
        }
        self.mode = mode;
        self.pending_drop = None;
        self.banner = None;
        self.save_start_mode(mode);
    }

    fn save_start_mode(&mut self, mode: StartMode) {
        match Config::load() {
            Ok(mut cfg) => {
                cfg.start_mode = mode;
                if let Err(e) = cfg.save() {
                    self.log(format!("保存启动模式失败：{e}"));
                }
            }
            Err(e) => self.log(format!("读取配置失败：{e}")),
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .filter(|path| !path.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.dispatch_push(dropped);
        }
    }

    fn dispatch_push(&mut self, paths: Vec<PathBuf>) {
        if self.mode != StartMode::Server {
            self.banner = Some(("切换到「服务端」模式后才能投送".into(), true));
            return;
        }
        if self.running.is_none() {
            self.banner = Some(("请先启动服务".into(), true));
            return;
        }
        if self.clients.is_empty() {
            self.banner = Some(("没有在线客户端；请让对端以客户端模式连接".into(), true));
            return;
        }

        if let Some(id) = self.push_target.clone() {
            if self.clients.iter().any(|c| c.id == id) {
                self.banner = None;
                self.server.push_files(&id, paths);
                return;
            }
            self.push_target = None;
        }

        if self.clients.len() == 1 {
            let id = self.clients[0].id.clone();
            self.push_target = Some(id.clone());
            self.banner = None;
            self.server.push_files(&id, paths);
            return;
        }

        // 多个客户端且没有选中：拖入后弹窗选择接收方。
        self.banner = None;
        self.pending_drop = Some(paths);
    }

    fn handle_client_event(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Status(status) => {
                match &status {
                    ClientStatus::Connected => self.log("已连接到服务器"),
                    ClientStatus::Failed(error) => {
                        self.log(format!("客户端连接失败：{error}"));
                        self.banner = Some((error.clone(), true));
                    }
                    ClientStatus::Reconnecting { attempt, error } => {
                        self.log(format!("连接中断（第 {attempt} 次重连）：{error}"));
                    }
                    ClientStatus::Disconnected => self.log("客户端已断开"),
                    ClientStatus::Connecting => {}
                }
                self.client_status = status;
            }
            ClientEvent::BatchStarted {
                batch_id,
                name,
                total_files,
                total_bytes,
            } => {
                self.log(format!(
                    "开始接收「{name}」（{total_files} 个文件，{}）",
                    human_size(total_bytes)
                ));
                self.activity_start(batch_id, name, total_bytes, false, Some("服务器".into()));
            }
            ClientEvent::BatchProgress {
                batch_id,
                done,
                total_bytes,
            } => {
                self.activity_progress(&batch_id, done, total_bytes);
            }
            ClientEvent::BatchFinished { batch_id, bytes } => {
                self.log(format!("接收完成（{}）", human_size(bytes)));
                self.activity_finish(&batch_id, Some(bytes));
            }
            ClientEvent::BatchFailed {
                batch_id,
                error,
                done,
            } => {
                self.log(format!("接收失败：{error}"));
                self.activity_fail(&batch_id, error, Some(done), false);
            }
        }

        if self.activity.len() > 50 {
            self.activity.drain(..self.activity.len() - 50);
        }
    }

    fn connect_client(&mut self) {
        self.banner = None;

        let raw_url = self.client_server_url.trim();
        if raw_url.is_empty() {
            self.banner = Some(("请填写服务器地址".into(), true));
            return;
        }
        let base_url = match crate::client::normalize_server_url(raw_url) {
            Ok(url) => url.to_string(),
            Err(e) => {
                self.banner = Some((e, true));
                return;
            }
        };

        let username = self.client_username.trim().to_string();
        if username.is_empty() {
            self.banner = Some(("请填写用户名".into(), true));
            return;
        }
        if self.client_password.is_empty() {
            self.banner = Some(("请填写密码".into(), true));
            return;
        }

        let receive_dir = PathBuf::from(self.client_receive_dir_input.trim());
        if receive_dir.as_os_str().is_empty() {
            self.banner = Some(("请选择接收目录".into(), true));
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&receive_dir) {
            self.banner = Some((format!("无法创建接收目录：{e}"), true));
            return;
        }
        let receive_dir = match receive_dir.canonicalize() {
            Ok(path) => path,
            Err(e) => {
                self.banner = Some((format!("接收目录不可用：{e}"), true));
                return;
            }
        };

        let mut config = match Config::load() {
            Ok(config) => config,
            Err(e) => {
                self.banner = Some((format!("读取配置失败：{e}"), true));
                return;
            }
        };
        config.client.ensure_identity();
        config.client.server_url = base_url.clone();
        config.client.username = username.clone();
        config.client.display_name = self.client_display_name.trim().to_string();
        if config.client.display_name.is_empty() {
            config.client.display_name = crate::net::hostname();
        }
        config.client.receive_dir = Some(receive_dir.clone());
        if let Err(e) = config.save() {
            // 保存失败不阻止连接，但要让用户知道下次不会记住设置。
            self.log(format!("保存客户端设置失败：{e}"));
        }

        self.client_server_url = base_url.clone();
        self.client_username = username.clone();
        self.client_display_name = config.client.display_name.clone();
        self.client_receive_dir_input = receive_dir.display().to_string();
        let password = std::mem::take(&mut self.client_password);
        self.client_password_visible = false;

        self.client.connect(client::ClientRunSettings {
            base_url,
            username,
            password,
            client_id: config.client.client_id,
            display_name: config.client.display_name,
            receive_dir,
        });
        self.client_status = ClientStatus::Connecting;
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        // 先把命令收干净再处理：`try_recv` 借着 `self.tray`，
        // 而下面的分支要 `&mut self`（start/stop），边收边处理借用冲突。
        let mut commands = Vec::new();
        if let Some(tray) = &self.tray {
            while let Ok(command) = tray.events.try_recv() {
                commands.push(command);
            }
        }

        for command in commands {
            match command {
                TrayCommand::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCommand::OpenBrowser => self.open_browser(ctx),
                TrayCommand::ToggleServer => {
                    if self.mode == StartMode::Client {
                        self.switch_mode(StartMode::Server);
                    }
                    if self.running.is_some() {
                        self.stop();
                    } else {
                        self.start();
                    }
                }
                TrayCommand::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn open_browser(&self, ctx: &egui::Context) {
        if let Some(url) = self.primary_url() {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

impl eframe::App for GuiApp {
    /// 在每次绘制之前跑，**窗口隐藏时也会跑**（只要有 request_repaint）。
    /// 所以网络事件、托盘菜单这些跟画面无关的事情都放这里。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        loop {
            match self.server.events.try_recv() {
                Ok(event) => self.handle_server_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        loop {
            match self.client.events.try_recv() {
                Ok(event) => self.handle_client_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // 服务端的 tracing 日志（接收完成、清理任务、限流告警……）
        while let Ok(line) = self.log_rx.try_recv() {
            self.push_log_line(line);
        }

        self.handle_tray(ctx);
        self.handle_dropped_files(ctx);

        // 点窗口的 X：有托盘就藏起来（服务继续跑），没有托盘才真退出。
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        // 有传输在进行时按 10Hz 重绘，进度条才会动。
        // 闲着的时候不主动重绘，免得白烧 CPU。
        if self
            .activity
            .iter()
            .any(|a| a.state == ActivityState::Running)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG_TOP))
            .show(ui, |ui| {
                theme::paint_background(ui);
                egui::Frame::new()
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add_space(2.0);
                                self.draw_header(ui);
                                ui.add_space(14.0);

                                if ui.available_width() >= 820.0 {
                                    ui.columns(2, |columns| {
                                        self.draw_left_column(&mut columns[0], &ctx);
                                        self.draw_right_column(&mut columns[1], &ctx);
                                    });
                                } else {
                                    self.draw_left_column(ui, &ctx);
                                    ui.add_space(14.0);
                                    self.draw_right_column(ui, &ctx);
                                }

                                ui.add_space(14.0);
                                self.draw_footer(ui);
                                ui.add_space(6.0);
                            });
                    });
            });

        self.draw_receiver_picker(&ctx);
    }
}

// ─────────────────────────── 绘制 ───────────────────────────

impl GuiApp {
    fn draw_header(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(18))
            .shadow(egui::epaint::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 0,
                color: egui::Color32::from_black_alpha(80),
            })
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    theme::logo(ui, 42.0);
                    ui.add_space(11.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Wiretoss")
                                .size(21.0)
                                .strong()
                                .color(theme::TEXT),
                        );
                        ui.label(
                            egui::RichText::new("局域网文件快传")
                                .size(11.5)
                                .color(theme::TEXT_DIM),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (text, color) = self.status_badge();
                        theme::pill(ui, &text, color);
                    });
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let selected = match self.mode {
                        StartMode::Server => 0,
                        StartMode::Client => 1,
                    };
                    if let Some(chosen) = theme::segmented(ui, &["服务端", "客户端"], selected)
                    {
                        let next = if chosen == 0 {
                            StartMode::Server
                        } else {
                            StartMode::Client
                        };
                        if next != self.mode {
                            self.switch_mode(next);
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let hint = match self.mode {
                            StartMode::Server => "对端用浏览器打开即可互传",
                            StartMode::Client => "连接后自动接收投送",
                        };
                        ui.label(
                            egui::RichText::new(hint)
                                .size(11.0)
                                .color(theme::TEXT_FAINT),
                        );
                    });
                });

                if let Some((message, is_error)) = &self.banner {
                    let color = if *is_error { theme::DANGER } else { theme::OK };
                    ui.add_space(11.0);
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgba_unmultiplied(
                            color.r(),
                            color.g(),
                            color.b(),
                            24,
                        ))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(
                                color.r(),
                                color.g(),
                                color.b(),
                                80,
                            ),
                        ))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(message).size(12.0).color(color));
                        });
                }
            });
    }

    fn status_badge(&self) -> (String, egui::Color32) {
        match self.mode {
            StartMode::Server => match &self.running {
                Some(running) => (format!("● 运行中 · 端口 {}", running.port), theme::OK),
                None => ("● 已停止".to_string(), theme::TEXT_FAINT),
            },
            StartMode::Client => match &self.client_status {
                ClientStatus::Connecting => ("● 连接中".to_string(), theme::WARN),
                ClientStatus::Connected => ("● 已连接".to_string(), theme::OK),
                ClientStatus::Reconnecting { .. } => ("● 重连中".to_string(), theme::WARN),
                ClientStatus::Disconnected => ("● 未连接".to_string(), theme::TEXT_FAINT),
                ClientStatus::Failed(_) => ("● 连接失败".to_string(), theme::DANGER),
            },
        }
    }

    fn draw_left_column(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.mode {
            StartMode::Server => {
                self.draw_settings(ui, ctx);
                ui.add_space(14.0);
                // 在线客户端是投送的入口，放在设置下面，避免每次都要往下滚。
                self.draw_clients(ui, ctx);
            }
            StartMode::Client => self.draw_client_settings(ui, ctx),
        }
    }

    fn draw_right_column(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.mode == StartMode::Server {
            self.draw_access(ui, ctx);
            ui.add_space(14.0);
        }
        self.draw_activity(ui);
        ui.add_space(14.0);
        self.draw_log(ui);
    }

    fn draw_footer(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Wiretoss v{}", env!("CARGO_PKG_VERSION")))
                    .size(10.5)
                    .color(theme::TEXT_FAINT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new("数据只在局域网内传输")
                        .size(10.5)
                        .color(theme::TEXT_FAINT),
                );
            });
        });
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let running = self.running.is_some();

        theme::card(
            ui,
            "服务设置",
            Some("共享目录、账号和监听端口"),
            |ui| {
                ui.add_enabled_ui(!running, |ui| {
                    theme::field_label(ui, "共享目录");
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 84.0).max(120.0);
                        ui.add_sized(
                            [width, 30.0],
                            egui::TextEdit::singleline(&mut self.root_input),
                        );
                        if theme::secondary_button(ui, "选择…").clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.root_input = dir.display().to_string();
                            }
                        }
                    });

                    ui.add_space(8.0);
                    theme::field_label(ui, "用户名");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.username).desired_width(f32::INFINITY),
                    );

                    ui.add_space(8.0);
                    theme::field_label(ui, "密码");
                    ui.horizontal(|ui| {
                        let hint = if self.has_password {
                            "留空则不修改"
                        } else if self.allow_guest {
                            "可留空（只读）"
                        } else {
                            "必须设置一个"
                        };
                        let width = (ui.available_width() - 170.0).max(110.0);
                        ui.add_sized(
                            [width, 30.0],
                            egui::TextEdit::singleline(&mut self.password_input)
                                .password(!self.password_visible)
                                .hint_text(hint),
                        );
                        ui.toggle_value(&mut self.password_visible, "显示");
                        if theme::secondary_button(ui, "随机生成").clicked() {
                            self.password_input =
                                config::generate_password(config::DEFAULT_PASSWORD_LEN);
                            self.password_visible = true;
                        }
                    });
                    ui.add_space(4.0);
                    theme::hint(
                        ui,
                        if self.has_password {
                            "密码以 Argon2 哈希保存，界面上无法回显；要改就输入新的。"
                        } else {
                            "配置里只存哈希、不存明文，所以设好之后请记牢。"
                        },
                    );

                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.allow_guest,
                        "允许无账号只读访问（浏览、下载）",
                    );
                    theme::hint(
                        ui,
                        if self.allow_guest {
                            "未登录访客只能浏览和下载；上传、删除、投送仍需登录。局域网内任何人都能看到共享目录。"
                        } else {
                            "关闭时，所有访问都需要登录。"
                        },
                    );

                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut self.https,
                        "启用 HTTPS（桌面浏览器保存到本机目录）",
                    );
                    theme::hint(
                        ui,
                        "HTTP 仍保留给手机；局域网电脑首次使用要安装并信任 certs/ca.pem。",
                    );
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("HTTPS 端口")
                                .size(12.0)
                                .color(theme::TEXT_DIM),
                        );
                        ui.add_enabled(
                            self.https,
                            egui::DragValue::new(&mut self.https_port).range(1..=65535),
                        );
                    });

                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(
                        egui::RichText::new("高级设置")
                            .size(12.0)
                            .color(theme::TEXT_DIM),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        egui::Grid::new("advanced")
                            .num_columns(2)
                            .spacing([14.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("端口")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                                ui.add(egui::DragValue::new(&mut self.port).range(1..=65535));
                                ui.end_row();

                                ui.label(
                                    egui::RichText::new("监听地址")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.bind).desired_width(160.0),
                                );
                                ui.end_row();

                                ui.label("");
                                ui.checkbox(
                                    &mut self.advertise_mdns,
                                    "广播 mDNS（对端可用「主机名.local」访问）",
                                );
                                ui.end_row();
                            });
                    });
                });

                theme::divider(ui);
                ui.horizontal(|ui| {
                    if running {
                        if theme::danger_button(ui, "停止服务").clicked() {
                            self.stop();
                        }
                    } else if theme::primary_button(ui, "启动服务").clicked() {
                        self.start();
                    }

                    ui.add_enabled_ui(running, |ui| {
                        if theme::secondary_button(ui, "打开浏览器").clicked() {
                            self.open_browser(ctx);
                        }
                    });

                    if running && theme::ghost_button(ui, "隐藏窗口").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                });

                if running {
                    ui.add_space(4.0);
                    theme::hint(ui, "服务运行中，设置已锁定；要改先停止服务。");
                }
            },
        );
    }

    fn draw_access(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(running) = &self.running else { return };
        let urls = running.urls.clone();
        let https_urls = running.https_urls.clone();
        let mdns_ok = running.mdns_ok;
        let primary = self.primary_url();

        theme::card(
            ui,
            "扫码访问",
            Some("手机扫码，或把地址发给同一局域网里的设备"),
            |ui| {
                ui.horizontal_top(|ui| {
                    if let Some(url) = &primary {
                        egui::Frame::new()
                            .fill(egui::Color32::WHITE)
                            .corner_radius(egui::CornerRadius::same(14))
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                draw_qr(ui, url, 150.0);
                            });
                        ui.add_space(16.0);
                    }

                    ui.vertical(|ui| {
                        for (index, url) in urls.iter().enumerate() {
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 8))
                                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                                .corner_radius(egui::CornerRadius::same(9))
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let color = if index == 0 {
                                            theme::TEXT
                                        } else {
                                            theme::TEXT_DIM
                                        };
                                        // 给「复制」留出固定宽度：长 URL 直接截断，
                                        // 否则按钮会压在地址上。
                                        let width = (ui.available_width() - 50.0).max(70.0);
                                        ui.add_sized(
                                            [width, 18.0],
                                            egui::Label::new(
                                                egui::RichText::new(url)
                                                    .monospace()
                                                    .size(11.5)
                                                    .color(color),
                                            )
                                            .truncate(),
                                        );
                                        if ui.small_button("复制").clicked() {
                                            ctx.copy_text(url.clone());
                                        }
                                    });
                                });
                            ui.add_space(5.0);
                        }

                        if !mdns_ok {
                            theme::hint(ui, "mDNS 未启用，对端请用上面的 IP 地址");
                        }
                        ui.add_space(2.0);
                        theme::hint(ui, "对端登录用上面设置的账号密码。");

                        if !https_urls.is_empty() {
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new("桌面浏览器 · 保存到本机目录")
                                    .size(12.0)
                                    .strong()
                                    .color(theme::TEXT),
                            );
                            ui.add_space(4.0);
                            for url in &https_urls {
                                egui::Frame::new()
                                    .fill(egui::Color32::from_rgba_unmultiplied(255, 255, 255, 8))
                                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                                    .corner_radius(egui::CornerRadius::same(9))
                                    .inner_margin(egui::Margin::symmetric(10, 6))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let width = (ui.available_width() - 50.0).max(70.0);
                                            ui.add_sized(
                                                [width, 18.0],
                                                egui::Label::new(
                                                    egui::RichText::new(url)
                                                        .monospace()
                                                        .size(11.5)
                                                        .color(theme::TEXT_DIM),
                                                )
                                                .truncate(),
                                            );
                                            if ui.small_button("复制").clicked() {
                                                ctx.copy_text(url.clone());
                                            }
                                        });
                                    });
                                ui.add_space(5.0);
                            }
                            if let Ok(ca) = crate::tls::ca_path() {
                                let ca_text = ca.display().to_string();
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("CA 证书  {ca_text}"))
                                            .size(10.5)
                                            .color(theme::TEXT_FAINT),
                                    );
                                    if ui.small_button("复制").clicked() {
                                        ctx.copy_text(ca_text);
                                    }
                                });
                            }
                            theme::hint(
                                ui,
                                "首次使用需在浏览器电脑上安装并信任 CA 证书；网页里也可以下载。",
                            );
                        }
                    });
                });
            },
        );
    }

    fn draw_clients(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.running.is_none() {
            return;
        }

        theme::card(
            ui,
            &format!("在线设备 · {}", self.clients.len()),
            Some("点选接收方，然后把文件拖进窗口任意位置"),
            |ui| {
                if self.clients.is_empty() {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui
                            .allocate_exact_size(egui::Vec2::new(34.0, 34.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            egui::CornerRadius::same(10),
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10),
                        );
                        ui.painter()
                            .circle_filled(rect.center(), 4.0, theme::ACCENT);
                        ui.add_space(2.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("还没有设备连接")
                                    .size(13.0)
                                    .color(theme::TEXT),
                            );
                            theme::hint(ui, "让另一台电脑切到「客户端」模式并连接。");
                        });
                    });
                } else {
                    let mut new_target: Option<Option<String>> = None;
                    for client in &self.clients {
                        let selected = self.push_target.as_deref() == Some(client.id.as_str());
                        let label = format!(
                            "{}   ·   {}   ·   已连接 {}",
                            client.name,
                            client.ip,
                            human_duration(client.connected_at.elapsed())
                        );
                        if ui
                            .selectable_label(selected, egui::RichText::new(label).size(12.0))
                            .clicked()
                        {
                            new_target = Some(if selected {
                                None
                            } else {
                                Some(client.id.clone())
                            });
                        }
                    }
                    if let Some(target) = new_target {
                        self.push_target = target;
                    }
                }

                // 拖拽悬停提示。落点坐标在系统拖拽期间不可用，所以这里只提示
                // 当前点选的目标，不尝试判断鼠标在哪一行。
                if ctx.input(|input| !input.raw.hovered_files.is_empty()) {
                    ui.add_space(5.0);
                    if self.clients.is_empty() {
                        ui.label(
                            egui::RichText::new("当前没有在线客户端，无法投送")
                                .size(12.0)
                                .color(theme::DANGER),
                        );
                    } else if let Some(id) = &self.push_target {
                        let name = self
                            .clients
                            .iter()
                            .find(|c| &c.id == id)
                            .map(|c| c.name.clone())
                            .unwrap_or_default();
                        ui.label(
                            egui::RichText::new(format!("松开将发送给「{name}」"))
                                .size(12.0)
                                .color(theme::ACCENT),
                        );
                    } else if self.clients.len() > 1 {
                        ui.label(
                            egui::RichText::new("松开后选择接收方")
                                .size(12.0)
                                .color(theme::WARN),
                        );
                    }
                }
            },
        );
    }

    fn draw_receiver_picker(&mut self, ctx: &egui::Context) {
        let Some(paths) = self.pending_drop.take() else {
            return;
        };

        let mut chosen: Option<String> = None;
        let mut cancel = false;
        egui::Window::new("选择接收方")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("{} 个文件/文件夹，投送给：", paths.len()));
                ui.add_space(6.0);
                if self.clients.is_empty() {
                    ui.label("没有在线客户端");
                }
                for client in &self.clients {
                    if ui
                        .button(format!("{}  ({})", client.name, client.ip))
                        .clicked()
                    {
                        chosen = Some(client.id.clone());
                    }
                }
                ui.add_space(6.0);
                if ui.button("取消").clicked() {
                    cancel = true;
                }
            });

        if let Some(id) = chosen {
            self.server.push_files(&id, paths);
        } else if !cancel {
            // 窗口还开着，下一帧继续显示。
            self.pending_drop = Some(paths);
        }
    }

    fn draw_client_settings(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let connected = matches!(
            self.client_status,
            ClientStatus::Connecting | ClientStatus::Connected | ClientStatus::Reconnecting { .. }
        );

        theme::card(
            ui,
            "连接设置",
            Some("连接后自动接收投送，文件直接保存到接收目录"),
            |ui| {
                ui.add_enabled_ui(!connected, |ui| {
                    theme::field_label(ui, "服务器地址");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.client_server_url)
                            .desired_width(f32::INFINITY)
                            .hint_text("192.168.1.5:8080"),
                    );

                    ui.add_space(8.0);
                    theme::field_label(ui, "用户名");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.client_username)
                            .desired_width(f32::INFINITY),
                    );

                    ui.add_space(8.0);
                    theme::field_label(ui, "密码");
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 64.0).max(110.0);
                        ui.add_sized(
                            [width, 30.0],
                            egui::TextEdit::singleline(&mut self.client_password)
                                .password(!self.client_password_visible)
                                .hint_text("不保存，重连时需重新输入"),
                        );
                        ui.toggle_value(&mut self.client_password_visible, "显示");
                    });

                    ui.add_space(8.0);
                    theme::field_label(ui, "设备名称");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.client_display_name)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(3.0);
                    theme::hint(ui, "设备名称会显示在服务端的投送列表里。");

                    ui.add_space(8.0);
                    theme::field_label(ui, "接收目录");
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 84.0).max(120.0);
                        ui.add_sized(
                            [width, 30.0],
                            egui::TextEdit::singleline(&mut self.client_receive_dir_input),
                        );
                        if theme::secondary_button(ui, "选择…").clicked() {
                            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                self.client_receive_dir_input = dir.display().to_string();
                            }
                        }
                    });
                });

                theme::divider(ui);
                ui.horizontal(|ui| {
                    if connected {
                        if theme::danger_button(ui, "断开").clicked() {
                            self.client.disconnect();
                            self.client_status = ClientStatus::Disconnected;
                        }
                    } else if theme::primary_button(ui, "连接").clicked() {
                        self.connect_client();
                    }

                    let (text, color) = match &self.client_status {
                        ClientStatus::Connecting => ("连接中…".to_string(), theme::WARN),
                        ClientStatus::Connected => ("● 已连接，等待投送".to_string(), theme::OK),
                        ClientStatus::Reconnecting { attempt, error } => {
                            (format!("重连中（第 {attempt} 次）：{error}"), theme::WARN)
                        }
                        ClientStatus::Disconnected => ("未连接".to_string(), theme::TEXT_FAINT),
                        ClientStatus::Failed(error) => {
                            (format!("连接失败：{error}"), theme::DANGER)
                        }
                    };
                    ui.label(egui::RichText::new(text).size(12.0).color(color));
                });
            },
        );
    }

    fn draw_activity(&mut self, ui: &mut egui::Ui) {
        theme::card(ui, "传输活动", Some("最近 8 条"), |ui| {
            if self.activity.is_empty() {
                theme::hint(ui, "还没有传输记录。");
                return;
            }

            for index in 0..self.activity.len().min(8) {
                let row = &self.activity[self.activity.len() - 1 - index];
                ui.horizontal(|ui| {
                    let (mark, color) = match &row.state {
                        ActivityState::Running => {
                            (if row.outgoing { "↑" } else { "↓" }, theme::ACCENT)
                        }
                        ActivityState::Done => ("✓", theme::OK),
                        ActivityState::Failed(_) => ("✕", theme::DANGER),
                    };
                    ui.label(egui::RichText::new(mark).size(13.0).strong().color(color));

                    let label = match &row.peer {
                        Some(peer) if row.outgoing => format!("{} → {}", row.name, peer),
                        Some(peer) => format!("{} ← {}", row.name, peer),
                        None => row.name.clone(),
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::TEXT))
                            .truncate(),
                    );

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| match &row.state {
                            ActivityState::Failed(error) => {
                                ui.label(
                                    egui::RichText::new(truncate(error, 40))
                                        .size(11.5)
                                        .color(theme::DANGER),
                                );
                            }
                            ActivityState::Done => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "完成 · {}",
                                        human_size(row.total)
                                    ))
                                    .size(11.0)
                                    .color(theme::TEXT_FAINT),
                                );
                            }
                            ActivityState::Running => {
                                let frac = if row.total > 0 {
                                    row.done as f32 / row.total as f32
                                } else {
                                    0.0
                                };
                                ui.add(
                                    egui::ProgressBar::new(frac)
                                        .desired_width(130.0)
                                        .fill(theme::ACCENT)
                                        .corner_radius(4)
                                        .text(format!(
                                            "{} / {}",
                                            human_size(row.done),
                                            human_size(row.total)
                                        )),
                                );
                            }
                        },
                    );
                });
            }
        });
    }

    fn draw_log(&mut self, ui: &mut egui::Ui) {
        theme::card(
            ui,
            "运行日志",
            Some(&format!("{} 条", self.log.len())),
            |ui| {
                if self.log.is_empty() {
                    theme::hint(ui, "暂无日志。");
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.label(
                                egui::RichText::new(line)
                                    .monospace()
                                    .size(11.0)
                                    .color(theme::TEXT_DIM),
                            );
                        }
                    });
            },
        );
    }
}

// ─────────────────────────── 辅助 ───────────────────────────

/// 用 painter 直接画二维码模块，不生成纹理也不用 image crate。
/// 每个模块是一个实心方块，任意缩放都是硬边，不会糊。
fn draw_qr(ui: &mut egui::Ui, data: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else {
        return;
    };
    // `to_vec`/`into_vec` 在 0.14 里已废弃，用 to_colors。
    let modules = code.to_colors();
    let count = code.width();
    if count == 0 {
        return;
    }

    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();

    // 白底 + 四周留白：二维码规范要求至少 4 个模块的静默区，
    // 缺了它很多扫描器会识别不出来。
    painter.rect_filled(rect, 6.0, egui::Color32::WHITE);

    const QUIET: f32 = 2.0;
    let cell = size / (count as f32 + QUIET * 2.0);
    let origin = rect.min + egui::vec2(cell * QUIET, cell * QUIET);

    for y in 0..count {
        for x in 0..count {
            if !matches!(modules[y * count + x], qrcode::Color::Dark) {
                continue;
            }
            let min = origin + egui::vec2(x as f32 * cell, y as f32 * cell);
            // 多画半个像素，避免相邻模块之间出现发丝一样的白缝。
            painter.rect_filled(
                egui::Rect::from_min_size(min, egui::vec2(cell + 0.5, cell + 0.5)),
                0.0,
                egui::Color32::BLACK,
            );
        }
    }
}

/// egui 自带的字体只有拉丁字形，中文会渲染成一个个方块。
/// 从系统里找一份中文字体挂成**后备**字体：拉丁字符仍用 egui 自带的那套
/// （更清爽），只有它渲染不了的才落到系统字体上。
/// 系统里可能存在的字体，按优先级排列。
///
/// 不把字体打进二进制是因为一份完整中文字体有十几 MB —— 比整个程序还大，
/// 而两个目标平台的系统里本来就都有。
const CJK_FONT_CANDIDATES: &[&str] = &[
    // macOS
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    // Windows
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyh.ttf",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
];

/// 从系统里找一份能用的中文字体，返回 (路径, 字节)。
fn load_cjk_font() -> Option<(&'static str, Vec<u8>)> {
    CJK_FONT_CANDIDATES
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| (*path, bytes)))
}

fn install_cjk_font(ctx: &egui::Context) {
    let Some((path, bytes)) = load_cjk_font() else {
        tracing::warn!("没找到可用的中文字体，界面上的中文可能显示为方块");
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    // `.ttc` 是字体集合，index 0 取第一个 face。
    fonts.font_data.insert(
        "system-cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("system-cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    tracing::info!("已加载中文字体：{path}");
}

fn human_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{} 小时", secs / 3600)
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cjk_font_exists_on_this_platform() {
        // 界面文案全是中文，而 egui 自带字体没有 CJK 字形。
        // 找不到系统字体的话整个界面会变成一排排方块，
        // 偏偏那种"能跑但没法用"的故障最容易被忽略，所以用测试钉住。
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            return; // Linux 上不保证装了中文字体
        }
        assert!(
            CJK_FONT_CANDIDATES
                .iter()
                .any(|p| std::path::Path::new(p).exists()),
            "本机没找到任何中文字体，界面会显示成一堆方块"
        );
    }

    #[test]
    fn cjk_font_actually_provides_glyphs() {
        // 上一条测试只保证文件存在；这一条要求字体真的能解析出中文字形。
        // 光有文件而解析失败的话，界面上照样是一排方块 ——
        // 这种"看起来装好了其实没用"的情况必须由测试挡住。
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            return;
        }
        let Some((path, bytes)) = load_cjk_font() else {
            panic!("找不到中文字体");
        };

        let mut definitions = egui::FontDefinitions::default();
        definitions.font_data.insert(
            "system-cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        definitions
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .push("system-cjk".to_owned());

        let mut fonts = egui::epaint::text::Fonts::new(Default::default(), definitions);
        assert!(
            fonts.has_glyphs(&egui::FontId::proportional(14.0), "中文文件名"),
            "{path} 解析不出中文字形"
        );
    }

    #[test]
    fn human_size_is_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(human_size(1024 * 1024 * 1024 * 5), "5.0 GB");
    }

    #[test]
    fn truncate_keeps_char_boundaries() {
        assert_eq!(truncate("abc", 10), "abc");
        // 中文是多字节的，按字节截断会 panic
        assert_eq!(truncate("中文文件名很长很长", 4), "中文文…");
    }
}
