//! Design system: gold-on-near-black palette (plus light), type scale,
//! spacing and the shared widgets (header tabs, tool rows, panel
//! headers, big tool buttons, pills, cards).

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, TextStyle, Vec2,
};

pub const SP_XS: f32 = 4.0;
pub const SP_SM: f32 = 8.0;
pub const SP_MD: f32 = 12.0;
pub const SP_LG: f32 = 16.0;
pub const R_SM: f32 = 4.0;
pub const R_MD: f32 = 6.0;
pub const R_LG: f32 = 12.0;
pub const ROW_H: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hi: Color32,
    pub border: Color32,
    /// Thin gold outline colour.
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub success: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub canvas: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        let border = Color32::from_rgb(38, 36, 33);
        let accent = Color32::from_rgb(222, 176, 76);
        Self {
            dark: true,
            bg: Color32::from_rgb(9, 9, 10),
            panel: Color32::from_rgb(17, 17, 19),
            surface: Color32::from_rgb(23, 23, 25),
            surface_hi: Color32::from_rgb(32, 31, 30),
            border,
            line: mix(border, accent, 0.42),
            text: Color32::from_rgb(232, 229, 222),
            muted: Color32::from_rgb(148, 142, 130),
            accent,
            accent_text: Color32::from_rgb(20, 16, 8),
            success: Color32::from_rgb(64, 200, 100),
            danger: Color32::from_rgb(240, 72, 60),
            warn: Color32::from_rgb(255, 165, 30),
            canvas: Color32::from_rgb(13, 13, 14),
        }
    }

    pub fn light() -> Self {
        let border = Color32::from_rgb(214, 208, 196);
        let accent = Color32::from_rgb(160, 116, 22);
        Self {
            dark: false,
            bg: Color32::from_rgb(234, 232, 228),
            panel: Color32::from_rgb(247, 246, 242),
            surface: Color32::from_rgb(255, 255, 253),
            surface_hi: Color32::from_rgb(236, 233, 226),
            border,
            line: mix(border, accent, 0.45),
            text: Color32::from_rgb(30, 28, 24),
            muted: Color32::from_rgb(112, 106, 96),
            accent,
            accent_text: Color32::from_rgb(255, 255, 255),
            success: Color32::from_rgb(36, 150, 70),
            danger: Color32::from_rgb(200, 40, 35),
            warn: Color32::from_rgb(190, 115, 0),
            canvas: Color32::from_rgb(220, 218, 213),
        }
    }

    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Opaque accent tint for selection fills (CPU mixed so every GPU
    /// backend shows the same colour).
    pub fn accent_soft(&self) -> Color32 {
        mix(
            self.surface,
            self.accent,
            if self.dark { 0.15 } else { 0.16 },
        )
    }
}

/// Linear mix of two opaque colours.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Apply palette, type scale and spacing to the context.
pub fn apply(ctx: &egui::Context, p: &Palette) {
    // Pin the theme: egui's default `ThemePreference::System` would swap in
    // its stock light style whenever the OS reports a light theme (Windows
    // default, and Wine), ignoring the app's own palette.
    let theme = if p.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    let mut style = (*ctx.style_of(theme)).clone();
    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.panel;
    v.window_fill = p.surface;
    v.extreme_bg_color = if p.dark { p.bg } else { p.surface };
    v.faint_bg_color = p.surface_hi;
    v.code_bg_color = p.surface_hi;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_soft();
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.window_corner_radius = CornerRadius::same(R_LG as u8);
    v.menu_corner_radius = CornerRadius::same(R_MD as u8);
    v.window_stroke = Stroke::new(1.0_f32, p.line);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.override_text_color = None;
    let w = &mut v.widgets;
    for (state, fill) in [
        (&mut w.noninteractive, p.surface),
        (&mut w.inactive, p.surface_hi),
        (&mut w.hovered, mix(p.surface_hi, p.accent, 0.12)),
        (&mut w.active, mix(p.surface_hi, p.accent, 0.22)),
        (&mut w.open, p.surface_hi),
    ] {
        state.corner_radius = CornerRadius::same(R_SM as u8);
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.fg_stroke = Stroke::new(1.0_f32, p.text);
        state.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.line);
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.open.bg_stroke = Stroke::new(1.0_f32, p.line);
    style.visuals = v;

    style.spacing.item_spacing = Vec2::new(SP_SM, 5.0);
    style.spacing.button_padding = Vec2::new(8.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    style.spacing.slider_width = 140.0;
    style.spacing.window_margin = egui::Margin::same(SP_LG as i8);
    style.spacing.menu_margin = egui::Margin::same(SP_SM as i8);
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.animation_time = 0.14;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(16.0)),
        (TextStyle::Body, FontId::proportional(12.5)),
        (TextStyle::Button, FontId::proportional(12.5)),
        (TextStyle::Small, FontId::proportional(10.5)),
        (TextStyle::Monospace, FontId::monospace(12.0)),
    ]
    .into();
    ctx.set_style_of(theme, style);
}

// ---- brand ----------------------------------------------------------------

/// Faceted gold gem logo painted into `rect`.
pub fn gem(painter: &egui::Painter, rect: Rect, p: &Palette) {
    let c = rect.center();
    let w = rect.width() * 0.5;
    let h = rect.height() * 0.5;
    let top_y = c.y - h * 0.55;
    let girdle_y = c.y - h * 0.15;
    let tip = Pos2::new(c.x, c.y + h * 0.9);
    let tl = Pos2::new(c.x - w * 0.55, top_y);
    let tr = Pos2::new(c.x + w * 0.55, top_y);
    let gl = Pos2::new(c.x - w, girdle_y);
    let gr = Pos2::new(c.x + w, girdle_y);
    let light = mix(p.accent, Color32::WHITE, 0.45);
    let dark = mix(p.accent, Color32::BLACK, 0.35);
    // crown
    painter.add(egui::Shape::convex_polygon(
        vec![tl, tr, gr, gl],
        light,
        Stroke::NONE,
    ));
    // pavilion halves
    painter.add(egui::Shape::convex_polygon(
        vec![gl, Pos2::new(c.x, girdle_y), tip],
        p.accent,
        Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![Pos2::new(c.x, girdle_y), gr, tip],
        dark,
        Stroke::NONE,
    ));
    let s = Stroke::new(1.0_f32, mix(p.accent, Color32::BLACK, 0.55));
    let m1 = Pos2::new(c.x - w * 0.3, girdle_y);
    let m2 = Pos2::new(c.x + w * 0.3, girdle_y);
    for (a, b) in [
        (tl, m1),
        (m1, Pos2::new(c.x, top_y)),
        (Pos2::new(c.x, top_y), m2),
        (m2, tr),
        (gl, gr),
        (m1, tip),
        (m2, tip),
    ] {
        painter.line_segment([a, b], s);
    }
    painter.add(egui::Shape::closed_line(
        vec![tl, tr, gr, tip, gl],
        Stroke::new(1.2_f32, mix(p.accent, Color32::WHITE, 0.2)),
    ));
}

/// Big header tab: icon + label, gold outline when active.
pub fn mode_tab(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    label: &str,
    active: bool,
    width: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let fill = if active {
        mix(p.surface, p.accent, 0.10)
    } else if hovered {
        p.surface_hi
    } else {
        p.surface
    };
    let stroke = if active {
        Stroke::new(1.5_f32, p.accent)
    } else {
        Stroke::new(1.0_f32, p.border)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(R_SM as u8),
        fill,
        stroke,
        egui::StrokeKind::Middle,
    );
    let col = if active {
        p.accent
    } else if hovered {
        p.text
    } else {
        mix(p.muted, p.text, 0.4)
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_uppercase(), FontId::proportional(12.5), col);
    let icon_w = 22.0;
    let total = icon_w + 8.0 + galley.size().x;
    let x0 = rect.center().x - total / 2.0;
    ui.painter().text(
        Pos2::new(x0 + icon_w / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(19.0),
        col,
    );
    ui.painter().galley(
        Pos2::new(x0 + icon_w + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        col,
    );
    resp
}

/// Small gold outlined badge such as "Ps".
pub fn badge(ui: &mut egui::Ui, p: &Palette, text: &str) -> egui::Response {
    egui::Frame::NONE
        .stroke(Stroke::new(1.0_f32, p.accent))
        .fill(mix(p.panel, p.accent, 0.08))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.5)
                    .strong()
                    .color(p.accent),
            )
        })
        .response
}

/// Panel title bar (LAYERS / MASKS / PROPERTIES ...). Returns true if
/// the close/collapse control was clicked.
pub fn panel_header(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    badge_text: Option<&str>,
    open: bool,
) -> bool {
    let mut toggled = false;
    egui::Frame::NONE
        .fill(p.surface)
        .inner_margin(egui::Margin::symmetric(SP_SM as i8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title.to_uppercase())
                        .size(12.0)
                        .strong()
                        .color(p.text),
                );
                if let Some(b) = badge_text {
                    badge(ui, p, b);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let icon = if open {
                        egui_phosphor::regular::CARET_UP
                    } else {
                        egui_phosphor::regular::CARET_DOWN
                    };
                    if icon_button(ui, p, icon, if open { "Collapse" } else { "Expand" }, false)
                        .clicked()
                    {
                        toggled = true;
                    }
                });
            });
        });
    let r = ui.min_rect();
    ui.painter().hline(
        r.x_range(),
        ui.cursor().top(),
        Stroke::new(1.0_f32, p.border),
    );
    toggled
}

/// Collapsible gold section header for the tool panel.
pub fn section_header(ui: &mut egui::Ui, p: &Palette, title: &str, open: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let fill = if resp.hovered() {
        mix(p.surface, p.accent, 0.06)
    } else {
        p.surface
    };
    ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0_f32, p.border),
    );
    let caret = if open {
        egui_phosphor::regular::CARET_DOWN
    } else {
        egui_phosphor::regular::CARET_RIGHT
    };
    ui.painter().text(
        Pos2::new(rect.left() + 10.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        caret,
        FontId::proportional(11.0),
        p.accent,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 20.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        FontId::proportional(11.5),
        p.accent,
    );
    let chev = if open {
        egui_phosphor::regular::CARET_UP
    } else {
        egui_phosphor::regular::CARET_DOWN
    };
    ui.painter().text(
        Pos2::new(rect.right() - 12.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        chev,
        FontId::proportional(10.0),
        p.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Tool row: icon, label and right-aligned shortcut.
pub fn tool_row(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    label: &str,
    key: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), sense);
    let hovered = resp.hovered() && enabled;
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, p.accent_soft());
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())),
            CornerRadius::ZERO,
            p.accent,
        );
    } else if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, p.surface_hi);
    }
    let col = if !enabled {
        p.muted.gamma_multiply(0.6)
    } else if selected {
        p.accent
    } else {
        p.text
    };
    ui.painter().text(
        Pos2::new(rect.left() + 22.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(14.0),
        if selected { p.accent } else { col },
    );
    ui.painter().text(
        Pos2::new(rect.left() + 36.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        col,
    );
    ui.painter().text(
        Pos2::new(rect.right() - 12.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        key,
        FontId::proportional(11.5),
        if selected { p.accent } else { p.muted },
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Frameless icon button with hover background.
pub fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    tip: &str,
    on: bool,
) -> egui::Response {
    icon_button_sized(ui, p, icon, tip, on, 22.0, 14.0)
}

pub fn icon_button_sized(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    tip: &str,
    on: bool,
    size: f32,
    font: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if on {
        ui.painter().rect(
            rect,
            CornerRadius::same(R_SM as u8),
            p.accent_soft(),
            Stroke::new(1.0_f32, p.line),
            egui::StrokeKind::Middle,
        );
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_SM as u8), p.surface_hi);
    }
    let col = if on {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        mix(p.muted, p.text, 0.45)
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(font),
        col,
    );
    resp
}

/// Large icon-over-label button (Maya-style shelf).
pub fn big_tool(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: &str,
    label: &str,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let w = 62.0_f32.max(
        ui.painter()
            .layout_no_wrap(label.to_string(), FontId::proportional(11.0), p.text)
            .size()
            .x
            + 10.0,
    );
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(w, 54.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = resp.hovered() && enabled;
    if active {
        ui.painter().rect(
            rect,
            CornerRadius::same(R_SM as u8),
            p.accent_soft(),
            Stroke::new(1.0_f32, p.accent),
            egui::StrokeKind::Middle,
        );
    } else if hovered {
        ui.painter().rect(
            rect,
            CornerRadius::same(R_SM as u8),
            p.surface_hi,
            Stroke::new(1.0_f32, p.line),
            egui::StrokeKind::Middle,
        );
    }
    let icol = if !enabled {
        p.muted.gamma_multiply(0.55)
    } else {
        p.accent
    };
    let tcol = if !enabled {
        p.muted.gamma_multiply(0.6)
    } else if active {
        p.accent
    } else {
        p.text
    };
    ui.painter().text(
        Pos2::new(rect.center().x, rect.top() + 19.0),
        egui::Align2::CENTER_CENTER,
        icon,
        FontId::proportional(24.0),
        icol,
    );
    ui.painter().text(
        Pos2::new(rect.center().x, rect.bottom() - 10.0),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        tcol,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Vertical divider for toolbars.
pub fn vdivider(ui: &mut egui::Ui, p: &Palette, h: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, h), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0_f32, p.border));
}

/// Rounded card with an optional gold small-caps title.
pub fn card<R>(
    ui: &mut egui::Ui,
    p: &Palette,
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .fill(p.surface)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(R_MD as u8))
        // egui >= 0.31 counts the 1 px stroke in the frame size; take it out of
        // the margin so the card keeps its 0.29 dimensions.
        .inner_margin(egui::Margin::same((SP_SM + 1.0) as i8))
        .outer_margin(egui::Margin {
            left: 0,
            right: 0,
            top: 0,
            bottom: SP_SM as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(section(p, title));
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

/// Small-caps gold section label.
pub fn section(p: &Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(10.5)
        .color(p.accent)
        .strong()
}

pub fn muted(p: &Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text).color(p.muted).size(11.5)
}

/// Filled gold button.
pub fn primary_button(p: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.to_string())
            .color(p.accent_text)
            .strong(),
    )
    .fill(p.accent)
    .stroke(Stroke::NONE)
    .corner_radius(CornerRadius::same(R_SM as u8))
}

/// Outlined red button for Stop.
pub fn stop_button(p: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.to_string())
            .color(p.danger)
            .strong(),
    )
    .fill(p.surface_hi)
    .stroke(Stroke::new(1.0_f32, p.danger))
    .corner_radius(CornerRadius::same(R_SM as u8))
}

/// Quiet button that reads as selected when `on`.
pub fn toggle_button(p: &Palette, text: &str, on: bool) -> egui::Button<'static> {
    let (fill, color) = if on {
        (p.accent_soft(), p.accent)
    } else {
        (p.surface_hi, p.text)
    };
    egui::Button::new(egui::RichText::new(text.to_string()).color(color))
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, if on { p.accent } else { p.border }))
        .corner_radius(CornerRadius::same(R_SM as u8))
}

/// Status pill: coloured dot plus text.
pub fn pill(ui: &mut egui::Ui, p: &Palette, color: Color32, text: &str) -> egui::Response {
    egui::Frame::NONE
        .fill(p.surface_hi)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(egui::Margin::symmetric(SP_SM as i8 - 1, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, color);
                ui.label(egui::RichText::new(text).size(11.0).color(p.text));
            });
        })
        .response
}

/// Segmented control with an animated thumb. Returns the new index.
pub fn segmented(
    ui: &mut egui::Ui,
    p: &Palette,
    id: &str,
    labels: &[&str],
    selected: usize,
    seg_w: f32,
) -> Option<usize> {
    let n = labels.len().max(1);
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(seg_w * n as f32 + 4.0, h), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same((R_SM + 1.0) as u8),
        p.surface_hi,
        Stroke::new(1.0_f32, p.border),
        egui::StrokeKind::Middle,
    );
    let t =
        ui.ctx()
            .animate_value_with_time(egui::Id::new(id).with("thumb"), selected as f32, 0.16);
    let thumb = Rect::from_min_size(
        rect.min + Vec2::new(2.0 + t * seg_w, 2.0),
        Vec2::new(seg_w, h - 4.0),
    );
    painter.rect(
        thumb,
        CornerRadius::same(R_SM as u8),
        p.accent_soft(),
        Stroke::new(1.0_f32, p.accent),
        egui::StrokeKind::Middle,
    );
    let mut picked = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(
            rect.min + Vec2::new(2.0 + i as f32 * seg_w, 2.0),
            Vec2::new(seg_w, h - 4.0),
        );
        let resp = ui
            .interact(r, egui::Id::new(id).with(i), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() && i != selected {
            picked = Some(i);
        }
        let color = if i == selected {
            p.accent
        } else if resp.hovered() {
            p.text
        } else {
            p.muted
        };
        painter.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            FontId::proportional(12.0),
            color,
        );
    }
    picked
}

/// Key-value row used in inspectors.
pub fn kv(ui: &mut egui::Ui, p: &Palette, key: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).color(p.muted).size(11.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value.into()).color(p.text).size(11.5));
        });
    });
}

/// Two-column property row with fixed key column (Properties panel).
pub fn prop(ui: &mut egui::Ui, p: &Palette, key: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(86.0, 16.0), Sense::hover());
        ui.painter().text(
            r.left_center(),
            egui::Align2::LEFT_CENTER,
            key,
            FontId::proportional(11.5),
            p.muted,
        );
        ui.label(egui::RichText::new(value.into()).color(p.text).size(11.5));
    });
}

/// Empty-state block centred in the available space.
pub fn empty_state(ui: &mut egui::Ui, p: &Palette, icon: &str, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.3).max(SP_LG));
        ui.label(egui::RichText::new(icon).size(34.0).color(p.muted));
        ui.add_space(SP_SM);
        ui.label(egui::RichText::new(title).size(15.0).color(p.text).strong());
        ui.add_space(SP_XS);
        ui.label(egui::RichText::new(body).size(12.0).color(p.muted));
    });
}
