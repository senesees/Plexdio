//! Library browser (Artists / Albums / Tracks / Genres tabs with filter + sort) and the
//! playlists overview.

use super::common::*;
use crate::library_model::{sort_key, Id, LibraryModel};
use crate::ui::theme;
use egui::RichText;
use std::cmp::Reverse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Title,
    Artist,
    Year,
    Added,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtistSort {
    #[default]
    Name,
    Added,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// Persistent browse state (filter text, sorts) plus memoised sorted/filtered index lists.
#[derive(Default)]
pub struct BrowseState {
    pub filter: String,
    pub album_sort: AlbumSort,
    pub artist_sort: ArtistSort,
    pub track_sort: TrackSort,
    pub track_desc: bool,
    memo: std::collections::HashMap<&'static str, (u64, Vec<usize>)>,
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

impl BrowseState {
    fn memo(&mut self, name: &'static str, key: u64, compute: impl FnOnce() -> Vec<usize>) -> &[usize] {
        let fresh = self.memo.get(name).is_none_or(|(k, _)| *k != key);
        if fresh {
            self.memo.insert(name, (key, compute()));
        }
        &self.memo[name].1
    }

    pub fn albums(&mut self, lib: &LibraryModel, ver: u64) -> Vec<usize> {
        let f = self.filter.trim().to_lowercase();
        let sort = self.album_sort;
        let key = fingerprint(&[&ver, &f, &sort]);
        self.memo("albums", key, || {
            let mut v: Vec<usize> = (0..lib.albums.len())
                .filter(|&i| matches(&lib.albums[i].title, &f) || matches(&lib.albums[i].artist, &f))
                .collect();
            match sort {
                AlbumSort::Title => {} // model order is by title
                AlbumSort::Artist => v.sort_by_cached_key(|&i| (sort_key(&lib.albums[i].artist), Reverse(lib.albums[i].year))),
                AlbumSort::Year => v.sort_by_key(|&i| Reverse(lib.albums[i].year)),
                AlbumSort::Added => v.sort_by_key(|&i| Reverse(lib.albums[i].added_at)),
            }
            v
        })
        .to_vec()
    }

    pub fn artists(&mut self, lib: &LibraryModel, ver: u64) -> Vec<usize> {
        let f = self.filter.trim().to_lowercase();
        let sort = self.artist_sort;
        let key = fingerprint(&[&ver, &f, &sort]);
        self.memo("artists", key, || {
            let mut v: Vec<usize> = (0..lib.artists.len()).filter(|&i| matches(&lib.artists[i].title, &f)).collect();
            if sort == ArtistSort::Added {
                v.sort_by_key(|&i| Reverse(lib.artists[i].added_at));
            }
            v
        })
        .to_vec()
    }

    pub fn tracks(&mut self, lib: &LibraryModel, ver: u64) -> Vec<Id> {
        let f = self.filter.trim().to_lowercase();
        let (sort, desc) = (self.track_sort, self.track_desc);
        let key = fingerprint(&[&ver, &f, &sort, &desc]);
        let idx = self
            .memo("tracks", key, || {
                let mut v: Vec<usize> = (0..lib.tracks.len())
                    .filter(|&i| {
                        let t = &lib.tracks[i];
                        f.is_empty() || matches(&t.title, &f) || matches(t.display_artist(), &f) || matches(&t.album, &f)
                    })
                    .collect();
                let t = &lib.tracks;
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
    match tab {
        LibTab::Albums => {
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter albums");
                ui.add_space(12.0);
                ui.label(RichText::new("Sort").color(theme::TEXT_DIM));
                for (s, l) in [(AlbumSort::Title, "Title"), (AlbumSort::Artist, "Artist"), (AlbumSort::Year, "Year"), (AlbumSort::Added, "Recently added")] {
                    ui.selectable_value(&mut state.album_sort, s, l);
                }
            });
            ui.add_space(8.0);
            let v = state.albums(lib, ver);
            ui.label(RichText::new(format!("{} albums", v.len())).small().color(theme::TEXT_DIM));
            let mut open = None;
            card_grid(
                ui,
                ctx.art,
                v.len(),
                false,
                "albums-grid",
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
            if let Some(id) = open {
                ctx.actions.push(UiAction::Navigate(Route::Album(id)));
            }
        }
        LibTab::Artists => {
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter artists");
                ui.add_space(12.0);
                ui.label(RichText::new("Sort").color(theme::TEXT_DIM));
                ui.selectable_value(&mut state.artist_sort, ArtistSort::Name, "Name");
                ui.selectable_value(&mut state.artist_sort, ArtistSort::Added, "Recently added");
            });
            ui.add_space(8.0);
            let v = state.artists(lib, ver);
            ui.label(RichText::new(format!("{} artists", v.len())).small().color(theme::TEXT_DIM));
            let mut open = None;
            card_grid(
                ui,
                ctx.art,
                v.len(),
                true,
                "artists-grid",
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
            if let Some(id) = open {
                ctx.actions.push(UiAction::Navigate(Route::Artist(id)));
            }
        }
        LibTab::Tracks => {
            ui.horizontal(|ui| {
                filter_box(ui, &mut state.filter, "Filter tracks");
                ui.add_space(12.0);
                ui.label(RichText::new("Sort").color(theme::TEXT_DIM));
                for (s, l) in [
                    (TrackSort::Title, "Title"),
                    (TrackSort::Artist, "Artist"),
                    (TrackSort::Album, "Album"),
                    (TrackSort::Duration, "Length"),
                    (TrackSort::Plays, "Plays"),
                    (TrackSort::Rating, "Rating"),
                    (TrackSort::Added, "Added"),
                ] {
                    ui.selectable_value(&mut state.track_sort, s, l);
                }
                if ui.selectable_label(state.track_desc, if state.track_desc { "Descending" } else { "Ascending" }).clicked() {
                    state.track_desc = !state.track_desc;
                }
            });
            ui.add_space(8.0);
            let ids = state.tracks(lib, ver);
            ui.label(RichText::new(format!("{} tracks", ids.len())).small().color(theme::TEXT_DIM));
            track_table(ui, ctx, &ids, TableOpts { album: true, artist: true, track_numbers: false, art: true }, "tracks-table");
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
    ui.heading("Playlists");
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
