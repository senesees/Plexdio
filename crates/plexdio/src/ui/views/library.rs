//! Library browser (Artists / Albums / Tracks / Genres tabs with filter, sort, filter chips and
//! an A–Z jump bar) and the playlists overview.

use super::common::*;
use crate::library_model::{sort_key, Id, LibraryModel};
use crate::ui::theme;
use egui::RichText;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum AlbumSort {
    #[default]
    Title,
    Artist,
    Year,
    Added,
    Plays,
    Rating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum ArtistSort {
    #[default]
    Name,
    Added,
    Plays,
    Albums,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum TrackSort {
    #[default]
    Title,
    Artist,
    Album,
    Duration,
    Plays,
    Rating,
    Added,
}

/// Minimum rating for the Rating filter chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub enum RatingFilter {
    #[default]
    Any,
    /// Any rating at all.
    Rated,
    ThreeStars,
    FiveStars,
}

impl RatingFilter {
    /// Lowest accepted rating on the 0..=10 scale.
    fn min(self) -> Option<f32> {
        match self {
            RatingFilter::Any => None,
            RatingFilter::Rated => Some(0.5),
            RatingFilter::ThreeStars => Some(6.0),
            RatingFilter::FiveStars => Some(10.0),
        }
    }

    fn label(self) -> &'static str {
        match self {
            RatingFilter::Any => "Any rating",
            RatingFilter::Rated => "Rated",
            RatingFilter::ThreeStars => "3★ and up",
            RatingFilter::FiveStars => "5★",
        }
    }
}

/// Filter chips shared by the Artists, Albums and Tracks tabs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct LibFilters {
    pub unplayed: bool,
    pub rating: RatingFilter,
    /// First year of a decade (1990 = the 1990s).
    pub decade: Option<i32>,
    pub genre: Option<String>,
}

impl LibFilters {
    fn active(&self) -> bool {
        *self != Self::default()
    }
}

/// Sorts and filters of the library tabs, kept in the config across restarts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryPrefs {
    pub album_sort: AlbumSort,
    pub album_desc: bool,
    pub artist_sort: ArtistSort,
    pub artist_desc: bool,
    pub track_sort: TrackSort,
    pub track_desc: bool,
    pub filters: LibFilters,
}

/// Per-album and per-artist play/rating totals, rebuilt when the library changes.
#[derive(Default)]
struct Stats {
    ver: Option<u64>,
    /// Plays and mean rating (of rated tracks, 0 when none) per album index.
    album: Vec<(u64, f32)>,
    /// Plays per artist index.
    artist_plays: Vec<u64>,
    /// Genres of each artist (its own and its albums').
    artist_genres: Vec<Vec<Arc<str>>>,
}

/// Persistent browse state (filter text, sorts, chips) plus memoised sorted/filtered lists.
#[derive(Default)]
pub struct BrowseState {
    pub filter: String,
    pub prefs: LibraryPrefs,
    /// Prefs changed this frame (the app saves them to the config).
    pub prefs_changed: bool,
    /// Row/card to scroll to on the next frame (A–Z jump bar).
    scroll_to: Option<usize>,
    stats: Stats,
    memo: std::collections::HashMap<&'static str, (u64, Vec<usize>)>,
    /// Jump bar letters per tab, for the list they were computed from.
    letters: std::collections::HashMap<&'static str, (u64, Vec<(char, usize)>)>,
}

fn fingerprint(parts: &[&dyn std::fmt::Debug]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        format!("{p:?}").hash(&mut h);
    }
    h.finish()
}

pub fn matches(hay: &str, needle_lc: &str) -> bool {
    needle_lc.is_empty() || hay.to_lowercase().contains(needle_lc)
}

/// Jump bar bucket of a sort key: `A`..=`Z` (accents folded), anything else is `#`.
pub fn letter_of(key: &str) -> char {
    match key.chars().next().map(super::search::fold) {
        Some(c @ 'a'..='z') => c.to_ascii_uppercase(),
        _ => '#',
    }
}

/// A title without a leading "The " (the model sorts titles that way, see `sort_key`).
fn strip_the(s: &str) -> &str {
    let t = s.trim_start();
    match t.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("the ") => &t[4..],
        _ => t,
    }
}

/// Decade (first year) of a year, `None` when unknown.
fn decade_of(year: i32) -> Option<i32> {
    (year >= 1000).then_some(year / 10 * 10)
}

impl BrowseState {
    pub fn with_prefs(prefs: LibraryPrefs) -> Self {
        Self { prefs, ..Default::default() }
    }

    /// Jump bar letters for `tab`, recomputed only when the list changes.
    fn letters<'a>(&mut self, tab: &'static str, ver: u64, keys: impl FnOnce() -> Vec<&'a str>) -> Vec<(char, usize)> {
        let key = fingerprint(&[&ver, &self.filter.trim().to_lowercase(), &self.prefs]);
        if self.letters.get(tab).is_none_or(|(k, _)| *k != key) {
            self.letters.insert(tab, (key, letter_index(keys().into_iter())));
        }
        self.letters[tab].1.clone()
    }

    fn memo(&mut self, name: &'static str, key: u64, compute: impl FnOnce(&Stats) -> Vec<usize>) -> &[usize] {
        let fresh = self.memo.get(name).is_none_or(|(k, _)| *k != key);
        if fresh {
            let v = compute(&self.stats);
            self.memo.insert(name, (key, v));
        }
        &self.memo[name].1
    }

    fn ensure_stats(&mut self, lib: &LibraryModel, ver: u64) {
        if self.stats.ver == Some(ver) {
            return;
        }
        let mut album = vec![(0u64, 0f32, 0u32); lib.albums.len()];
        let album_idx: std::collections::HashMap<Id, usize> = lib.albums.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let artist_idx: std::collections::HashMap<Id, usize> = lib.artists.iter().enumerate().map(|(i, a)| (a.id, i)).collect();
        let mut artist_plays = vec![0u64; lib.artists.len()];
        for t in &lib.tracks {
            if let Some(&ai) = album_idx.get(&t.album_id) {
                album[ai].0 += t.view_count as u64;
                if t.user_rating > 0.0 {
                    album[ai].1 += t.user_rating;
                    album[ai].2 += 1;
                }
            }
            if let Some(&ri) = artist_idx.get(&t.artist_id) {
                artist_plays[ri] += t.view_count as u64;
            }
        }
        let mut artist_genres: Vec<Vec<Arc<str>>> = lib.artists.iter().map(|a| a.genre.iter().cloned().collect()).collect();
        for al in &lib.albums {
            if let (Some(&ri), Some(g)) = (artist_idx.get(&al.artist_id), &al.genre) {
                if !artist_genres[ri].contains(g) {
                    artist_genres[ri].push(g.clone());
                }
            }
        }
        self.stats = Stats {
            ver: Some(ver),
            album: album.into_iter().map(|(p, r, n)| (p, if n > 0 { r / n as f32 } else { 0.0 })).collect(),
            artist_plays,
            artist_genres,
        };
    }

    pub fn albums(&mut self, lib: &LibraryModel, ver: u64) -> Vec<usize> {
        self.ensure_stats(lib, ver);
        let f = self.filter.trim().to_lowercase();
        let (sort, desc, chips) = (self.prefs.album_sort, self.prefs.album_desc, self.prefs.filters.clone());
        let key = fingerprint(&[&ver, &f, &sort, &desc, &chips]);
        self.memo("albums", key, |st| {
            let a = &lib.albums;
            let mut v: Vec<usize> = (0..a.len())
                .filter(|&i| matches(&a[i].title, &f) || matches(&a[i].artist, &f))
                .filter(|&i| !chips.unplayed || st.album[i].0 == 0)
                .filter(|&i| chips.rating.min().is_none_or(|m| st.album[i].1 >= m))
                .filter(|&i| chips.decade.is_none_or(|d| decade_of(a[i].year) == Some(d)))
                .filter(|&i| chips.genre.as_deref().is_none_or(|g| a[i].genre.as_deref() == Some(g)))
                .collect();
            match sort {
                AlbumSort::Title => {} // model order is by title
                AlbumSort::Artist => v.sort_by_cached_key(|&i| (sort_key(&a[i].artist), Reverse(a[i].year))),
                AlbumSort::Year => v.sort_by_key(|&i| Reverse(a[i].year)),
                AlbumSort::Added => v.sort_by_key(|&i| Reverse(a[i].added_at)),
                AlbumSort::Plays => v.sort_by_key(|&i| Reverse(st.album[i].0)),
                AlbumSort::Rating => v.sort_by(|&x, &y| st.album[y].1.total_cmp(&st.album[x].1)),
            }
            if desc {
                v.reverse();
            }
            v
        })
        .to_vec()
    }

    pub fn artists(&mut self, lib: &LibraryModel, ver: u64) -> Vec<usize> {
        self.ensure_stats(lib, ver);
        let f = self.filter.trim().to_lowercase();
        let (sort, desc, chips) = (self.prefs.artist_sort, self.prefs.artist_desc, self.prefs.filters.clone());
        let key = fingerprint(&[&ver, &f, &sort, &desc, &chips]);
        self.memo("artists", key, |st| {
            let a = &lib.artists;
            let n_albums = |i: usize| lib.artist_albums.get(&a[i].id).map_or(0, Vec::len);
            let mut v: Vec<usize> = (0..a.len())
                .filter(|&i| matches(&a[i].title, &f))
                .filter(|&i| !chips.unplayed || st.artist_plays[i] == 0)
                .filter(|&i| chips.genre.as_deref().is_none_or(|g| st.artist_genres[i].iter().any(|x| &**x == g)))
                .collect();
            match sort {
                ArtistSort::Name => {}
                ArtistSort::Added => v.sort_by_key(|&i| Reverse(a[i].added_at)),
                ArtistSort::Plays => v.sort_by_key(|&i| Reverse(st.artist_plays[i])),
                ArtistSort::Albums => v.sort_by_key(|&i| Reverse(n_albums(i))),
            }
            if desc {
                v.reverse();
            }
            v
        })
        .to_vec()
    }

    pub fn tracks(&mut self, lib: &LibraryModel, ver: u64) -> Vec<Id> {
        self.ensure_stats(lib, ver);
        let f = self.filter.trim().to_lowercase();
        let (sort, desc, chips) = (self.prefs.track_sort, self.prefs.track_desc, self.prefs.filters.clone());
        let key = fingerprint(&[&ver, &f, &sort, &desc, &chips]);
        let idx = self
            .memo("tracks", key, |_| {
                let t = &lib.tracks;
                let year = |i: usize| if t[i].year > 0 { t[i].year } else { lib.album(t[i].album_id).map_or(0, |a| a.year) };
                let genre = |i: usize| t[i].genre.as_deref().or_else(|| lib.album(t[i].album_id).and_then(|a| a.genre.as_deref()));
                let mut v: Vec<usize> = (0..t.len())
                    .filter(|&i| f.is_empty() || matches(&t[i].title, &f) || matches(t[i].display_artist(), &f) || matches(&t[i].album, &f))
                    .filter(|&i| !chips.unplayed || t[i].view_count == 0)
                    .filter(|&i| chips.rating.min().is_none_or(|m| t[i].user_rating >= m))
                    .filter(|&i| chips.decade.is_none_or(|d| decade_of(year(i)) == Some(d)))
                    .filter(|&i| chips.genre.as_deref().is_none_or(|g| genre(i) == Some(g)))
                    .collect();
                match sort {
                    TrackSort::Title => {}
                    TrackSort::Artist => v.sort_by_cached_key(|&i| (sort_key(t[i].display_artist()), sort_key(&t[i].album), t[i].disc, t[i].index)),
                    TrackSort::Album => v.sort_by_cached_key(|&i| (sort_key(&t[i].album), t[i].disc, t[i].index)),
                    TrackSort::Duration => v.sort_by_key(|&i| t[i].duration_ms),
                    TrackSort::Plays => v.sort_by_key(|&i| Reverse(t[i].view_count)),
                    TrackSort::Rating => v.sort_by_key(|&i| Reverse((t[i].user_rating * 10.0) as i32)),
                    TrackSort::Added => v.sort_by_key(|&i| Reverse(t[i].added_at)),
                }
                if desc {
                    v.reverse();
                }
                v
            })
            .to_vec();
        idx.into_iter().map(|i| lib.tracks[i].id).collect()
    }
}

fn tab_button(ui: &mut egui::Ui, current: LibTab, tab: LibTab, label: &str) -> bool {
    let sel = current == tab;
    let text = RichText::new(label).size(14.0).color(if sel { egui::Color32::BLACK } else { theme::TEXT });
    let fill = if sel { theme::TEXT } else { egui::Color32::from_rgb(42, 42, 42) };
    ui.add(egui::Button::new(text).fill(fill).corner_radius(16).min_size(egui::vec2(70.0, 30.0))).clicked()
}

fn filter_box(ui: &mut egui::Ui, filter: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text(hint.to_string())
            .desired_width(220.0)
            .margin(egui::Margin::symmetric(10, 6)),
    );
    if !filter.is_empty() && crate::ui::icons::button(ui, crate::ui::icons::Icon::Close, 18.0, false).clicked() {
        filter.clear();
    }
}

/// "Sort ▾" combo plus the ascending/descending toggle. Returns true when something changed.
fn sort_controls<T: PartialEq + Copy>(ui: &mut egui::Ui, salt: &str, sort: &mut T, desc: &mut bool, options: &[(T, &str)]) -> bool {
    let before = (*sort, *desc);
    let current = options.iter().find(|(s, _)| s == sort).map_or("", |(_, l)| l);
    egui::ComboBox::from_id_salt(salt).selected_text(format!("Sort: {current}")).width(150.0).show_ui(ui, |ui| {
        for (s, l) in options {
            ui.selectable_value(sort, *s, *l);
        }
    });
    if chip(ui, *desc, "Reverse").on_hover_text("Show the list in the opposite order").clicked() {
        *desc = !*desc;
    }
    before != (*sort, *desc)
}

/// A pill-shaped toggle used for the filter chips.
fn chip(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(13.0).color(if on { egui::Color32::BLACK } else { theme::TEXT });
    let fill = if on { theme::ACCENT } else { egui::Color32::from_rgb(42, 42, 42) };
    ui.add(egui::Button::new(text).fill(fill).corner_radius(14).min_size(egui::vec2(0.0, 26.0)))
}

/// Filter chips (Unplayed, Rating, Decade, Genre and Clear). `full` adds Rating and Decade
/// (Albums and Tracks; artists have neither). Returns true when a filter changed.
fn filter_chips(ui: &mut egui::Ui, lib: &LibraryModel, f: &mut LibFilters, full: bool) -> bool {
    let before = f.clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if chip(ui, f.unplayed, "Unplayed").clicked() {
            f.unplayed = !f.unplayed;
        }
        if full {
            let r = chip(ui, f.rating != RatingFilter::Any, if f.rating == RatingFilter::Any { "Rating ▾" } else { f.rating.label() });
            egui::Popup::menu(&r).show(|ui| {
                for opt in [RatingFilter::Any, RatingFilter::Rated, RatingFilter::ThreeStars, RatingFilter::FiveStars] {
                    if ui.selectable_label(f.rating == opt, opt.label()).clicked() {
                        f.rating = opt;
                        ui.close();
                    }
                }
            });
            let label = f.decade.map_or("Decade ▾".to_string(), |d| format!("{d}s"));
            let r = chip(ui, f.decade.is_some(), &label);
            egui::Popup::menu(&r).show(|ui| {
                let mut decades: Vec<i32> = lib.albums.iter().filter_map(|a| decade_of(a.year)).collect();
                decades.sort_unstable_by(|a, b| b.cmp(a));
                decades.dedup();
                if ui.selectable_label(f.decade.is_none(), "Any decade").clicked() {
                    f.decade = None;
                    ui.close();
                }
                for d in decades {
                    if ui.selectable_label(f.decade == Some(d), format!("{d}s")).clicked() {
                        f.decade = Some(d);
                        ui.close();
                    }
                }
            });
        }
        let label = f.genre.clone().unwrap_or_else(|| "Genre ▾".to_string());
        let r = chip(ui, f.genre.is_some(), &label);
        egui::Popup::menu(&r).show(|ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                if ui.selectable_label(f.genre.is_none(), "Any genre").clicked() {
                    f.genre = None;
                    ui.close();
                }
                let mut genres: Vec<&str> = lib.genres.iter().map(|(g, _)| &**g).collect();
                genres.sort_by_key(|g| g.to_lowercase());
                for g in genres {
                    if ui.selectable_label(f.genre.as_deref() == Some(g), g).clicked() {
                        f.genre = Some(g.to_string());
                        ui.close();
                    }
                }
            });
        });
        if f.active() && ui.link(RichText::new("Clear filters").size(13.0)).clicked() {
            *f = LibFilters::default();
        }
    });
    before != *f
}

/// First index per letter of a list in name order (for the jump bar). `keys` are sort keys in
/// display order.
pub fn letter_index<'a>(keys: impl Iterator<Item = &'a str>) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::new();
    for (i, k) in keys.enumerate() {
        let l = letter_of(k);
        if !out.iter().any(|(c, _)| *c == l) {
            out.push((l, i));
        }
    }
    out
}

const JUMP_W: f32 = 22.0;

/// Thin column of letters on the right; clicking one returns the index to scroll to. Letters
/// without entries are dimmed.
fn jump_bar(ui: &mut egui::Ui, rect: egui::Rect, index: &[(char, usize)]) -> Option<usize> {
    let letters: Vec<char> = std::iter::once('#').chain('A'..='Z').collect();
    let step = (rect.height() / letters.len() as f32).clamp(10.0, 22.0);
    let size = (step * 0.62).clamp(9.0, 12.5);
    let top = rect.top() + ((rect.height() - step * letters.len() as f32) / 2.0).max(0.0);
    let mut out = None;
    for (n, l) in letters.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(egui::pos2(rect.left(), top + n as f32 * step), egui::vec2(rect.width(), step));
        let target = index.iter().find(|(c, _)| *c == l).map(|(_, i)| *i);
        let resp = ui.interact(r, ui.scope_id().with(("jump", l)), if target.is_some() { egui::Sense::click() } else { egui::Sense::hover() });
        let color = match (target.is_some(), resp.hovered()) {
            (false, _) => theme::TEXT_DIM.linear_multiply(0.35),
            (true, true) => theme::ACCENT,
            (true, false) => theme::TEXT_DIM,
        };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, l, egui::FontId::proportional(size), color);
        if resp.clicked() {
            out = target;
        }
    }
    out
}

/// Split the remaining area into the list and (when `show`) a jump bar on its right; runs
/// `list` in the left part and returns the jump bar's pick.
fn with_jump_bar(ui: &mut egui::Ui, show: bool, index: &[(char, usize)], list: impl FnOnce(&mut egui::Ui)) -> Option<usize> {
    let avail = ui.available_rect_before_wrap();
    if !show || avail.height() < 200.0 {
        list(ui);
        return None;
    }
    let list_rect = egui::Rect::from_min_max(avail.min, egui::pos2(avail.right() - JUMP_W - 4.0, avail.bottom()));
    let bar_rect = egui::Rect::from_min_max(egui::pos2(avail.right() - JUMP_W, avail.top()), avail.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(list_rect), list);
    jump_bar(ui, bar_rect, index)
}

pub fn library_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, state: &mut BrowseState, tab: LibTab, ver: u64) {
    ui.horizontal(|ui| {
        ui.heading("Library");
        ui.add_space(16.0);
        for (t, label) in [(LibTab::Artists, "Artists"), (LibTab::Albums, "Albums"), (LibTab::Tracks, "Tracks"), (LibTab::Genres, "Genres")] {
            if tab_button(ui, tab, t, label) && t != tab {
                ctx.actions.push(UiAction::Navigate(Route::Library(t)));
            }
        }
    });
    ui.add_space(8.0);
    let lib = ctx.lib;
    let scroll_to = state.scroll_to.take();
    match tab {
        LibTab::Albums => {
            let mut changed = false;
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter albums");
                ui.add_space(8.0);
                let p = &mut state.prefs;
                changed |= sort_controls(
                    ui,
                    "album-sort",
                    &mut p.album_sort,
                    &mut p.album_desc,
                    &[
                        (AlbumSort::Title, "Title"),
                        (AlbumSort::Artist, "Artist"),
                        (AlbumSort::Year, "Year"),
                        (AlbumSort::Added, "Recently added"),
                        (AlbumSort::Plays, "Most played"),
                        (AlbumSort::Rating, "Rating"),
                    ],
                );
            });
            ui.add_space(4.0);
            changed |= filter_chips(ui, lib, &mut state.prefs.filters, true);
            state.prefs_changed |= changed;
            ui.add_space(6.0);
            let v = state.albums(lib, ver);
            ui.label(RichText::new(plural(v.len(), "album", "albums")).small().color(theme::TEXT_DIM));
            let by_name = state.prefs.album_sort == AlbumSort::Title;
            let index = if by_name { state.letters("albums", ver, || v.iter().map(|&i| strip_the(&lib.albums[i].title)).collect()) } else { Vec::new() };
            let mut open = None;
            state.scroll_to = with_jump_bar(ui, by_name, &index, |ui| {
                card_grid_at(
                    ui,
                    ctx.art,
                    v.len(),
                    false,
                    "albums-grid",
                    scroll_to,
                    |i| {
                        let a = &lib.albums[v[i]];
                        CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: &a.artist, seed: &a.title }
                    },
                    |i, r| {
                        if r.clicked() {
                            open = Some(lib.albums[v[i]].id);
                        }
                    },
                );
            });
            if let Some(id) = open {
                ctx.actions.push(UiAction::Navigate(Route::Album(id)));
            }
        }
        LibTab::Artists => {
            let mut changed = false;
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter artists");
                ui.add_space(8.0);
                let p = &mut state.prefs;
                changed |= sort_controls(
                    ui,
                    "artist-sort",
                    &mut p.artist_sort,
                    &mut p.artist_desc,
                    &[(ArtistSort::Name, "Name"), (ArtistSort::Added, "Recently added"), (ArtistSort::Plays, "Most played"), (ArtistSort::Albums, "Most albums")],
                );
            });
            ui.add_space(4.0);
            changed |= filter_chips(ui, lib, &mut state.prefs.filters, false);
            state.prefs_changed |= changed;
            ui.add_space(6.0);
            let v = state.artists(lib, ver);
            ui.label(RichText::new(plural(v.len(), "artist", "artists")).small().color(theme::TEXT_DIM));
            let by_name = state.prefs.artist_sort == ArtistSort::Name;
            let index = if by_name { state.letters("artists", ver, || v.iter().map(|&i| lib.artists[i].sort_key.as_ref()).collect()) } else { Vec::new() };
            let mut open = None;
            state.scroll_to = with_jump_bar(ui, by_name, &index, |ui| {
                card_grid_at(
                    ui,
                    ctx.art,
                    v.len(),
                    true,
                    "artists-grid",
                    scroll_to,
                    |i| {
                        let a = &lib.artists[v[i]];
                        CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: a.genre.as_deref().unwrap_or("Artist"), seed: &a.title }
                    },
                    |i, r| {
                        if r.clicked() {
                            open = Some(lib.artists[v[i]].id);
                        }
                    },
                );
            });
            if let Some(id) = open {
                ctx.actions.push(UiAction::Navigate(Route::Artist(id)));
            }
        }
        LibTab::Tracks => {
            let mut changed = false;
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter tracks");
                ui.add_space(8.0);
                let p = &mut state.prefs;
                changed |= sort_controls(
                    ui,
                    "track-sort",
                    &mut p.track_sort,
                    &mut p.track_desc,
                    &[
                        (TrackSort::Title, "Title"),
                        (TrackSort::Artist, "Artist"),
                        (TrackSort::Album, "Album"),
                        (TrackSort::Duration, "Length"),
                        (TrackSort::Plays, "Most played"),
                        (TrackSort::Rating, "Rating"),
                        (TrackSort::Added, "Recently added"),
                    ],
                );
            });
            ui.add_space(4.0);
            changed |= filter_chips(ui, lib, &mut state.prefs.filters, true);
            state.prefs_changed |= changed;
            ui.add_space(6.0);
            let ids = state.tracks(lib, ver);
            ui.label(RichText::new(plural(ids.len(), "track", "tracks")).small().color(theme::TEXT_DIM));
            let by_name = state.prefs.track_sort == TrackSort::Title;
            let index = if by_name { state.letters("tracks", ver, || ids.iter().filter_map(|id| lib.track(*id)).map(|t| strip_the(&t.title)).collect()) } else { Vec::new() };
            let opts = TableOpts { album: true, artist: true, track_numbers: false, art: true, playlist: None };
            state.scroll_to = with_jump_bar(ui, by_name, &index, |ui| track_table_at(ui, ctx, &ids, opts, "tracks-table", scroll_to));
        }
        LibTab::Genres => {
            ui.horizontal(|ui| filter_box(ui, &mut state.filter, "Filter genres"));
            ui.add_space(8.0);
            let f = state.filter.trim().to_lowercase();
            let genres: Vec<&(std::sync::Arc<str>, usize)> = lib.genres.iter().filter(|(g, _)| matches(g, &f)).collect();
            if genres.is_empty() {
                ui.label(RichText::new("No genres").color(theme::TEXT_DIM));
            }
            egui::ScrollArea::vertical().id_salt("genres").auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                    for (g, n) in genres {
                        let (c1, c2) = crate::ui::art::placeholder_colors(g);
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(200.0, 96.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, 10.0, if resp.hovered() { c1 } else { c2 });
                        ui.painter().text(rect.min + egui::vec2(14.0, 14.0), egui::Align2::LEFT_TOP, g, egui::FontId::proportional(19.0), theme::TEXT);
                        ui.painter().text(
                            rect.left_bottom() + egui::vec2(14.0, -12.0),
                            egui::Align2::LEFT_BOTTOM,
                            if *n == 1 { "1 album".to_string() } else { format!("{n} albums") },
                            egui::FontId::proportional(12.5),
                            theme::TEXT_DIM,
                        );
                        if resp.clicked() {
                            ctx.actions.push(UiAction::Navigate(Route::Genre(g.clone())));
                        }
                    }
                });
            });
        }
    }
}

pub fn playlists_view(ui: &mut egui::Ui, ctx: &mut ViewCtx) {
    ui.horizontal(|ui| {
        ui.heading("Playlists");
        ui.add_space(12.0);
        if ui.add(egui::Button::new("+  New playlist").corner_radius(14.0)).clicked() {
            ctx.actions.push(UiAction::NewPlaylist(Vec::new()));
        }
    });
    ui.add_space(8.0);
    let lib = ctx.lib;
    if lib.playlists.is_empty() {
        ui.label(RichText::new("No playlists in this library yet.").color(theme::TEXT_DIM));
        return;
    }
    let subs: Vec<String> = lib
        .playlists
        .iter()
        .map(|p| format!("{}{} tracks · {}", if p.smart { "Smart · " } else { "" }, p.leaf_count, fmt_long_duration(p.duration_ms)))
        .collect();
    let mut open = None;
    card_grid(
        ui,
        ctx.art,
        lib.playlists.len(),
        false,
        "playlists-grid",
        |i| {
            let p = &lib.playlists[i];
            CardItem { thumb: p.composite.as_deref(), title: &p.title, subtitle: &subs[i], seed: &p.title }
        },
        |i, r| {
            if r.clicked() {
                open = Some(lib.playlists[i].id);
            }
        },
    );
    if let Some(id) = open {
        ctx.actions.push(UiAction::Navigate(Route::Playlist(id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::db::{Kind, LibraryDb};
    use crate::plex::models::Metadata;

    fn meta(id: i64, kind: &str, title: &str, parent: i64, year: i64, plays: i64, rating: f64, genre: Option<&str>) -> Metadata {
        Metadata {
            rating_key: Some(id.to_string()),
            title: Some(title.into()),
            kind: Some(kind.into()),
            parent_rating_key: Some(parent.to_string()),
            grandparent_rating_key: Some("1".into()),
            parent_title: Some(format!("Album {parent}")),
            grandparent_title: Some("Artist".into()),
            year: Some(year),
            view_count: Some(plays),
            user_rating: Some(rating),
            duration: Some(1000),
            genres: genre.map(|g| vec![crate::plex::models::Tag { tag: g.into() }]).unwrap_or_default(),
            ..Default::default()
        }
    }

    fn library() -> LibraryModel {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert(Kind::Artist, &[meta(1, "artist", "Artist", 0, 0, 0, 0.0, None)]).unwrap();
        db.upsert(
            Kind::Album,
            &[
                meta(10, "album", "The Apples", 1, 1994, 0, 0.0, Some("Rock")),
                meta(11, "album", "Bananas", 1, 2003, 0, 0.0, Some("Jazz")),
                meta(12, "album", "Éclairs", 1, 1999, 0, 0.0, Some("Rock")),
            ],
        )
        .unwrap();
        db.upsert(
            Kind::Track,
            &[
                meta(100, "track", "One", 10, 0, 3, 8.0, None),
                meta(101, "track", "Two", 11, 0, 0, 0.0, None),
                meta(102, "track", "Three", 12, 0, 1, 10.0, None),
            ],
        )
        .unwrap();
        LibraryModel::load(&db).unwrap()
    }

    #[test]
    fn letters_fold_accents_and_group_the_rest() {
        assert_eq!(letter_of("éclairs"), 'E');
        assert_eq!(letter_of("42nd street"), '#');
        assert_eq!(strip_the("The Apples"), "Apples");
        assert_eq!(strip_the("Theme"), "Theme");
        let idx = letter_index(["apples", "avocado", "bananas", "éclairs", "zebra"].into_iter());
        assert_eq!(idx, vec![('A', 0), ('B', 2), ('E', 3), ('Z', 4)]);
    }

    #[test]
    fn album_filters_and_sorts() {
        let lib = library();
        let mut st = BrowseState::default();
        let titles = |st: &mut BrowseState| -> Vec<String> { st.albums(&lib, 1).iter().map(|&i| lib.albums[i].title.to_string()).collect() };
        assert_eq!(titles(&mut st), vec!["The Apples", "Bananas", "Éclairs"]);
        st.prefs.filters.genre = Some("Rock".into());
        assert_eq!(titles(&mut st), vec!["The Apples", "Éclairs"]);
        st.prefs.filters.decade = Some(1990);
        st.prefs.filters.genre = None;
        assert_eq!(titles(&mut st), vec!["The Apples", "Éclairs"]);
        st.prefs.filters = LibFilters { unplayed: true, ..Default::default() };
        assert_eq!(titles(&mut st), vec!["Bananas"]);
        st.prefs.filters = LibFilters { rating: RatingFilter::FiveStars, ..Default::default() };
        assert_eq!(titles(&mut st), vec!["Éclairs"]);
        st.prefs.filters = LibFilters::default();
        st.prefs.album_sort = AlbumSort::Plays;
        assert_eq!(titles(&mut st), vec!["The Apples", "Éclairs", "Bananas"]);
        st.prefs.album_desc = true;
        assert_eq!(titles(&mut st), vec!["Bananas", "Éclairs", "The Apples"]);
    }

    #[test]
    fn track_filters_use_the_album_year_and_genre() {
        let lib = library();
        let mut st = BrowseState::default();
        st.prefs.filters.decade = Some(2000);
        let ids = st.tracks(&lib, 1);
        assert_eq!(ids, vec![101]);
        st.prefs.filters = LibFilters { genre: Some("Rock".into()), rating: RatingFilter::ThreeStars, ..Default::default() };
        let mut ids = st.tracks(&lib, 1);
        ids.sort();
        assert_eq!(ids, vec![100, 102]);
    }
}
