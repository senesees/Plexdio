//! Shared building blocks for library views: routes, actions, the virtualised track table and
//! the art card grids.

use crate::library_model::{Id, LibraryModel};
use crate::ui::art::{ArtCache, Shape};
use crate::ui::icons::{self, Icon};
use crate::ui::theme;
use egui::{Align2, Color32, FontId, Rect, RichText, Sense};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibTab {
    Artists,
    Albums,
    Tracks,
    Genres,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Home,
    Search,
    Library(LibTab),
    Playlists,
    Artist(Id),
    Album(Id),
    Playlist(Id),
    Genre(Arc<str>),
    /// The station started last (kept in [`RadioStation`] on the app).
    Radio,
    Settings,
}

/// A started radio station, shown as its own page (like Spotify's radio playlists).
#[derive(Debug, Clone, Default)]
pub struct RadioStation {
    /// Name of the seed (track, album or artist title).
    pub name: String,
    pub thumb: Option<String>,
    /// Round art for artist seeds.
    pub round: bool,
    /// Station tracks in play order; empty while `loading`.
    pub ids: Vec<Id>,
    /// The similar-tracks lookup is still running.
    pub loading: bool,
}

/// Where a radio station starts from.
#[derive(Debug, Clone, PartialEq)]
pub enum RadioSeed {
    Track(Id),
    Album(Id),
    Artist(Id),
}

/// Things views ask the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    Navigate(Route),
    /// Replace the queue with `ids` and start at `start`.
    Play(Vec<Id>, usize),
    PlayShuffled(Vec<Id>),
    PlayNext(Vec<Id>),
    AddToQueue(Vec<Id>),
    Radio(RadioSeed),
    /// Rating 0..=10 (0 clears).
    Rate(Id, u8),
}

/// Everything a view needs for one frame.
pub struct ViewCtx<'a> {
    pub lib: &'a LibraryModel,
    pub art: &'a mut ArtCache,
    pub actions: &'a mut Vec<UiAction>,
    pub now_playing: Option<Id>,
    /// Home mixes, refreshed after syncs and plays.
    pub mixes: &'a crate::player::mixes::Mixes,
}

pub fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub fn fmt_long_duration(ms: u64) -> String {
    let m = ms / 60_000;
    if ms < 60_000 {
        format!("{} sec", ms / 1000)
    } else if m >= 60 {
        format!("{} h {} min", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// Sub-heading used inside scrollable pages.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(title).size(20.0).strong());
    ui.add_space(6.0);
}

/// "1 track" / "24 tracks".
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Text that is clipped with an ellipsis to `width`.
pub fn clipped(ui: &egui::Ui, pos: egui::Pos2, text: &str, size: f32, color: Color32, width: f32, align: Align2) {
    let galley = ui.painter().layout(
        text.to_string(),
        FontId::proportional(size),
        color,
        f32::INFINITY,
    );
    if galley.size().x <= width {
        ui.painter().galley(align.anchor_size(pos, galley.size()).min, galley, color);
        return;
    }
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(FontId::proportional(size), color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(align.anchor_size(pos, galley.size()).min, galley, color);
}

/// Page title row with an optional back button. Returns true if back was clicked.
pub fn page_header(ui: &mut egui::Ui, title: &str, can_back: bool) -> bool {
    let mut back = false;
    ui.horizontal(|ui| {
        if can_back && icons::button(ui, Icon::Back, 26.0, false).on_hover_text("Back").clicked() {
            back = true;
        }
        ui.heading(title);
    });
    back
}

/// Play / shuffle / radio buttons for a collection.
pub fn play_buttons(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], radio: Option<RadioSeed>) {
    ui.horizontal(|ui| {
        if icons::round_button(ui, Icon::Play, 46.0).on_hover_text("Play").clicked() && !ids.is_empty() {
            ctx.actions.push(UiAction::Play(ids.to_vec(), 0));
        }
        ui.add_space(6.0);
        if icons::button(ui, Icon::Shuffle, 30.0, false).on_hover_text("Shuffle play").clicked() && !ids.is_empty() {
            ctx.actions.push(UiAction::PlayShuffled(ids.to_vec()));
        }
        if let Some(seed) = radio {
            if icons::button(ui, Icon::Radio, 30.0, false).on_hover_text("Start radio").clicked() {
                ctx.actions.push(UiAction::Radio(seed));
            }
        }
        if icons::button(ui, Icon::Queue, 30.0, false).on_hover_text("Add to queue").clicked() && !ids.is_empty() {
            ctx.actions.push(UiAction::AddToQueue(ids.to_vec()));
        }
    });
}

#[derive(Debug, Clone, Copy)]
pub struct TableOpts {
    /// Show the album column.
    pub album: bool,
    /// Show artist under the title.
    pub artist: bool,
    /// Show the track number (album view) instead of the row number.
    pub track_numbers: bool,
    /// Show small cover art per row.
    pub art: bool,
}

pub const ROW_H: f32 = 48.0;

/// Virtualised track list with its own scroll area. `ids` are track ids in display order.
pub fn track_table(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, salt: &str) {
    if ids.is_empty() {
        ui.label(RichText::new("No tracks").color(theme::TEXT_DIM));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, ids.len(), |ui, range| {
            for row in range {
                track_row(ui, ctx, ids, opts, row);
            }
        });
}

/// One track row, drawn at the cursor. Used by both the virtualised table and inline lists.
pub fn track_row(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, row: usize) {
    let id = ids[row];
    let Some(t) = ctx.lib.track(id) else { return };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::click());
    let playing = ctx.now_playing == Some(id);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme::CARD_HOVER);
    }
    let w = rect.width();
    let mut x = rect.left() + 10.0;
    let cy = rect.center().y;
    // Number / playing marker.
    let num = if opts.track_numbers { t.index as usize } else { row + 1 };
    if playing {
        icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 12.0, cy), egui::vec2(16.0, 16.0)), Icon::Volume, theme::ACCENT);
    } else if resp.hovered() {
        icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(x + 12.0, cy), egui::vec2(16.0, 16.0)), Icon::Play, theme::TEXT);
    } else {
        ui.painter().text(egui::pos2(x + 24.0, cy), Align2::RIGHT_CENTER, num.to_string(), FontId::proportional(13.0), theme::TEXT_DIM);
    }
    x += 40.0;
    if opts.art {
        let r = Rect::from_min_size(egui::pos2(x, cy - 18.0), egui::vec2(36.0, 36.0));
        ctx.art.paint(ui, r, t.thumb.as_deref(), &t.album, Shape::Rounded(4));
        x += 46.0;
    }
    let dur_w = 60.0;
    let album_w = if opts.album { (w * 0.28).min(320.0) } else { 0.0 };
    let title_w = (rect.right() - x - dur_w - album_w - 20.0).max(60.0);
    let title_color = if playing { theme::ACCENT } else { theme::TEXT };
    if opts.artist {
        clipped(ui, egui::pos2(x, cy - 9.0), &t.title, 14.5, title_color, title_w, Align2::LEFT_CENTER);
        clipped(ui, egui::pos2(x, cy + 10.0), t.display_artist(), 12.5, theme::TEXT_DIM, title_w, Align2::LEFT_CENTER);
    } else {
        clipped(ui, egui::pos2(x, cy), &t.title, 14.5, title_color, title_w, Align2::LEFT_CENTER);
    }
    if opts.album {
        let ax = rect.right() - dur_w - album_w - 10.0;
        clipped(ui, egui::pos2(ax, cy), &t.album, 13.0, theme::TEXT_DIM, album_w - 10.0, Align2::LEFT_CENTER);
    }
    if t.user_rating > 0.0 {
        let sr = Rect::from_center_size(egui::pos2(rect.right() - dur_w - 14.0, cy), egui::vec2(14.0, 14.0));
        icons::paint(ui.painter(), sr, Icon::StarFilled, theme::ACCENT.linear_multiply(0.8));
    }
    ui.painter().text(
        egui::pos2(rect.right() - 12.0, cy),
        Align2::RIGHT_CENTER,
        fmt_duration(t.duration_ms as u64),
        FontId::proportional(13.0),
        theme::TEXT_DIM,
    );
    if resp.double_clicked() {
        ctx.actions.push(UiAction::Play(ids.to_vec(), row));
    }
    resp.context_menu(|ui| track_menu(ui, ctx, id, ids, row));
}

/// Non-scrolling track list for pages that already scroll. Draws at most `max_rows` rows.
pub fn track_list(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, max_rows: usize) {
    if ids.is_empty() {
        ui.label(RichText::new("No tracks").color(theme::TEXT_DIM));
        return;
    }
    for row in 0..ids.len().min(max_rows) {
        track_row(ui, ctx, ids, opts, row);
    }
}

pub fn track_menu(ui: &mut egui::Ui, ctx: &mut ViewCtx, id: Id, ids: &[Id], row: usize) {
    let Some(t) = ctx.lib.track(id) else { return };
    let (album_id, artist_id, rating) = (t.album_id, t.artist_id, t.user_rating);
    ui.set_min_width(180.0);
    if ui.button("Play").clicked() {
        ctx.actions.push(UiAction::Play(ids.to_vec(), row));
        ui.close();
    }
    if ui.button("Play next").clicked() {
        ctx.actions.push(UiAction::PlayNext(vec![id]));
        ui.close();
    }
    if ui.button("Add to queue").clicked() {
        ctx.actions.push(UiAction::AddToQueue(vec![id]));
        ui.close();
    }
    if ui.button("Start track radio").clicked() {
        ctx.actions.push(UiAction::Radio(RadioSeed::Track(id)));
        ui.close();
    }
    ui.separator();
    if ui.button("Go to album").clicked() {
        ctx.actions.push(UiAction::Navigate(Route::Album(album_id)));
        ui.close();
    }
    if ctx.lib.artist(artist_id).is_some() && ui.button("Go to artist").clicked() {
        ctx.actions.push(UiAction::Navigate(Route::Artist(artist_id)));
        ui.close();
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Rate");
        for star in 1..=5u8 {
            let filled = rating >= (star * 2) as f32;
            if icons::button(ui, if filled { Icon::StarFilled } else { Icon::Star }, 20.0, filled).clicked() {
                let new = if filled && rating as u8 == star * 2 { 0 } else { star * 2 };
                ctx.actions.push(UiAction::Rate(id, new));
                ui.close();
            }
        }
    });
}

#[derive(Clone, Copy)]
pub struct CardItem<'a> {
    pub thumb: Option<&'a str>,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub seed: &'a str,
}

pub const CARD_W: f32 = 172.0;
pub const CARD_H: f32 = 226.0;

/// Draw one card at the cursor. Returns its response.
pub fn card(ui: &mut egui::Ui, art: &mut ArtCache, item: CardItem, round: bool, width: f32) -> egui::Response {
    let h = width + 54.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, theme::CARD_HOVER);
    } else {
        ui.painter().rect_filled(rect, 8.0, theme::CARD);
    }
    let pad = 12.0;
    let art_rect = Rect::from_min_size(rect.min + egui::vec2(pad, pad), egui::vec2(width - 2.0 * pad, width - 2.0 * pad));
    art.paint(ui, art_rect, item.thumb, item.seed, if round { Shape::Circle } else { Shape::Rounded(6) });
    let tx = rect.left() + pad;
    let ty = art_rect.bottom() + 16.0;
    let align = if round { Align2::CENTER_CENTER } else { Align2::LEFT_CENTER };
    let x = if round { rect.center().x } else { tx };
    clipped(ui, egui::pos2(x, ty), item.title, 14.5, theme::TEXT, width - 2.0 * pad, align);
    clipped(ui, egui::pos2(x, ty + 19.0), item.subtitle, 12.5, theme::TEXT_DIM, width - 2.0 * pad, align);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Virtualised grid of cards. `on_click(i)` is called with the clicked index.
pub fn card_grid<'a>(
    ui: &mut egui::Ui,
    art: &mut ArtCache,
    n: usize,
    round: bool,
    salt: &str,
    item: impl Fn(usize) -> CardItem<'a>,
    mut on_click: impl FnMut(usize, &egui::Response),
) {
    if n == 0 {
        ui.label(RichText::new("Nothing here").color(theme::TEXT_DIM));
        return;
    }
    let (cols, w, row_h) = grid_metrics(ui.available_width());
    let rows = n.div_ceil(cols);
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show_rows(ui, row_h, rows, |ui, range| {
        for r in range {
            grid_row(ui, art, n, round, cols, w, r, &item, &mut on_click);
        }
    });
}

const GRID_GAP: f32 = 14.0;

/// Columns, card width and row height for a card grid `avail` points wide.
fn grid_metrics(avail: f32) -> (usize, f32, f32) {
    let cols = ((avail + GRID_GAP) / (CARD_W + GRID_GAP)).floor().max(1.0) as usize;
    let w = ((avail + GRID_GAP) / cols as f32 - GRID_GAP).max(120.0);
    (cols, w, w + 54.0 + GRID_GAP)
}

#[allow(clippy::too_many_arguments)]
fn grid_row<'a>(
    ui: &mut egui::Ui,
    art: &mut ArtCache,
    n: usize,
    round: bool,
    cols: usize,
    w: f32,
    r: usize,
    item: &impl Fn(usize) -> CardItem<'a>,
    on_click: &mut impl FnMut(usize, &egui::Response),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GRID_GAP;
        for c in 0..cols {
            let i = r * cols + c;
            if i >= n {
                break;
            }
            let resp = card(ui, art, item(i), round, w);
            on_click(i, &resp);
        }
    });
    ui.add_space(GRID_GAP - ui.spacing().item_spacing.y);
}

/// Card grid without its own scroll area, for pages that already scroll (all rows are drawn).
pub fn card_grid_inline<'a>(
    ui: &mut egui::Ui,
    art: &mut ArtCache,
    n: usize,
    round: bool,
    item: impl Fn(usize) -> CardItem<'a>,
    mut on_click: impl FnMut(usize, &egui::Response),
) {
    if n == 0 {
        ui.label(RichText::new("Nothing here").color(theme::TEXT_DIM));
        return;
    }
    let (cols, w, _) = grid_metrics(ui.available_width());
    for r in 0..n.div_ceil(cols) {
        grid_row(ui, art, n, round, cols, w, r, &item, &mut on_click);
    }
}

/// Horizontal strip of cards (Home rows), not virtualised (short).
pub fn card_row<'a>(
    ui: &mut egui::Ui,
    art: &mut ArtCache,
    n: usize,
    round: bool,
    salt: &str,
    item: impl Fn(usize) -> CardItem<'a>,
    mut on_click: impl FnMut(usize, &egui::Response),
) {
    egui::ScrollArea::horizontal().id_salt(salt).auto_shrink([false, true]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for i in 0..n {
                let resp = card(ui, art, item(i), round, 160.0);
                on_click(i, &resp);
            }
        });
    });
}
