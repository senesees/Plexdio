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
    /// A "Made for you" mix, by [`crate::player::mixes::Mix::key`].
    Mix(String),
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
    /// Ask for a name, then create a playlist holding these tracks (may be empty).
    NewPlaylist(Vec<Id>),
    /// Append tracks to a playlist (asks first when some are already in it).
    AddToPlaylist(Id, Vec<Id>),
    /// Remove entries from a playlist, by playlist page table row.
    RemoveFromPlaylist(Id, Vec<usize>),
    /// Move a playlist entry from one table row to another.
    MovePlaylistItem(Id, usize, usize),
    /// Ask for a new name.
    RenamePlaylist(Id),
    /// Ask for confirmation, then delete.
    DeletePlaylist(Id),
    /// Show the Track info dialog.
    TrackInfo(Id),
}

/// Selected rows of one track list (click, Ctrl+click, Shift+click, Ctrl+A).
#[derive(Debug, Default, Clone)]
pub struct Selection {
    /// Salt of the list the rows belong to.
    pub list: String,
    /// Selected rows, ascending.
    pub rows: std::collections::BTreeSet<usize>,
    /// Track id of each selected row, in row order.
    pub ids: Vec<Id>,
    /// Row Shift+click extends from.
    anchor: Option<usize>,
    /// The list is this playlist (selection can be removed from it).
    pub playlist: Option<Id>,
}

impl Selection {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn contains(&self, list: &str, row: usize) -> bool {
        self.list == list && self.rows.contains(&row)
    }

    /// More than one row selected: menus and the selection bar act on all of them.
    pub fn is_multi(&self) -> bool {
        self.rows.len() > 1
    }

    /// Apply a click on `row` of `list` (`toggle` = Ctrl, `range` = Shift).
    pub fn click(&mut self, list: &str, row: usize, ids: &[Id], toggle: bool, range: bool, playlist: Option<Id>) {
        if self.list != list {
            self.clear();
            self.list = list.to_string();
        }
        self.playlist = playlist;
        match (range, self.anchor) {
            (true, Some(a)) => {
                self.rows = (a.min(row)..=a.max(row)).collect();
            }
            _ if toggle => {
                if !self.rows.remove(&row) {
                    self.rows.insert(row);
                }
                self.anchor = Some(row);
            }
            _ => {
                self.rows = [row].into();
                self.anchor = Some(row);
            }
        }
        self.sync_ids(ids);
    }

    pub fn select_all(&mut self, list: &str, ids: &[Id], playlist: Option<Id>) {
        self.list = list.to_string();
        self.playlist = playlist;
        self.rows = (0..ids.len()).collect();
        self.anchor = Some(0);
        self.sync_ids(ids);
    }

    fn sync_ids(&mut self, ids: &[Id]) {
        self.ids = self.rows.iter().filter_map(|&r| ids.get(r).copied()).collect();
    }

    /// Forget the selection when `list` no longer shows the selected tracks at those rows
    /// (re-sorted, filtered or edited).
    fn validate(&mut self, list: &str, ids: &[Id]) {
        if self.list != list || self.rows.is_empty() {
            return;
        }
        let ok = self.rows.len() == self.ids.len() && self.rows.iter().zip(&self.ids).all(|(&r, &id)| ids.get(r) == Some(&id));
        if !ok {
            self.clear();
        }
    }

    /// Ctrl+A selects the whole list, Esc clears (unless a text field has the keyboard).
    fn keys(&mut self, ui: &egui::Ui, list: &str, ids: &[Id], playlist: Option<Id>) {
        if ui.ctx().egui_wants_keyboard_input() || !(self.list.is_empty() || self.list == list) {
            return;
        }
        let (all, esc) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::A), i.key_pressed(egui::Key::Escape)));
        if all {
            self.select_all(list, ids, playlist);
        } else if esc {
            self.clear();
        }
    }
}

/// Everything a view needs for one frame.
pub struct ViewCtx<'a> {
    pub lib: &'a LibraryModel,
    pub art: &'a mut ArtCache,
    pub actions: &'a mut Vec<UiAction>,
    pub now_playing: Option<Id>,
    /// Home mixes, refreshed after syncs and plays.
    pub mixes: &'a crate::player::mixes::Mixes,
    /// Similar artists looked up so far ("Fans also like"), by artist id.
    pub similar: &'a std::collections::HashMap<Id, Vec<Id>>,
    /// Selected rows of the current page's track list.
    pub selection: &'a mut Selection,
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
        let add = icons::button(ui, Icon::Plus, 30.0, false).on_hover_text("Add to playlist");
        if !ids.is_empty() {
            egui::Popup::menu(&add).show(|ui| add_to_playlist_menu(ui, ctx, ids, None));
        }
    });
}

/// "New playlist…" and every regular playlist (except `skip`, the one being shown); picking one
/// adds `ids` to it.
pub fn add_to_playlist_menu(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], skip: Option<Id>) {
    ui.set_min_width(200.0);
    if ui.button("New playlist…").clicked() {
        ctx.actions.push(UiAction::NewPlaylist(ids.to_vec()));
        ui.close();
    }
    let lists: Vec<&crate::library_model::Playlist> = ctx.lib.playlists.iter().filter(|p| !p.smart && Some(p.id) != skip).collect();
    if lists.is_empty() {
        return;
    }
    ui.separator();
    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for p in lists {
            if ui.button(&*p.title).clicked() {
                ctx.actions.push(UiAction::AddToPlaylist(p.id, ids.to_vec()));
                ui.close();
            }
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
    /// The list is this (regular) playlist: rows can be dragged to reorder and removed.
    pub playlist: Option<Id>,
}

pub const ROW_H: f32 = 48.0;

/// Virtualised track list with its own scroll area. `ids` are track ids in display order.
pub fn track_table(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, salt: &str) {
    track_table_at(ui, ctx, ids, opts, salt, None);
}

/// [`track_table`] that first scrolls so row `scroll_to` is at the top (A–Z jump bar).
pub fn track_table_at(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, salt: &str, scroll_to: Option<usize>) {
    if ids.is_empty() {
        ui.label(RichText::new("No tracks").color(theme::TEXT_DIM));
        return;
    }
    ctx.selection.validate(salt, ids);
    ctx.selection.keys(ui, salt, ids, opts.playlist);
    let mut area = egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]);
    if let Some(row) = scroll_to {
        area = area.vertical_scroll_offset(row as f32 * (ROW_H + ui.spacing().item_spacing.y));
    }
    area.show_rows(ui, ROW_H, ids.len(), |ui, range| {
            for row in range {
                track_row(ui, ctx, ids, opts, row, salt);
            }
        });
}

/// One track row, drawn at the cursor. Used by both the virtualised table and inline lists.
pub fn track_row(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, row: usize, list: &str) {
    let id = ids[row];
    let Some(t) = ctx.lib.track(id) else { return };
    let sense = if opts.playlist.is_some() { Sense::click_and_drag() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), sense);
    if let Some(pl) = opts.playlist {
        playlist_drag(ui, ctx, pl, row, rect, &resp);
    }
    let playing = ctx.now_playing == Some(id);
    let w = rect.width();
    let mut x = rect.left() + 10.0;
    let cy = rect.center().y;
    // The hover play icon is a single-click button (Spotify style); double-clicking the row
    // still plays too. It sits on top of the row, so hovering it keeps the row highlighted.
    let icon_rect = Rect::from_center_size(egui::pos2(x + 12.0, cy), egui::vec2(16.0, 16.0));
    let play = (!playing).then(|| ui.interact(icon_rect.expand(8.0), resp.id.with("play"), Sense::click()));
    let play_hovered = play.as_ref().is_some_and(|p| p.hovered());
    let hovered = resp.hovered() || play_hovered;
    // Click selects (Ctrl toggles, Shift extends); right-click on an unselected row selects it.
    if resp.clicked() || (resp.secondary_clicked() && !ctx.selection.contains(list, row)) {
        let m = ui.input(|i| i.modifiers);
        let primary = resp.clicked();
        ctx.selection.click(list, row, ids, primary && m.command, primary && m.shift, opts.playlist);
    }
    let selected = ctx.selection.contains(list, row);
    if selected {
        ui.painter().rect_filled(rect, 6.0, theme::SELECTED);
    } else if hovered {
        ui.painter().rect_filled(rect, 6.0, theme::CARD_HOVER);
    }
    // Number / playing marker.
    let num = if opts.track_numbers { t.index as usize } else { row + 1 };
    if playing {
        icons::paint(ui.painter(), icon_rect, Icon::Volume, theme::ACCENT);
    } else if hovered {
        icons::paint(ui.painter(), icon_rect, Icon::Play, if play_hovered { theme::ACCENT } else { theme::TEXT });
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
    let play_clicked = play.is_some_and(|p| p.on_hover_text("Play").on_hover_cursor(egui::CursorIcon::PointingHand).clicked());
    if play_clicked || resp.double_clicked() {
        ctx.actions.push(UiAction::Play(ids.to_vec(), row));
    }
    resp.context_menu(|ui| {
        if ctx.selection.is_multi() && ctx.selection.contains(list, row) {
            selection_menu(ui, ctx);
        } else {
            track_menu(ui, ctx, id, ids, row, opts.playlist);
        }
    });
}

/// Id of the playlist row being dragged (egui temp memory; cleared by [`end_playlist_drag`]).
fn drag_id(playlist: Id) -> egui::Id {
    egui::Id::unique(("playlist-drag", playlist))
}

/// Drag to reorder on playlist pages: remembers the dragged row, marks where it would land and
/// asks the app to move it when the button is released over another row.
fn playlist_drag(ui: &mut egui::Ui, ctx: &mut ViewCtx, playlist: Id, row: usize, rect: Rect, resp: &egui::Response) {
    let id = drag_id(playlist);
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, row));
    }
    let Some(from) = ui.data(|d| d.get_temp::<usize>(id)) else { return };
    if from == row {
        ui.painter().rect_filled(rect, 6.0, theme::CARD);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }
    let over = ui.ctx().pointer_latest_pos().is_some_and(|p| rect.contains(p));
    if !over {
        return;
    }
    // The row lands where this one is: above it when moving up, below it when moving down.
    let y = if row < from { rect.top() + 1.0 } else { rect.bottom() - 1.0 };
    ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], egui::Stroke::new(2.0, theme::ACCENT));
    if ui.input(|i| i.pointer.any_released()) {
        ctx.actions.push(UiAction::MovePlaylistItem(playlist, from, row));
        ui.data_mut(|d| d.remove::<usize>(id));
    }
}

/// Forget a finished (or cancelled) playlist row drag. Call after drawing the table.
pub fn end_playlist_drag(ui: &mut egui::Ui, playlist: Id) {
    if ui.input(|i| i.pointer.any_released() || i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.remove::<usize>(drag_id(playlist)));
    }
}

/// Non-scrolling track list for pages that already scroll. Draws at most `max_rows` rows.
pub fn track_list(ui: &mut egui::Ui, ctx: &mut ViewCtx, ids: &[Id], opts: TableOpts, max_rows: usize, salt: &str) {
    if ids.is_empty() {
        ui.label(RichText::new("No tracks").color(theme::TEXT_DIM));
        return;
    }
    let shown = &ids[..ids.len().min(max_rows)];
    ctx.selection.validate(salt, shown);
    ctx.selection.keys(ui, salt, shown, opts.playlist);
    for row in 0..shown.len() {
        track_row(ui, ctx, shown, opts, row, salt);
    }
}

/// Right-click menu for several selected tracks.
fn selection_menu(ui: &mut egui::Ui, ctx: &mut ViewCtx) {
    let ids = ctx.selection.ids.clone();
    let rows: Vec<usize> = ctx.selection.rows.iter().copied().collect();
    let playlist = ctx.selection.playlist;
    ui.set_min_width(200.0);
    ui.label(RichText::new(format!("{} selected", plural(ids.len(), "track", "tracks"))).small().color(theme::TEXT_DIM));
    if ui.button("Play").clicked() {
        ctx.actions.push(UiAction::Play(ids.clone(), 0));
        ui.close();
    }
    if ui.button("Play next").clicked() {
        ctx.actions.push(UiAction::PlayNext(ids.clone()));
        ui.close();
    }
    if ui.button("Add to queue").clicked() {
        ctx.actions.push(UiAction::AddToQueue(ids.clone()));
        ui.close();
    }
    ui.menu_button("Add to playlist", |ui| add_to_playlist_menu(ui, ctx, &ids, playlist));
    if let Some(pl) = playlist {
        if ui.button("Remove from this playlist").clicked() {
            ctx.actions.push(UiAction::RemoveFromPlaylist(pl, rows));
            ui.close();
        }
    }
}

/// Floating bar with actions for the selection, shown while two or more tracks are selected.
/// Draw it last on the page, `bottom` is the bottom edge of the content area.
pub fn selection_bar(ui: &mut egui::Ui, ctx: &mut ViewCtx, bottom: egui::Pos2) {
    if !ctx.selection.is_multi() {
        return;
    }
    let ids = ctx.selection.ids.clone();
    let rows: Vec<usize> = ctx.selection.rows.iter().copied().collect();
    let playlist = ctx.selection.playlist;
    let mut clear = false;
    egui::Area::new(egui::Id::unique("selection-bar"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(bottom - egui::vec2(0.0, 14.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).corner_radius(22.0).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(RichText::new(format!("{} selected", ids.len())).strong());
                    ui.separator();
                    if icons::button(ui, Icon::Play, 26.0, false).on_hover_text("Play the selection").clicked() {
                        ctx.actions.push(UiAction::Play(ids.clone(), 0));
                    }
                    if icons::button(ui, Icon::Queue, 26.0, false).on_hover_text("Add to queue").clicked() {
                        ctx.actions.push(UiAction::AddToQueue(ids.clone()));
                    }
                    let add = icons::button(ui, Icon::Plus, 26.0, false).on_hover_text("Add to playlist");
                    egui::Popup::menu(&add).show(|ui| add_to_playlist_menu(ui, ctx, &ids, playlist));
                    if let Some(pl) = playlist {
                        if icons::button(ui, Icon::Trash, 26.0, false).on_hover_text("Remove from this playlist").clicked() {
                            ctx.actions.push(UiAction::RemoveFromPlaylist(pl, rows.clone()));
                        }
                    }
                    ui.separator();
                    if icons::button(ui, Icon::Close, 22.0, false).on_hover_text("Clear the selection (Esc)").clicked() {
                        clear = true;
                    }
                });
            });
        });
    if clear {
        ctx.selection.clear();
    }
}

pub fn track_menu(ui: &mut egui::Ui, ctx: &mut ViewCtx, id: Id, ids: &[Id], row: usize, playlist: Option<Id>) {
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
    ui.menu_button("Add to playlist", |ui| add_to_playlist_menu(ui, ctx, &[id], playlist));
    if let Some(pl) = playlist {
        if ui.button("Remove from this playlist").clicked() {
            ctx.actions.push(UiAction::RemoveFromPlaylist(pl, vec![row]));
            ui.close();
        }
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
    if ui.button("Track info…").clicked() {
        ctx.actions.push(UiAction::TrackInfo(id));
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
    on_click: impl FnMut(usize, &egui::Response),
) {
    card_grid_at(ui, art, n, round, salt, None, item, on_click);
}

/// [`card_grid`] that first scrolls so the row holding card `scroll_to` is at the top.
#[allow(clippy::too_many_arguments)]
pub fn card_grid_at<'a>(
    ui: &mut egui::Ui,
    art: &mut ArtCache,
    n: usize,
    round: bool,
    salt: &str,
    scroll_to: Option<usize>,
    item: impl Fn(usize) -> CardItem<'a>,
    mut on_click: impl FnMut(usize, &egui::Response),
) {
    if n == 0 {
        ui.label(RichText::new("Nothing here").color(theme::TEXT_DIM));
        return;
    }
    let (cols, w, row_h) = grid_metrics(ui.available_width());
    let rows = n.div_ceil(cols);
    let mut area = egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]);
    if let Some(i) = scroll_to {
        area = area.vertical_scroll_offset((i / cols) as f32 * (row_h + ui.spacing().item_spacing.y));
    }
    area.show_rows(ui, row_h, rows, |ui, range| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_toggle_and_range_select() {
        let ids = [10, 11, 12, 13, 14];
        let mut s = Selection::default();
        s.click("t", 1, &ids, false, false, None);
        assert_eq!(s.ids, vec![11]);
        // Ctrl adds and removes single rows.
        s.click("t", 3, &ids, true, false, None);
        assert_eq!(s.ids, vec![11, 13]);
        s.click("t", 1, &ids, true, false, None);
        assert_eq!(s.ids, vec![13]);
        // Shift selects from the anchor (the last clicked row, 1) to here.
        s.click("t", 0, &ids, false, true, None);
        assert_eq!(s.ids, vec![10, 11]);
        s.click("t", 3, &ids, false, true, None);
        assert_eq!(s.ids, vec![11, 12, 13]);
        // A plain click starts over; a click in another list replaces the selection.
        s.click("t", 4, &ids, false, false, None);
        assert_eq!(s.ids, vec![14]);
        s.click("other", 0, &ids, true, false, Some(7));
        assert_eq!((s.list.as_str(), s.ids.clone(), s.playlist), ("other", vec![10], Some(7)));
    }

    #[test]
    fn selection_is_dropped_when_the_list_changes() {
        let ids = [10, 11, 12];
        let mut s = Selection::default();
        s.select_all("t", &ids, None);
        assert!(s.is_multi());
        s.validate("t", &ids);
        assert_eq!(s.rows.len(), 3);
        // Another list's contents don't matter.
        s.validate("u", &[1]);
        assert_eq!(s.rows.len(), 3);
        // Re-sorted: same length, different order.
        s.validate("t", &[12, 11, 10]);
        assert!(s.rows.is_empty());
        s.select_all("t", &ids, None);
        s.validate("t", &[10, 11]);
        assert!(s.rows.is_empty(), "shorter list clears the selection");
    }
}
