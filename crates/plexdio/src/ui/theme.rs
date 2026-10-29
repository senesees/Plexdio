//! Dark, Spotify-like look.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(18, 18, 18);
pub const SIDEBAR: Color32 = Color32::from_rgb(0, 0, 0);
pub const BAR: Color32 = Color32::from_rgb(24, 24, 24);
pub const CARD: Color32 = Color32::from_rgb(28, 28, 28);
pub const CARD_HOVER: Color32 = Color32::from_rgb(40, 40, 40);
/// Selected rows in track lists.
pub const SELECTED: Color32 = Color32::from_rgb(56, 56, 56);
pub const ACCENT: Color32 = Color32::from_rgb(229, 160, 13);
pub const TEXT: Color32 = Color32::from_rgb(235, 235, 235);
pub const TEXT_DIM: Color32 = Color32::from_rgb(167, 167, 167);
pub const ERROR: Color32 = Color32::from_rgb(232, 90, 79);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(26.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.scroll.bar_width = 8.0;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BAR;
    v.extreme_bg_color = Color32::from_rgb(36, 36, 36);
    v.faint_bg_color = Color32::from_rgb(24, 24, 24);
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(40, 40, 40));
    w.inactive.bg_fill = Color32::from_rgb(40, 40, 40);
    w.inactive.weak_bg_fill = Color32::from_rgb(40, 40, 40);
    w.hovered.bg_fill = Color32::from_rgb(56, 56, 56);
    w.hovered.weak_bg_fill = Color32::from_rgb(56, 56, 56);
    w.active.bg_fill = Color32::from_rgb(70, 70, 70);
    w.active.weak_bg_fill = Color32::from_rgb(70, 70, 70);
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(6);
    }
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// Accent-filled pill button.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let label = egui::RichText::new(text).size(15.0).strong().color(Color32::BLACK);
    ui.add(
        egui::Button::new(label)
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(20))
            .min_size(egui::vec2(160.0, 38.0)),
    )
}

/// Subtle outlined pill button.
pub fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let label = egui::RichText::new(text).size(14.0).color(TEXT);
    ui.add(
        egui::Button::new(label)
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, Color32::from_rgb(110, 110, 110)))
            .corner_radius(CornerRadius::same(20))
            .min_size(egui::vec2(120.0, 34.0)),
    )
}

/// On/off switch (Spotify-style settings toggle). Marks the response changed when flipped.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(38.0, 22.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let enabled = ui.is_enabled();
    let dim = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
    let track = if *on { ACCENT } else if resp.hovered() { Color32::from_rgb(96, 96, 96) } else { Color32::from_rgb(80, 80, 80) };
    ui.painter().rect_filled(rect, CornerRadius::same(11), dim(track));
    let r = rect.height() / 2.0 - 3.0;
    let x = egui::lerp((rect.left() + r + 3.0)..=(rect.right() - r - 3.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), r, dim(Color32::WHITE));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded card frame.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(24))
}

/// Small rotating arc used as a busy indicator.
pub fn spinner(ui: &mut egui::Ui, size: f32) {
    ui.add(egui::Spinner::new().size(size).color(ACCENT));
}
