// 必须是二进制 crate 的根（main.rs），放 lib.rs 里不起作用。
// 只在 release 下生效：debug 构建保留控制台，方便开发时看日志。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use wiretoss::config::{self, Config, StartMode};
use wiretoss::error::{AppError, AppResult};
use wiretoss::state::AppState;
use wiretoss::{net, router};

#[derive(Parser, Debug)]
#[command(
    name = "wiretoss",
    version,
    about = "Mac <-> Windows 局域网文件快传：浏览器互传，或客户端模式接收隔空投送",
    long_about = "默认打开图形控制面板。加 --headless 则不开窗口，直接在终端里跑服务 \
                  （适合脚本、开机自启、CI），此时下面那些参数才生效；\
                  加 --client 则以客户端模式启动，连接服务端并自动接收投送。"
)]
struct Args {
    /// 不开图形界面，直接在终端里跑服务
    #[arg(long)]
    headless: bool,

    /// 以客户端模式启动图形界面：连接服务端并自动接收投送
    #[arg(long, conflicts_with = "headless")]
    client: bool,

    /// 监听端口
    #[arg(short, long)]
    port: Option<u16>,

    /// 监听地址。默认 0.0.0.0，即局域网内所有机器都能访问
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,

    /// 共享目录（默认 ~/Wiretoss）
    #[arg(short, long)]
    root: Option<PathBuf>,

    /// 对端登录用的用户名（首次运行时可用，也可用来改已有设置）
    #[arg(long)]
    user: Option<String>,

    /// 对端登录用的密码（首次运行时可用，也可用来改已有设置）
    #[arg(long)]
    password: Option<String>,

    /// 重新生成一个随机密码并打印
    #[arg(long)]
    reset_password: bool,

    /// 不广播 mDNS 服务
    #[arg(long)]
    no_mdns: bool,

    /// 不打印二维码
    #[arg(long)]
    no_qr: bool,
}

fn main() -> AppResult<()> {
    // 必须在任何线程起来之前抓，否则拿不到本地时区（见 logging 模块的说明）。
    wiretoss::logging::capture_local_offset();

    let args = Args::parse();

    if !args.headless {
        // 图形模式下 tracing 由窗口接管（见 gui::logging）：日志写进界面的
        // 日志面板，而不是终端。这里不能再初始化一次，否则全局 subscriber
        // 被占住，窗口就永远收不到日志了。
        let initial_mode = if args.client {
            Some(StartMode::Client)
        } else {
            None
        };
        return wiretoss::gui::run(initial_mode).map_err(|e| AppError::Internal(e.to_string()));
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wiretoss=info,tower_http=warn".into()),
        )
        .with_target(false)
        .with_timer(wiretoss::logging::timer())
        .init();

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run(args))
}

async fn run(args: Args) -> AppResult<()> {
    let (mut cfg, shown_password) = resolve_credentials(&args)?;

    // --root / --port 只在本次运行生效，刻意不写回配置文件：
    // 它们更像"这一次这么跑"，而配置文件存的是长期身份（用户名、口令哈希）。
    if let Some(root) = &args.root {
        cfg.root = root.clone();
    }
    if let Some(port) = args.port {
        cfg.port = port;
    }

    let https = cfg.https;
    let https_port = cfg.https_port;
    let state = AppState::new(cfg.clone())?;
    // 共享目录的外部变化（Finder、资源管理器、其他程序）也要让网页自动刷新。
    let _watch = wiretoss::watch::spawn(state.clone());

    let addr: SocketAddr = format!("{}:{}", args.bind, cfg.port)
        .parse()
        .map_err(|e| AppError::BadRequest(format!("监听地址无效：{e}")))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| AppError::Internal(format!("无法监听 {addr}：{e}")))?;
    let actual_port = listener.local_addr()?.port();

    // daemon 必须活到进程结束，drop 掉服务就从网络上消失了。
    let _mdns = if args.no_mdns {
        None
    } else {
        net::advertise(actual_port)
    };

    print_banner(&cfg, actual_port, shown_password.as_deref(), args.no_qr);
    if https {
        println!();
        println!("  HTTPS（桌面浏览器保存到本机目录）");
        for ip in net::lan_ipv4_addresses() {
            println!("    https://{ip}:{https_port}");
        }
        println!("    https://localhost:{https_port}");
        if let Ok(ca) = wiretoss::tls::ca_path() {
            println!("  CA 证书  {}", ca.display());
        }
    }

    // 后台定期回收：会话、下载券、以及被放弃的上传（连同它占的中转磁盘）。
    // 这是一个可能连续跑好几天的常驻服务，只在启动时清理是不够的。
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(600));
            ticker.tick().await; // 第一次立刻返回，跳过
            loop {
                ticker.tick().await;
                state.purge_stale();
            }
        });
    }

    let app = router::build_router(state);

    if https {
        if https_port == 0 || https_port == actual_port {
            return Err(AppError::BadRequest(
                "HTTPS 端口无效：不能为 0，也不能和 HTTP 端口相同".into(),
            ));
        }
        let tls_paths = wiretoss::tls::ensure()?;
        wiretoss::tls::install_crypto_provider();
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls_paths.cert, &tls_paths.key)
                .await
                .map_err(|e| AppError::Internal(format!("加载 HTTPS 证书失败：{e}")))?;

        let https_addr: SocketAddr = format!("{}:{}", args.bind, https_port)
            .parse()
            .map_err(|_| AppError::BadRequest("HTTPS 监听地址无效".into()))?;

        let http_server = axum::serve(
            listener,
            app.clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        );
        let https_server = axum_server::bind_rustls(https_addr, tls_config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>());

        tokio::select! {
            result = http_server => result.map_err(|e| AppError::Internal(e.to_string()))?,
            result = https_server => result.map_err(|e| AppError::Internal(e.to_string()))?,
        }
    } else {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(())
}

/// 决定本次运行使用哪份账号密码。
///
/// 首次运行和之后的运行是两条完全不同的路径：前者要引导用户把账号密码
/// 设成自己记得住的，后者不该再打扰他。
fn resolve_credentials(args: &Args) -> AppResult<(Config, Option<String>)> {
    if !Config::exists() {
        let (username, password) = first_run_setup(args)?;
        let cfg = Config::create(&username, &password)?;
        return Ok((cfg, Some(password)));
    }

    let mut cfg = Config::load()?;
    let mut dirty = false;

    if let Some(user) = &args.user {
        if cfg.username != *user {
            cfg.username = user.clone();
            dirty = true;
        }
    }

    // 密码优先级：--password > --reset-password > 沿用配置里已有的。
    let shown = if let Some(p) = &args.password {
        cfg.set_password(p)?;
        Some(p.clone())
    } else if args.reset_password {
        let p = config::generate_password(config::DEFAULT_PASSWORD_LEN);
        cfg.set_password(&p)?;
        Some(p)
    } else {
        if dirty {
            cfg.save()?;
        }
        None
    };

    Ok((cfg, shown))
}

/// 首次运行：确定对端登录用的用户名和密码。
///
/// 优先级是「命令行参数 > 交互式询问 > 自动生成」。只有在**终端**里才会询问——
/// 被管道或脚本调用时问了也没人答，只会把进程挂死。
fn first_run_setup(args: &Args) -> AppResult<(String, String)> {
    let default_user = args
        .user
        .clone()
        .unwrap_or_else(|| config::DEFAULT_USERNAME.to_string());
    let default_pass = args
        .password
        .clone()
        .unwrap_or_else(|| config::generate_password(config::DEFAULT_PASSWORD_LEN));

    // 参数给全了就是明确的非交互意图，别多问。
    if args.user.is_some() && args.password.is_some() {
        return Ok((default_user, default_pass));
    }
    if !std::io::stdin().is_terminal() {
        return Ok((default_user, default_pass));
    }

    println!();
    println!("  首次运行 —— 请设置对端浏览器登录用的账号。");
    println!("  直接回车即使用方括号里的默认值。");
    println!();

    let username = prompt_line("  用户名", &default_user)?;
    let password = if args.password.is_some() {
        default_pass
    } else {
        prompt_new_password(&default_pass)?
    };

    println!();
    println!("  已保存。以后想改，运行：");
    println!("    wiretoss --user 新用户名 --password 新密码");
    Ok((username, password))
}

fn prompt_line(label: &str, default: &str) -> AppResult<String> {
    print!("{label} [{default}]: ");
    std::io::stdout().flush()?;
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf)?;
    let value = buf.trim();
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value.to_string()
    })
}

/// 读一个新密码：不回显，并要求确认一次。
///
/// 确认这步不是啰嗦——配置里只存 Argon2 哈希，密码打错了就再也拿不回来，
/// 只能重设。宁可现在多敲一遍。
fn prompt_new_password(default: &str) -> AppResult<String> {
    println!("  密码   自己输入一个，或直接回车用随机生成的 [{default}]");
    print!("  密码: ");
    std::io::stdout().flush()?;
    let first = rpassword::read_password()?;
    let first = first.trim();

    if first.is_empty() {
        return Ok(default.to_string());
    }
    if first.chars().count() < 4 {
        return Err(AppError::BadRequest("密码至少 4 位".into()));
    }

    print!("  再输一次确认: ");
    std::io::stdout().flush()?;
    let again = rpassword::read_password()?;
    if again.trim() != first {
        return Err(AppError::BadRequest("两次输入不一致，请重新运行".into()));
    }
    Ok(first.to_string())
}

fn print_banner(cfg: &Config, port: u16, password: Option<&str>, no_qr: bool) {
    let host = net::hostname();
    let ips = net::lan_ipv4_addresses();

    println!();
    println!("  Wiretoss v{}", env!("CARGO_PKG_VERSION"));
    println!("  ─────────────────────────────────────────────");
    println!("  共享目录  {}", cfg.root.display());
    println!("  用户名    {}", cfg.username);
    if let Some(pw) = password {
        println!("  密码      {pw}");
        println!("            （请立即记下；之后不再显示，可用 --reset-password 重置）");
    } else {
        println!("  密码      （沿用已保存的密码，忘了就用 --reset-password 重置）");
    }
    println!();
    println!("  本机打开   http://localhost:{port}");

    let primary = ips.first();
    for ip in &ips {
        println!("  对端打开   http://{ip}:{port}");
    }
    println!("  或         http://{host}.local:{port}");

    if ips.is_empty() {
        println!();
        println!("  ⚠ 没有检测到局域网 IPv4 地址。若本机确实已联网，");
        println!("    请检查网卡是否拿到 IP（169.254.x.x 表示 DHCP 失败）。");
    }

    println!();
    println!("  Windows 首次运行会弹出防火墙授权框，必须勾选「专用网络」并允许，");
    println!("  否则对端连不上。");

    if !no_qr {
        if let Some(ip) = primary {
            let url = format!("http://{ip}:{port}");
            if let Some(qr) = net::qr_to_terminal(&url) {
                println!();
                println!("  手机扫码打开 {url}");
                for line in qr.lines() {
                    println!("  {line}");
                }
            }
        }
    }
    println!();
}
