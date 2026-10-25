//! 桌面控制面板的视觉主题。
//!
//! 和浏览器端共用同一套观感：深蓝黑底、蓝紫强调色、半透明卡片、细描边。
//! egui 没有 CSS，颜色和间距集中在这个文件里，draw_* 里只关心内容。

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle, Vec2,
};

pub const BG_TOP: Color32 = Color32::from_rgb(11, 16, 32);
pub const BG_BOTTOM: Color32 = Color32::from_rgb(7, 10, 21);
pub const CARD: Color32 = Color32::from_rgba_unmultiplied_const(21, 27, 46, 238);
pub const INPUT_BG: Color32 = Color32::from_rgb(9, 13, 26);
pub const BORDER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 23);
pub const BORDER_STRONG: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 43);

pub const TEXT: Color32 = Color32::from_rgb(232, 236, 245);
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 147, 168);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(94, 103, 128);

pub const ACCENT: Color32 = Color32::from_rgb(91, 140, 255);
pub const ACCENT_2: Color32 = Color32::from_rgb(139, 92, 246);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(91, 140, 255, 40);
pub const OK: Color32 = Color32::from_rgb(52, 211, 153);
pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
pub const DANGER: Color32 = Color32::from_rgb(251, 113, 133);

/// 在创建窗口后调用一次。字体已经在 `install_cjk_font` 里装好了。
pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let mut visuals = egui::Visuals::dark();

    visuals.dark_mode = true;
    visuals.panel_fill = BG_TOP;
    visuals.window_fill = CARD;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(18);
    visuals.menu_corner_radius = CornerRadius::same(12);
    visuals.window_shadow = shadow();
    visuals.popup_shadow = shadow();
    visuals.extreme_bg_color = INPUT_BG;
    visuals.text_edit_bg_color = Some(INPUT_BG);
    visuals.faint_bg_color = Color32::from_rgba_unmultiplied_const(255, 255, 255, 8);
    visuals.code_bg_color = Color32::from_rgba_unmultiplied_const(255, 255, 255, 12);
    visuals.selection = egui::style::Selection {
        bg_fill: Color32::from_rgba_unmultiplied_const(91, 140, 255, 88),
        stroke: Stroke::new(1.0, TEXT),
    };
    visuals.hyperlink_color = ACCENT;
    visuals.warn_fg_color = WARN;
    visuals.error_fg_color = DANGER;
    visuals.weak_text_color = Some(TEXT_DIM);
    visuals.collapsing_header_frame = false;
    visuals.striped = false;
    visuals.widgets = widgets();

    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(9.0, 9.0);
    style.spacing.button_padding = Vec2::new(13.0, 7.0);
    style.spacing.interact_size = Vec2::new(38.0, 30.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.indent = 16.0;
    style.spacing.combo_width = 110.0;
    style.spacing.slider_width = 140.0;
    style.spacing.icon_width = 16.0;
    style.spacing.icon_width_inner = 9.0;
    style.spacing.icon_spacing = 7.0;
    style.animation_time = 0.16;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(23.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into_iter()
    .collect();

    ctx.set_style_of(egui::Theme::Dark, style);
}

fn shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: Color32::from_black_alpha(110),
    }
}

fn widgets() -> egui::style::Widgets {
    use egui::style::WidgetVisuals;

    let noninteractive = WidgetVisuals {
        bg_fill: BG_TOP,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::new(1.0, BORDER),
        fg_stroke: Stroke::new(1.0, TEXT),
        corner_radius: CornerRadius::same(10),
        expansion: 0.0,
    };
    let inactive = WidgetVisuals {
        bg_fill: Color32::from_rgba_unmultiplied_const(255, 255, 255, 14),
        weak_bg_fill: Color32::from_rgba_unmultiplied_const(255, 255, 255, 14),
        bg_stroke: Stroke::new(1.0, BORDER),
        fg_stroke: Stroke::new(1.0, TEXT),
        corner_radius: CornerRadius::same(10),
        expansion: 0.0,
    };
    let hovered = WidgetVisuals {
        bg_fill: Color32::from_rgba_unmultiplied_const(255, 255, 255, 24),
        weak_bg_fill: Color32::from_rgba_unmultiplied_const(255, 255, 255, 24),
        bg_stroke: Stroke::new(1.0, BORDER_STRONG),
        fg_stroke: Stroke::new(1.0, TEXT),
        corner_radius: CornerRadius::same(10),
        expansion: 0.0,
    };
    let active = WidgetVisuals {
        bg_fill: ACCENT_SOFT,
        weak_bg_fill: Color32::from_rgba_unmultiplied_const(91, 140, 255, 54),
        bg_stroke: Stroke::new(1.0, ACCENT),
        fg_stroke: Stroke::new(1.0, TEXT),
        corner_radius: CornerRadius::same(10),
        expansion: 0.0,
    };

    egui::style::Widgets {
        noninteractive,
        inactive,
        hovered,
        active,
        open: active,
    }
}

/// 铺满整个面板的深色渐变加两团柔光。必须在所有内容之前画。
pub fn paint_background(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();

    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), BG_TOP);
    mesh.colored_vertex(rect.right_top(), Color32::from_rgb(16, 21, 43));
    mesh.colored_vertex(rect.left_bottom(), BG_BOTTOM);
    mesh.colored_vertex(rect.right_bottom(), Color32::from_rgb(8, 12, 24));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));

    glow(
        painter,
        rect.right_top() + Vec2::new(-60.0, 60.0),
        300.0,
        Color32::from_rgba_unmultiplied_const(91, 140, 255, 26),
    );
    glow(
        painter,
        rect.right_bottom() + Vec2::new(-130.0, -70.0),
        250.0,
        Color32::from_rgba_unmultiplied_const(ACCENT_2.r(), ACCENT_2.g(), ACCENT_2.b(), 22),
    );
}

fn glow(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    let steps = 40;
    for index in 0..=steps {
        let angle = std::f32::consts::TAU * index as f32 / steps as f32;
        mesh.colored_vertex(
            center + Vec2::new(angle.cos(), angle.sin()) * radius,
            Color32::TRANSPARENT,
        );
    }
    for index in 0..steps {
        mesh.add_triangle(0, index + 1, index + 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 品牌标记：蓝色圆角方块 + 两个反向箭头，和 .app 图标同源。
pub fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let painter = ui.painter();
    let radius = CornerRadius::same((size * 0.28).round() as u8);
    painter.rect_filled(rect, radius, ACCENT);

    let scale = size / 44.0;
    let point = |x: f32, y: f32| egui::pos2(rect.min.x + x * scale, rect.min.y + y * scale);
    let white = Color32::WHITE;
    let dim = Color32::from_rgba_unmultiplied_const(255, 255, 255, 190);
    let stroke = Stroke::new(3.0 * scale, white);
    let dim_stroke = Stroke::new(3.0 * scale, dim);

    painter.line_segment([point(8.0, 14.0), point(27.0, 14.0)], stroke);
    painter.add(egui::Shape::convex_polygon(
        vec![point(26.0, 8.0), point(37.0, 14.0), point(26.0, 20.0)],
        white,
        Stroke::NONE,
    ));

    painter.line_segment([point(36.0, 30.0), point(17.0, 30.0)], dim_stroke);
    painter.add(egui::Shape::convex_polygon(
        vec![point(18.0, 24.0), point(7.0, 30.0), point(18.0, 36.0)],
        dim,
        Stroke::NONE,
    ));
}

/// 一张卡片。和浏览器的 .glass 对应：半透明深色底、细描边、圆角。
pub fn card<R>(
    ui: &mut egui::Ui,
    title: &str,
    hint: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        })
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            card_header(ui, title, hint);
            add(ui)
        })
        .inner
}

fn card_header(ui: &mut egui::Ui, title: &str, hint: Option<&str>) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 18.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(2), ACCENT);
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(15.5).strong().color(TEXT));
    });
    if let Some(hint) = hint {
        ui.add_space(2.0);
        ui.label(RichText::new(hint).size(11.5).color(TEXT_DIM));
    }
    ui.add_space(12.0);
}

pub fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(TEXT_DIM));
}

pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(TEXT_FAINT));
}

pub fn divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(8.0);
}

pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    let fill = Color32::from_rgba_unmultiplied_const(color.r(), color.g(), color.b(), 26);
    let stroke = Stroke::new(
        1.0,
        Color32::from_rgba_unmultiplied_const(color.r(), color.g(), color.b(), 82),
    );
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).strong().color(color));
        });
}

/// 服务端 / 客户端切换。返回用户点中的新下标。
pub fn segmented(ui: &mut egui::Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let mut chosen = None;
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied_const(7, 10, 21, 170))
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(11))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (index, label) in labels.iter().enumerate() {
                    let active = index == selected;
                    let mut text = RichText::new(*label).size(12.5).color(if active {
                        TEXT
                    } else {
                        TEXT_DIM
                    });
                    if active {
                        text = text.strong();
                    }
                    if ui.selectable_label(active, text).clicked() {
                        chosen = Some(index);
                    }
                }
            });
        });
    chosen
}

pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(13.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(ACCENT)
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied_const(255, 255, 255, 36),
        ))
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(0.0, 34.0)),
    )
}

pub fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(TEXT))
            .corner_radius(CornerRadius::same(10))
            .min_size(Vec2::new(0.0, 34.0)),
    )
}

pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .size(13.0)
                .color(Color32::from_rgb(254, 205, 211)),
        )
        .fill(Color32::from_rgba_unmultiplied_const(251, 113, 133, 30))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_unmultiplied_const(251, 113, 133, 92),
        ))
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(0.0, 34.0)),
    )
}

pub fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(TEXT_DIM))
            .frame(false)
            .min_size(Vec2::new(0.0, 28.0)),
    )
}
