//! Search: an instant scan over the in-memory library, supplemented by the server's
//! `/hubs/search` (useful when the cache is stale or a query matches something the local
//! snapshot does not have yet).

use super::common::*;
use crate::library_model::{Id, LibraryModel};
use crate::plex::models::Hub;
use crate::ui::theme;
use egui::RichText;

/// How long to wait after the last keystroke before asking the server.
const SERVER_DEBOUNCE: f64 = 0.4;

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// Focus the field on the next frame (Ctrl+F / clicking Search).
    pub focus: bool,
    pub server_for: Option<String>,
    pub server_hubs: Vec<Hub>,
    pub server_running: bool,
    /// Bumped for every server request so late replies are dropped.
    pub server_seq: u64,
    /// Last query seen by the debounce logic.
    pub last_seen: String,
    pub changed_at: f64,
    memo: Option<(String, u64, LocalResults)>,
    /// Keyboard highlight: index into [`LocalResults::hits`].
    cursor: Option<usize>,
}

/// One result the keyboard can move to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Artist(usize),
    Album(usize),
    Playlist(usize),
    Genre(usize),
    /// Position in the shown track list.
    Track(usize),
}

#[derive(Default, Clone)]
pub struct LocalResults {
    /// Library indices, best match first.
    pub artists: Vec<usize>,
    pub artist_total: usize,
    pub albums: Vec<usize>,
    pub album_total: usize,
    /// Track ids, best match first (capped).
    pub tracks: Vec<Id>,
    pub track_total: usize,
    /// `lib.playlists` / `lib.genres` indices.
    pub playlists: Vec<usize>,
    pub genres: Vec<usize>,
    /// The single best match, shown first as a big card.
    pub top: Option<Hit>,
}

impl LocalResults {
    pub fn total(&self) -> usize {
        self.artist_total + self.album_total + self.track_total + self.playlists.len() + self.genres.len()
    }

    /// Every result in page order (top result, artists, albums, playlists, genres, tracks), as
    /// the arrow keys walk them.
    pub fn hits(&self) -> Vec<Hit> {
        let mut v: Vec<Hit> = self.top.into_iter().collect();
        v.extend((0..self.artists.len()).map(|i| Hit::Artist(self.artists[i])));
        v.extend((0..self.albums.len()).map(|i| Hit::Album(self.albums[i])));
        v.extend(self.playlists.iter().map(|&i| Hit::Playlist(i)));
        v.extend(self.genres.iter().map(|&i| Hit::Genre(i)));
        v.extend((0..self.tracks.len().min(MAX_SHOWN_TRACKS)).map(Hit::Track));
        v
    }
}

/// 0 = the whole name matches, 1 = it starts with the query, 2 = it contains it.
fn rank(hay: &str, needle: &[char]) -> Option<u8> {
    let s = match_score(hay, needle)?;
    Some(if s == 0 && hay.chars().count() == needle.len() { 0 } else { s + 1 })
}

/// Pick the top result: the best ranked first hit of each kind; ties go to artists, then
/// albums, playlists and tracks (Spotify's order).
fn top_hit(lib: &LibraryModel, out: &LocalResults, needle: &[char]) -> Option<Hit> {
    let mut best: Option<(u8, u8, Hit)> = None;
    let mut consider = |r: Option<u8>, prio: u8, hit: Hit| {
        if let Some(r) = r {
            if best.is_none_or(|(br, bp, _)| (r, prio) < (br, bp)) {
                best = Some((r, prio, hit));
            }
        }
    };
    if let Some(&i) = out.artists.first() {
        consider(rank(&lib.artists[i].title, needle), 0, Hit::Artist(i));
    }
    if let Some(&i) = out.albums.first() {
        consider(rank(&lib.albums[i].title, needle), 1, Hit::Album(i));
    }
    if let Some(&i) = out.playlists.first() {
        consider(rank(&lib.playlists[i].title, needle), 2, Hit::Playlist(i));
    }
    if let Some(t) = out.tracks.first().and_then(|id| lib.track(*id)) {
        consider(rank(&t.title, needle), 3, Hit::Track(0));
    }
    best.map(|b| b.2)
}

pub(crate) use crate::library_model::fold;

/// 0 = prefix, 1 = substring, `None` = no match.
fn match_score(hay: &str, needle: &[char]) -> Option<u8> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut matched = 0usize;
    for (idx, c) in hay.chars().enumerate() {
        let f = fold(c);
        if f == needle[matched] {
            matched += 1;
            if matched == needle.len() {
                return Some(if idx + 1 == needle.len() { 0 } else { 1 });
            }
        } else {
            matched = usize::from(f == needle[0]);
        }
    }
    None
}

const MAX_PER_ARTIST: usize = 80;
const MAX_PER_ALBUM: usize = 120;
const MAX_TRACKS_PER_BUCKET: usize = 600;
/// Track rows rendered inline in the results page (the page itself scrolls).
const MAX_SHOWN_TRACKS: usize = 100;

fn search_local(lib: &LibraryModel, query: &str) -> LocalResults {
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut out = LocalResults::default();

    // Artists: kept in name order within each score bucket.
    let mut buckets: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for (i, a) in lib.artists.iter().enumerate() {
        if let Some(s) = match_score(&a.title, &needle) {
            out.artist_total += 1;
            if buckets[s as usize].len() < MAX_PER_ARTIST {
                buckets[s as usize].push(i);
            }
        }
    }
    out.artists = buckets.into_iter().flatten().collect();

    let mut buckets: [Vec<usize>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (i, a) in lib.albums.iter().enumerate() {
        let score = match_score(&a.title, &needle).or_else(|| match_score(&a.artist, &needle).map(|s| s + 2));
        if let Some(s) = score {
            out.album_total += 1;
            let b = &mut buckets[s as usize];
            if b.len() < MAX_PER_ALBUM {
                b.push(i);
            }
        }
    }
    out.albums = buckets.into_iter().flatten().collect();

    let mut buckets: [Vec<Id>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for t in &lib.tracks {
        let score = match_score(&t.title, &needle)
            .or_else(|| match_score(t.display_artist(), &needle).map(|s| s + 2))
            .or_else(|| match_score(&t.album, &needle).map(|s| s + 2));
        if let Some(s) = score {
            out.track_total += 1;
            let b = &mut buckets[s as usize];
            if b.len() < MAX_TRACKS_PER_BUCKET {
                b.push(t.id);
            }
        }
    }
    out.tracks = buckets.into_iter().flatten().collect();

    let mut ranked: Vec<(u8, usize)> = (0..lib.playlists.len()).filter_map(|i| rank(&lib.playlists[i].title, &needle).map(|r| (r, i))).collect();
    ranked.sort_by_key(|p| p.0);
    out.playlists = ranked.into_iter().map(|p| p.1).collect();
    let mut ranked: Vec<(u8, usize)> = (0..lib.genres.len()).filter_map(|i| rank(&lib.genres[i].0, &needle).map(|r| (r, i))).collect();
    ranked.sort_by_key(|p| p.0);
    out.genres = ranked.into_iter().map(|p| p.1).collect();
    out.top = top_hit(lib, &out, &needle);
    out
}

impl SearchState {
    fn results(&mut self, lib: &LibraryModel, ver: u64) -> &LocalResults {
        let q = self.query.trim().to_lowercase();
        let fresh = self.memo.as_ref().is_none_or(|(mq, mv, _)| mq != &q || *mv != ver);
        if fresh {
            if self.memo.as_ref().is_none_or(|(mq, _, _)| mq != &q) {
                self.cursor = None;
            }
            self.memo = Some((q.clone(), ver, search_local(lib, &q)));
        }
        &self.memo.as_ref().expect("just filled").2
    }

    /// True when the server results belong to the current query.
    fn server_is_current(&self) -> bool {
        self.server_for.as_deref() == Some(self.query.trim().to_lowercase().as_str())
    }
}

/// Accent outline around the keyboard-highlighted result, scrolled into view.
fn mark(r: &egui::Response) {
    r.ctx.layer_painter(r.layer_id).rect_stroke(r.rect.expand(2.0), 10.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Outside);
    r.scroll_to_me(None);
}

/// What Enter (or Shift+Enter: add to queue) does with a result.
fn activate(ctx: &mut ViewCtx, hit: Hit, tracks: &[Id], queue: bool) {
    let lib = ctx.lib;
    let ids: Vec<Id> = match hit {
        Hit::Artist(i) => lib.artist_track_ids(lib.artists[i].id),
        Hit::Album(i) => lib.album_track_ids(lib.albums[i].id),
        Hit::Playlist(i) => {
            let p = &lib.playlists[i];
            lib.playlist_rows(p).into_iter().map(|r| p.items[r]).collect()
        }
        Hit::Genre(_) => Vec::new(),
        Hit::Track(k) => tracks.get(k).copied().into_iter().collect(),
    };
    if queue {
        if !ids.is_empty() {
            ctx.actions.push(UiAction::AddToQueue(ids));
        }
        return;
    }
    ctx.actions.push(match hit {
        Hit::Artist(i) => UiAction::Navigate(Route::Artist(lib.artists[i].id)),
        Hit::Album(i) => UiAction::Navigate(Route::Album(lib.albums[i].id)),
        Hit::Playlist(i) => UiAction::Navigate(Route::Playlist(lib.playlists[i].id)),
        Hit::Genre(i) => UiAction::Navigate(Route::Genre(lib.genres[i].0.clone())),
        Hit::Track(k) => UiAction::Play(tracks.to_vec(), k),
    });
}

/// The big "Top result" card: art, name, kind and a play button.
fn top_card(ui: &mut egui::Ui, ctx: &mut ViewCtx, hit: Hit, tracks: &[Id], marked: bool) {
    let lib = ctx.lib;
    let (thumb, title, kind, round): (Option<&str>, &str, String, bool) = match hit {
        Hit::Artist(i) => (lib.artists[i].thumb.as_deref(), &lib.artists[i].title, "Artist".into(), true),
        Hit::Album(i) => (lib.albums[i].thumb.as_deref(), &lib.albums[i].title, format!("Album · {}", lib.albums[i].artist), false),
        Hit::Playlist(i) => (lib.playlists[i].composite.as_deref(), &lib.playlists[i].title, "Playlist".into(), false),
        Hit::Genre(i) => (None, &lib.genres[i].0, "Genre".into(), false),
        Hit::Track(k) => match tracks.get(k).and_then(|id| lib.track(*id)) {
            Some(t) => (t.thumb.as_deref(), &t.title, format!("Song · {}", t.display_artist()), false),
            None => return,
        },
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(440.0), 200.0), egui::Sense::click());
    ui.painter().rect_filled(rect, 10.0, if resp.hovered() { theme::CARD_HOVER } else { theme::CARD });
    let art = egui::Rect::from_min_size(rect.min + egui::vec2(18.0, 18.0), egui::vec2(96.0, 96.0));
    ctx.art.paint(ui, art, thumb, title, if round { crate::ui::art::Shape::Circle } else { crate::ui::art::Shape::Rounded(6) });
    clipped(ui, egui::pos2(rect.left() + 18.0, art.bottom() + 26.0), title, 26.0, theme::TEXT, rect.width() - 36.0, egui::Align2::LEFT_CENTER);
    ui.painter().text(egui::pos2(rect.left() + 18.0, art.bottom() + 56.0), egui::Align2::LEFT_CENTER, kind, egui::FontId::proportional(13.0), theme::TEXT_DIM);
    let play_rect = egui::Rect::from_center_size(rect.right_bottom() - egui::vec2(46.0, 46.0), egui::vec2(48.0, 48.0));
    let play = ui.interact(play_rect, resp.id.with("play"), egui::Sense::click());
    if resp.hovered() || play.hovered() {
        ui.painter().circle_filled(play_rect.center(), 24.0, theme::ACCENT);
        crate::ui::icons::paint(ui.painter(), play_rect.shrink(14.0), crate::ui::icons::Icon::Play, egui::Color32::from_rgb(24, 24, 24));
    }
    if marked {
        mark(&resp);
    }
    if play.clicked() {
        let ids: Vec<Id> = match hit {
            Hit::Track(_) => tracks.to_vec(),
            _ => Vec::new(),
        };
        match hit {
            Hit::Track(k) => ctx.actions.push(UiAction::Play(ids, k)),
            Hit::Genre(_) => activate(ctx, hit, tracks, false),
            _ => {
                activate(ctx, hit, tracks, true);
                // Play: replace the queue instead of appending.
                if let Some(UiAction::AddToQueue(ids)) = ctx.actions.pop() {
                    ctx.actions.push(UiAction::Play(ids, 0));
                }
            }
        }
    } else if resp.clicked() {
        activate(ctx, hit, tracks, false);
    }
}

pub fn search_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, state: &mut SearchState, ver: u64) {
    let lib = ctx.lib;
    // Arrow keys move through the results while the box keeps the focus; Enter opens.
    let (down, up, enter, shift) = ui.input_mut(|i| {
        let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown);
        let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp);
        (down, up, i.key_pressed(egui::Key::Enter), i.modifiers.shift)
    });
    let mut refocus = false;
    ui.horizontal(|ui| {
        ui.heading("Search");
        ui.add_space(12.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Artists, albums, songs, playlists and genres")
                .desired_width(420.0)
                .margin(egui::Margin::symmetric(12, 8)),
        );
        if state.focus || (refocus_box(&resp, enter) && state.cursor.is_some()) {
            resp.request_focus();
            state.focus = false;
        }
        refocus = resp.has_focus() || resp.lost_focus();
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !state.query.is_empty() {
            state.query.clear();
        }
        if !state.query.is_empty() && crate::ui::icons::button(ui, crate::ui::icons::Icon::Close, 18.0, false).clicked() {
            state.query.clear();
        }
        if state.server_running && state.server_is_current() {
            ui.add_space(8.0);
            theme::spinner(ui, 14.0);
            ui.label(RichText::new("Searching server…").small().color(theme::TEXT_DIM));
        }
    });
    ui.add_space(10.0);

    if state.query.trim().is_empty() {
        ui.label(RichText::new("Type to search your library. Ctrl+F jumps here.").color(theme::TEXT_DIM));
        return;
    }

    let results = state.results(lib, ver).clone();
    let hits = results.hits();
    if refocus && !hits.is_empty() {
        let n = hits.len();
        if down {
            state.cursor = Some(state.cursor.map_or(0, |c| (c + 1).min(n - 1)));
        } else if up {
            state.cursor = state.cursor.and_then(|c| c.checked_sub(1));
        }
    }
    let cursor = state.cursor.filter(|&c| c < hits.len());
    if let (true, Some(c)) = (enter, cursor) {
        activate(ctx, hits[c], &results.tracks[..results.tracks.len().min(MAX_SHOWN_TRACKS)], shift);
        if shift {
            state.focus = true;
        }
    }
    let marked = |h: Hit| cursor.is_some_and(|c| hits[c] == h);
    let top_marked = cursor == Some(0) && results.top.is_some();
    let section_marked = |h: Hit| marked(h) && !(top_marked && results.top == Some(h));
    if results.total() == 0 {
        ui.label(RichText::new(format!("No matches for “{}” in your library.", state.query.trim())).color(theme::TEXT_DIM));
    } else {
        egui::ScrollArea::vertical().id_salt("search-results").auto_shrink([false, false]).show(ui, |ui| {
            if let Some(top) = results.top {
                section(ui, "Top result");
                top_card(ui, ctx, top, &results.tracks[..results.tracks.len().min(MAX_SHOWN_TRACKS)], top_marked);
            }
            if !results.artists.is_empty() {
                section(ui, "Artists");
                let mut open = None;
                card_row(
                    ui,
                    ctx.art,
                    results.artists.len(),
                    true,
                    "search-artists",
                    |i| {
                        let a = &lib.artists[results.artists[i]];
                        CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: a.genre.as_deref().unwrap_or("Artist"), seed: &a.title }
                    },
                    |i, r| {
                        if section_marked(Hit::Artist(results.artists[i])) {
                            mark(r);
                        }
                        if r.clicked() {
                            open = Some(lib.artists[results.artists[i]].id);
                        }
                    },
                );
                if let Some(id) = open {
                    ctx.actions.push(UiAction::Navigate(Route::Artist(id)));
                }
            }
            if !results.albums.is_empty() {
                section(ui, "Albums");
                let mut open = None;
                card_row(
                    ui,
                    ctx.art,
                    results.albums.len(),
                    false,
                    "search-albums",
                    |i| {
                        let a = &lib.albums[results.albums[i]];
                        CardItem { thumb: a.thumb.as_deref(), title: &a.title, subtitle: &a.artist, seed: &a.title }
                    },
                    |i, r| {
                        if section_marked(Hit::Album(results.albums[i])) {
                            mark(r);
                        }
                        if r.clicked() {
                            open = Some(lib.albums[results.albums[i]].id);
                        }
                    },
                );
                if let Some(id) = open {
                    ctx.actions.push(UiAction::Navigate(Route::Album(id)));
                }
            }
            if !results.playlists.is_empty() {
                section(ui, "Playlists");
                let mut open = None;
                let subs: Vec<String> = results.playlists.iter().map(|&i| plural(lib.playlists[i].leaf_count as usize, "track", "tracks")).collect();
                card_row(
                    ui,
                    ctx.art,
                    results.playlists.len(),
                    false,
                    "search-playlists",
                    |i| {
                        let p = &lib.playlists[results.playlists[i]];
                        CardItem { thumb: p.composite.as_deref(), title: &p.title, subtitle: &subs[i], seed: &p.title }
                    },
                    |i, r| {
                        if section_marked(Hit::Playlist(results.playlists[i])) {
                            mark(r);
                        }
                        if r.clicked() {
                            open = Some(lib.playlists[results.playlists[i]].id);
                        }
                    },
                );
                if let Some(id) = open {
                    ctx.actions.push(UiAction::Navigate(Route::Playlist(id)));
                }
            }
            if !results.genres.is_empty() {
                section(ui, "Genres");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                    for &gi in &results.genres {
                        let (g, n) = &lib.genres[gi];
                        let (c1, c2) = crate::ui::art::placeholder_colors(g);
                        let (rect, r) = ui.allocate_exact_size(egui::vec2(170.0, 64.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, 10.0, if r.hovered() { c1 } else { c2 });
                        ui.painter().text(rect.min + egui::vec2(12.0, 10.0), egui::Align2::LEFT_TOP, g, egui::FontId::proportional(16.0), theme::TEXT);
                        ui.painter().text(rect.left_bottom() + egui::vec2(12.0, -9.0), egui::Align2::LEFT_BOTTOM, plural(*n, "album", "albums"), egui::FontId::proportional(12.0), theme::TEXT_DIM);
                        if section_marked(Hit::Genre(gi)) {
                            mark(&r);
                        }
                        if r.clicked() {
                            ctx.actions.push(UiAction::Navigate(Route::Genre(g.clone())));
                        }
                    }
                });
            }
            if !results.tracks.is_empty() {
                let shown = results.tracks.len().min(MAX_SHOWN_TRACKS);
                let label = if results.track_total > shown {
                    format!("Tracks (showing {shown} of {})", results.track_total)
                } else {
                    plural(results.track_total, "Track", "Tracks")
                };
                section(ui, &label);
                let top = ui.cursor().top();
                track_list(ui, ctx, &results.tracks, TableOpts { album: true, artist: true, track_numbers: false, art: true, playlist: None }, MAX_SHOWN_TRACKS, "search-tracks");
                // Highlight and reveal the track row the arrow keys are on.
                if let Some(Hit::Track(k)) = cursor.map(|c| hits[c]).filter(|h| section_marked(*h)) {
                    let y = top + k as f32 * (ROW_H + ui.spacing().item_spacing.y);
                    let r = egui::Rect::from_min_size(egui::pos2(ui.min_rect().left(), y), egui::vec2(ui.min_rect().width(), ROW_H));
                    ui.painter().rect_stroke(r, 6.0, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
                    ui.scroll_to_rect(r, None);
                }
            }
            server_supplement(ui, ctx, state, lib);
        });
    }

    // Keep a result area even when the local library has nothing, so server-only matches show.
    if results.total() == 0 {
        server_supplement(ui, ctx, state, lib);
    }
}

/// Enter makes a single-line box give up the focus; take it back after acting on a result so
/// typing can go on.
fn refocus_box(resp: &egui::Response, enter: bool) -> bool {
    enter && resp.lost_focus()
}

/// Show server search hits that are not in the cached library (stale cache, or a library the
/// user has not opened yet). Hits that are already shown above are skipped.
fn server_supplement(ui: &mut egui::Ui, ctx: &mut ViewCtx, state: &SearchState, lib: &LibraryModel) {
    if !state.server_is_current() || state.server_hubs.is_empty() {
        return;
    }
    let missing = missing_server_hits(lib, &state.server_hubs);
    if missing.is_empty() {
        return;
    }
    ui.add_space(10.0);
    section(ui, "Found on the server (not in the cached library)");
    ui.label(RichText::new("Refresh the library cache to add these.").small().color(theme::TEXT_DIM));
    for hit in missing.iter().take(20) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(hit.kind).small().color(theme::TEXT_DIM));
            ui.label(RichText::new(hit.title).color(theme::TEXT));
            if !hit.subtitle.is_empty() {
                ui.label(RichText::new(hit.subtitle).small().color(theme::TEXT_DIM));
            }
        });
    }
    let _ = (ctx, SERVER_DEBOUNCE);
}

/// Server hits that are not in the in-memory library, in hub order.
struct MissingHit<'a> {
    kind: &'static str,
    title: &'a str,
    subtitle: &'a str,
}

fn missing_server_hits<'a>(lib: &LibraryModel, hubs: &'a [Hub]) -> Vec<MissingHit<'a>> {
    let mut missing = Vec::new();
    for hub in hubs {
        for m in &hub.metadata {
            let id = m.rating_key.as_deref().and_then(|s| s.parse::<i64>().ok());
            let known = match (m.kind.as_deref(), id) {
                (Some("artist"), Some(id)) => lib.artist(id).is_some(),
                (Some("album"), Some(id)) => lib.album(id).is_some(),
                (Some("track"), Some(id)) => lib.track(id).is_some(),
                _ => false,
            };
            let Some(title) = m.title.as_deref().filter(|_| !known) else { continue };
            missing.push(MissingHit {
                kind: match m.kind.as_deref() {
                    Some("artist") => "Artist",
                    Some("album") => "Album",
                    _ => "Track",
                },
                title,
                subtitle: m.grandparent_title.as_deref().or(m.parent_title.as_deref()).unwrap_or(""),
            });
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_matches_unaccented_queries() {
        let needle: Vec<char> = "deja".chars().map(fold).collect();
        assert_eq!(match_score("Déjà Vu", &needle), Some(0));
        let needle: Vec<char> = "cafe".chars().map(fold).collect();
        assert_eq!(match_score("Café Électrique", &needle), Some(0));
        let needle: Vec<char> = "uber".chars().map(fold).collect();
        assert_eq!(match_score("Über Alles Gut", &needle), Some(0));
    }

    #[test]
    fn server_hits_are_labelled_and_kept_without_a_cache_entry() {
        let lib = LibraryModel::default();
        let hubs = vec![Hub {
            metadata: vec![
                crate::plex::models::Metadata { rating_key: Some("42".into()), kind: Some("track".into()), title: Some("Midnight Lantern".into()), grandparent_title: Some("Aurora Lines".into()), ..Default::default() },
                crate::plex::models::Metadata { rating_key: Some("7".into()), kind: Some("album".into()), title: Some("Some Album".into()), ..Default::default() },
                crate::plex::models::Metadata { kind: Some("track".into()), title: Some("No id".into()), ..Default::default() },
            ],
            ..Default::default()
        }];
        let hits = missing_server_hits(&lib, &hubs);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].kind, "Track");
        assert_eq!(hits[0].subtitle, "Aurora Lines");
        assert_eq!(hits[1].kind, "Album");
        assert_eq!(hits[1].subtitle, "");
    }

    #[test]
    fn scores_prefix_before_substring() {
        let needle: Vec<char> = "night".chars().map(fold).collect();
        assert_eq!(match_score("Night Signals", &needle), Some(0));
        assert_eq!(match_score("Late Night", &needle), Some(1));
        assert_eq!(match_score("Noon", &needle), None);
        // A repeated first letter must not lose an earlier start position.
        let needle: Vec<char> = "aa".chars().map(fold).collect();
        assert_eq!(match_score("baa", &needle), Some(1));
    }

    fn lib_with(artists: &[&str], albums: &[&str], tracks: &[&str]) -> LibraryModel {
        use crate::cache::db::{Kind, LibraryDb};
        use crate::plex::models::Metadata;
        let mut db = LibraryDb::open_in_memory().unwrap();
        let m = |id: usize, kind: &str, title: &str| Metadata {
            rating_key: Some(id.to_string()),
            title: Some(title.into()),
            kind: Some(kind.into()),
            parent_rating_key: Some("100".into()),
            grandparent_rating_key: Some("1".into()),
            ..Default::default()
        };
        let a: Vec<Metadata> = artists.iter().enumerate().map(|(i, t)| m(1 + i, "artist", t)).collect();
        let al: Vec<Metadata> = albums.iter().enumerate().map(|(i, t)| m(100 + i, "album", t)).collect();
        let tr: Vec<Metadata> = tracks.iter().enumerate().map(|(i, t)| m(1000 + i, "track", t)).collect();
        db.upsert(Kind::Artist, &a).unwrap();
        db.upsert(Kind::Album, &al).unwrap();
        db.upsert(Kind::Track, &tr).unwrap();
        LibraryModel::load(&db).unwrap()
    }

    #[test]
    fn exact_name_wins_the_top_result() {
        let lib = lib_with(&["Mila Varga", "Rain Dogs"], &["Neon Rain"], &["Rain"]);
        // "rain": the song is an exact match, the album/artist only contain or start with it.
        let r = search_local(&lib, "rain");
        assert_eq!(r.top, Some(Hit::Track(0)));
        // "rain dogs": the artist matches exactly.
        let r = search_local(&lib, "rain dogs");
        assert!(matches!(r.top, Some(Hit::Artist(i)) if &*lib.artists[i].title == "Rain Dogs"));
        // Equal rank: artists go first.
        let lib = lib_with(&["Velvet Underground"], &["Velvet Hours"], &[]);
        assert!(matches!(search_local(&lib, "velvet").top, Some(Hit::Artist(_))));
    }

    #[test]
    fn hits_follow_page_order() {
        let lib = lib_with(&["Aurora"], &["Aurora Live"], &["Aurora Song"]);
        let r = search_local(&lib, "aurora");
        let hits = r.hits();
        assert_eq!(hits[0], r.top.unwrap(), "top result comes first");
        assert!(matches!(hits[1], Hit::Artist(_)));
        assert!(matches!(hits[2], Hit::Album(_)));
        assert_eq!(*hits.last().unwrap(), Hit::Track(0));
    }
}
