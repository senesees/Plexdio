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
}

impl LocalResults {
    pub fn total(&self) -> usize {
        self.artist_total + self.album_total + self.track_total
    }
}

/// Lowercase and fold common Latin accents so "deja" finds "Déjà".
fn fold(c: char) -> char {
    match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'ś' | 'š' => 's',
        'ź' | 'ż' | 'ž' => 'z',
        'ł' => 'l',
        _ => {
            let lower = c.to_lowercase().next().unwrap_or(c);
            if lower == c {
                c
            } else {
                fold(lower)
            }
        }
    }
}

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
    out
}

impl SearchState {
    fn results(&mut self, lib: &LibraryModel, ver: u64) -> &LocalResults {
        let q = self.query.trim().to_lowercase();
        let fresh = self.memo.as_ref().is_none_or(|(mq, mv, _)| mq != &q || *mv != ver);
        if fresh {
            self.memo = Some((q.clone(), ver, search_local(lib, &q)));
        }
        &self.memo.as_ref().expect("just filled").2
    }

    /// True when the server results belong to the current query.
    fn server_is_current(&self) -> bool {
        self.server_for.as_deref() == Some(self.query.trim().to_lowercase().as_str())
    }
}

pub fn search_view(ui: &mut egui::Ui, ctx: &mut ViewCtx, state: &mut SearchState, ver: u64) {
    let lib = ctx.lib;
    ui.horizontal(|ui| {
        ui.heading("Search");
        ui.add_space(12.0);
        let resp = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Artists, albums and tracks")
                .desired_width(420.0)
                .margin(egui::Margin::symmetric(12, 8)),
        );
        if state.focus {
            resp.request_focus();
            state.focus = false;
        }
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
    if results.total() == 0 {
        ui.label(RichText::new(format!("No matches for “{}” in your library.", state.query.trim())).color(theme::TEXT_DIM));
    } else {
        egui::ScrollArea::vertical().id_salt("search-results").auto_shrink([false, false]).show(ui, |ui| {
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
                        if r.clicked() {
                            open = Some(lib.albums[results.albums[i]].id);
                        }
                    },
                );
                if let Some(id) = open {
                    ctx.actions.push(UiAction::Navigate(Route::Album(id)));
                }
            }
            if !results.tracks.is_empty() {
                let shown = results.tracks.len().min(MAX_SHOWN_TRACKS);
                let label = if results.track_total > shown {
                    format!("Tracks (showing {shown} of {})", results.track_total)
                } else {
                    plural(results.track_total, "Track", "Tracks")
                };
                section(ui, &label);
                track_list(ui, ctx, &results.tracks, TableOpts { album: true, artist: true, track_numbers: false, art: true }, MAX_SHOWN_TRACKS);
            }
            server_supplement(ui, ctx, state, lib);
        });
    }

    // Keep a result area even when the local library has nothing, so server-only matches show.
    if results.total() == 0 {
        server_supplement(ui, ctx, state, lib);
    }
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
}
