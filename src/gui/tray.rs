//! 菜单栏（macOS）/ 托盘（Windows）图标。
//!
//! 关掉窗口只是把窗口藏起来，服务继续在后台跑 —— 所以必须有个地方能把窗口
//! 叫回来、能真正退出，这就是这个图标存在的全部理由。

use std::sync::mpsc::{channel, Receiver};

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayCommand {
    Show,
    OpenBrowser,
    ToggleServer,
    Quit,
}

pub struct Tray {
    /// 必须一直持有：drop 掉图标就从菜单栏消失了。
    _icon: TrayIcon,
    pub events: Receiver<TrayCommand>,
}

/// 建不起来时返回 `None`，调用方要据此把「关闭窗口」降级成真退出 ——
/// 否则窗口藏起来之后用户再也没有办法把它叫回来。
pub fn create(ctx: &egui::Context) -> Option<Tray> {
    let show = MenuItem::new("显示窗口", true, None);
    let browse = MenuItem::new("打开浏览器", true, None);
    let toggle = MenuItem::new("启动 / 停止服务", true, None);
    let quit = MenuItem::new("退出 Wiretoss", true, None);

    let menu = Menu::new();
    menu.append_items(&[&show, &browse, &toggle, &quit]).ok()?;

    let (tx, events) = channel::<TrayCommand>();

    // 菜单事件是进程级回调，拿不到 GuiApp，所以用 channel 把命令送回去。
    // 每次都 request_repaint：窗口藏起来时 egui 不会自己跑，
    // 不叫醒它的话点菜单会毫无反应。
    let ids = [
        show.id().clone(),
        browse.id().clone(),
        toggle.id().clone(),
        quit.id().clone(),
    ];
    let ctx_for_menu = ctx.clone();
    let tx_for_menu = tx.clone();
    std::thread::Builder::new()
        .name("wiretoss-tray-menu".into())
        .spawn(move || {
            let receiver = MenuEvent::receiver();
            while let Ok(event) = receiver.recv() {
                let command = if event.id == ids[0] {
                    Some(TrayCommand::Show)
                } else if event.id == ids[1] {
                    Some(TrayCommand::OpenBrowser)
                } else if event.id == ids[2] {
                    Some(TrayCommand::ToggleServer)
                } else if event.id == ids[3] {
                    Some(TrayCommand::Quit)
                } else {
                    None
                };
                if let Some(command) = command {
                    if tx_for_menu.send(command).is_err() {
                        break;
                    }
                    ctx_for_menu.request_repaint();
                }
            }
        })
        .ok()?;

    // 左键单击图标直接把窗口叫回来。
    let ctx_for_icon = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            if tx.send(TrayCommand::Show).is_ok() {
                ctx_for_icon.request_repaint();
            }
        }
    }));

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Wiretoss");

    // macOS 上必须用模板图：系统会按菜单栏的明暗自动反色，浅色深色下都看得清。
    // 这个方法带 `#[cfg(target_os = "macos")]`，其他平台只能用普通图标，
    // 不加 cfg 门的话 Windows 直接编译不过。
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(build_icon());
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(build_icon());

    let icon = builder.build().ok()?;

    Some(Tray {
        _icon: icon,
        events,
    })
}

/// 菜单栏图标：两个反向箭头。
///
/// 程序化画出来而不是读图片文件 —— 菜单栏图标必须跟着系统主题反色
/// （所以是只有 alpha 有意义的纯黑模板图），而加载外部图片既要解码器
/// 又没法在交叉编译时保证资源路径对得上。这点形状不值得引入这些麻烦。
const ICON_SIZE: usize = 44; // 22pt @2x

fn build_icon() -> tray_icon::Icon {
    // vec! 初始化的就是全 0，alpha 本来就是 0（全透明），不用再清一遍。
    let mut rgba = vec![0u8; ICON_SIZE * ICON_SIZE * 4];

    // 上：向右的箭头
    rect(&mut rgba, 5, 12, 28, 17);
    triangle(&mut rgba, (26, 7), (38, 14), (26, 21));

    // 下：向左的箭头
    rect(&mut rgba, 16, 28, 39, 33);
    triangle(&mut rgba, (18, 23), (6, 30), (18, 37));

    tray_icon::Icon::from_rgba(rgba, ICON_SIZE as u32, ICON_SIZE as u32)
        .expect("图标尺寸固定，不会失败")
}

fn put(rgba: &mut [u8], x: i32, y: i32) {
    if x < 0 || y < 0 || x >= ICON_SIZE as i32 || y >= ICON_SIZE as i32 {
        return;
    }
    rgba[((y as usize) * ICON_SIZE + x as usize) * 4 + 3] = 0xff;
}

fn rect(rgba: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32) {
    for y in y0..y1 {
        for x in x0..x1 {
            put(rgba, x, y);
        }
    }
}

/// 重心法填充三角形。菜单栏图标尺寸很小，没必要上抗锯齿。
fn triangle(rgba: &mut [u8], a: (i32, i32), b: (i32, i32), c: (i32, i32)) {
    let cross = |p: (i32, i32), q: (i32, i32), r: (i32, i32)| {
        (p.0 - r.0) * (q.1 - r.1) - (q.0 - r.0) * (p.1 - r.1)
    };

    let min_x = a.0.min(b.0).min(c.0);
    let max_x = a.0.max(b.0).max(c.0);
    let min_y = a.1.min(b.1).min(c.1);
    let max_y = a.1.max(b.1).max(c.1);

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = (x, y);
            let (d1, d2, d3) = (cross(p, a, b), cross(p, b, c), cross(p, c, a));
            let has_negative = d1 < 0 || d2 < 0 || d3 < 0;
            let has_positive = d1 > 0 || d2 > 0 || d3 > 0;
            if !(has_negative && has_positive) {
                put(rgba, x, y);
            }
        }
    }
}
