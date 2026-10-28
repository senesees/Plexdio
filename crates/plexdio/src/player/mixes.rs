//! Home mixes ("Made for you") built from the cached library and the local play history: Most
//! played, Recently played, Rediscover, favourites, new additions, deep cuts and genre/decade
//! mixes. All of them are plain track-id lists, so the UI can show and start them without any
//! server round trip (offline included).

use crate::cache::db::LibraryDb;
use crate::library_model::{Id, LibraryModel, Track};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Longest mix shown on Home.
pub const MIX_LEN: usize = 30;
/// Longest genre / decade mix.
const STYLE_MIX_LEN: usize = 50;
/// Tracks played within this window never show up in Rediscover.
const REDISCOVER_AFTER_SECS: i64 = 30 * 24 * 3600;
/// Favourites not played for this long are "forgotten".
const FORGOTTEN_AFTER_SECS: i64 = 60 * 24 * 3600;
/// Tracks added within this window count as new.
const NEW_WITHIN_SECS: i64 = 60 * 24 * 3600;
/// At most this many tracks of one artist in a genre/decade mix.
const PER_ARTIST: usize = 4;
/// How many genre and decade mixes Home shows.
const GENRE_MIXES: usize = 4;
const DECADE_MIXES: usize = 2;

/// One mix, shown on Home as a card and opened as its own playlist-like page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mix {
    /// Stable key used by the route (`most-played`, `genre:Rock`, `decade:1990`, …).
    pub key: String,
    pub title: String,
    /// One line shown under the title on the mix page.
    pub description: String,
    pub ids: Vec<Id>,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Mixes {
    /// Non-empty mixes in display order.
    pub list: Vec<Mix>,
}

impl Mixes {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Mix> {
        self.list.iter().find(|m| m.key == key)
    }

    /// Build all mixes. `db` contributes the local play history; the server's view counts alone
    /// still produce a sensible "Most played" when the history is empty.
    pub fn load(lib: &LibraryModel, db: Option<&LibraryDb>, now: i64) -> Self {
        let recent = db.and_then(|d| d.recent_play_ids(500).ok()).unwrap_or_default();
        let counts: HashMap<Id, u64> = db
            .and_then(|d| d.play_counts().ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut list = Vec::new();
        let mut push = |key: String, title: String, description: String, ids: Vec<Id>| {
            if !ids.is_empty() {
                list.push(Mix { key, title, description, ids });
            }
        };
        let s = |x: &str| x.to_string();
        push(s("most-played"), s("Most played"), s("The songs you keep coming back to"), most_played(lib, &counts, MIX_LEN));
        push(s("recently-played"), s("Recently played"), s("What you listened to lately"), recently_played(lib, &recent, MIX_LEN));
        push(s("favourites"), s("Your favourites"), s("Your best-rated songs"), favourites(lib, MIX_LEN));
        push(s("rediscover"), s("Rediscover"), s("Songs you have not heard in a while"), rediscover(lib, now, MIX_LEN));
        push(s("forgotten"), s("Forgotten favourites"), s("Songs you loved but have not played for months"), forgotten_favourites(lib, now, MIX_LEN));
        push(s("new"), s("New additions"), s("The newest songs in your library"), new_additions(lib, now, MIX_LEN));
        push(s("deep-cuts"), s("Deep cuts"), s("Unplayed songs by the artists you listen to most"), deep_cuts(lib, &counts, now, MIX_LEN));
        for (genre, ids) in genre_mixes(lib, &counts, now, GENRE_MIXES, STYLE_MIX_LEN) {
            push(format!("genre:{genre}"), format!("{genre} Mix"), format!("A mix of {genre} from your library"), ids);
        }
        for (decade, ids) in decade_mixes(lib, &counts, now, DECADE_MIXES, STYLE_MIX_LEN) {
            push(format!("decade:{decade}"), format!("{decade}s Mix"), format!("Songs from the {decade}s"), ids);
        }
        Self { list }
    }
}

/// Tracks ordered by how often they were played: the server count plus twice the local one
/// (a local play means the user actually listened here). Ties go to the more recently played
/// and then the better rating.
pub fn most_played(lib: &LibraryModel, local: &HashMap<Id, u64>, n: usize) -> Vec<Id> {
    let mut scored: Vec<(u64, i64, f32, Id)> = lib
        .tracks
        .iter()
        .map(|t| {
            let local = local.get(&t.id).copied().unwrap_or(0);
            (t.view_count as u64 + local * 2, t.last_viewed_at, t.user_rating, t.id)
        })
        .filter(|s| s.0 > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.total_cmp(&a.2)).then(a.3.cmp(&b.3)));
    scored.into_iter().take(n).map(|s| s.3).collect()
}

/// Most recently played first. `recent` is newest-first and may repeat ids; the library's
/// `lastViewedAt` is the fallback when no local history exists.
pub fn recently_played(lib: &LibraryModel, recent: &[Id], n: usize) -> Vec<Id> {
    let mut seen = HashSet::new();
    let mut out: Vec<Id> = recent
        .iter()
        .copied()
        .filter(|id| lib.track(*id).is_some())
        .filter(|id| seen.insert(*id))
        .take(n)
        .collect();
    if out.is_empty() {
        let mut played: Vec<&crate::library_model::Track> = lib.tracks.iter().filter(|t| t.last_viewed_at > 0).collect();
        played.sort_by_key(|t| std::cmp::Reverse(t.last_viewed_at));
        out = played.into_iter().take(n).map(|t| t.id).collect();
    }
    out
}

/// Tracks the user has not played for a while (or never), weighted towards well-rated ones and
/// returned in a stable pseudo-random order so the section does not look identical every day.
pub fn rediscover(lib: &LibraryModel, now: i64, n: usize) -> Vec<Id> {
    let mut candidates: Vec<&crate::library_model::Track> = lib
        .tracks
        .iter()
        .filter(|t| t.last_viewed_at == 0 || now.saturating_sub(t.last_viewed_at) >= REDISCOVER_AFTER_SECS)
        .collect();
    if candidates.is_empty() {
        // Everything was played recently: fall back to the least recently played tracks.
        let mut all: Vec<&crate::library_model::Track> = lib.tracks.iter().collect();
        all.sort_by_key(|t| t.last_viewed_at);
        candidates = all.into_iter().take(n).collect();
    }
    let mut keyed: Vec<(f64, Id)> = candidates
        .into_iter()
        .map(|t| {
            let weight = 1.0 + f64::from(t.user_rating) * 0.35 + (t.added_at % 1000) as f64 / 10_000.0;
            (mix_hash(t.id) / weight, t.id)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().take(n).map(|k| k.1).collect()
}

/// Deterministic 0..1 hash of an id (stable across runs, unlike the std hasher).
fn mix_hash(id: i64) -> f64 {
    let mut x = id as u64 ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Rated tracks, best first (at least 3 stars); ties go to the more played.
pub fn favourites(lib: &LibraryModel, n: usize) -> Vec<Id> {
    let mut rated: Vec<&Track> = lib.tracks.iter().filter(|t| t.user_rating >= 6.0).collect();
    rated.sort_by(|a, b| b.user_rating.total_cmp(&a.user_rating).then(b.view_count.cmp(&a.view_count)).then(a.id.cmp(&b.id)));
    rated.into_iter().take(n).map(|t| t.id).collect()
}

/// Well-rated or often-played tracks that have not been played for a couple of months.
pub fn forgotten_favourites(lib: &LibraryModel, now: i64, n: usize) -> Vec<Id> {
    let mut old: Vec<&Track> = lib
        .tracks
        .iter()
        .filter(|t| t.last_viewed_at > 0 && now.saturating_sub(t.last_viewed_at) >= FORGOTTEN_AFTER_SECS)
        .filter(|t| t.user_rating >= 6.0 || t.view_count >= 3)
        .collect();
    old.sort_by(|a, b| {
        let sa = a.view_count as f32 + a.user_rating;
        let sb = b.view_count as f32 + b.user_rating;
        sb.total_cmp(&sa).then(a.id.cmp(&b.id))
    });
    old.into_iter().take(n).map(|t| t.id).collect()
}

/// Tracks added lately, newest first; unplayed ones before played ones. Falls back to the
/// newest tracks when nothing was added within the window.
pub fn new_additions(lib: &LibraryModel, now: i64, n: usize) -> Vec<Id> {
    let mut fresh: Vec<&Track> = lib.tracks.iter().filter(|t| t.added_at > 0 && now.saturating_sub(t.added_at) <= NEW_WITHIN_SECS).collect();
    if fresh.is_empty() {
        fresh = lib.tracks.iter().filter(|t| t.added_at > 0).collect();
    }
    fresh.sort_by(|a, b| (a.view_count > 0).cmp(&(b.view_count > 0)).then(b.added_at.cmp(&a.added_at)).then(a.album_id.cmp(&b.album_id)).then(a.index.cmp(&b.index)));
    fresh.into_iter().take(n).map(|t| t.id).collect()
}

/// Play score of every artist (server plays plus twice the local ones).
fn artist_plays(lib: &LibraryModel, local: &HashMap<Id, u64>) -> HashMap<Id, u64> {
    let mut plays: HashMap<Id, u64> = HashMap::new();
    for t in &lib.tracks {
        *plays.entry(t.artist_id).or_default() += t.view_count as u64 + local.get(&t.id).copied().unwrap_or(0) * 2;
    }
    plays
}

/// Never-played tracks by the ten most played artists, in a daily pseudo-random order with at
/// most a few tracks per artist.
pub fn deep_cuts(lib: &LibraryModel, local: &HashMap<Id, u64>, now: i64, n: usize) -> Vec<Id> {
    let plays = artist_plays(lib, local);
    let mut top: Vec<(Id, u64)> = plays.into_iter().filter(|p| p.1 > 0).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top: HashSet<Id> = top.into_iter().take(10).map(|p| p.0).collect();
    let cands: Vec<&Track> =
        lib.tracks.iter().filter(|t| top.contains(&t.artist_id) && t.view_count == 0 && local.get(&t.id).copied().unwrap_or(0) == 0).collect();
    daily_pick(cands, now, n, local)
}

/// Genre of a track, falling back to its album's.
fn track_genre<'a>(lib: &'a LibraryModel, t: &'a Track) -> Option<&'a Arc<str>> {
    t.genre.as_ref().or_else(|| lib.album(t.album_id).and_then(|a| a.genre.as_ref()))
}

/// Year of a track, falling back to its album's.
fn track_year(lib: &LibraryModel, t: &Track) -> i32 {
    if t.year > 0 {
        t.year
    } else {
        lib.album(t.album_id).map_or(0, |a| a.year)
    }
}

/// Groups ranked by plays (then size), each turned into a daily pick. Groups with fewer than
/// `min` tracks are skipped.
fn group_mixes<K: Clone + Eq + std::hash::Hash + Ord>(
    lib: &LibraryModel,
    local: &HashMap<Id, u64>,
    now: i64,
    groups: usize,
    n: usize,
    key: impl Fn(&Track) -> Option<K>,
) -> Vec<(K, Vec<Id>)> {
    const MIN: usize = 8;
    let mut by: HashMap<K, Vec<&Track>> = HashMap::new();
    for t in &lib.tracks {
        if let Some(k) = key(t) {
            by.entry(k).or_default().push(t);
        }
    }
    let mut ranked: Vec<(u64, usize, K)> = by
        .iter()
        .filter(|(_, ts)| ts.len() >= MIN)
        .map(|(k, ts)| (ts.iter().map(|t| t.view_count as u64 + local.get(&t.id).copied().unwrap_or(0) * 2).sum(), ts.len(), k.clone()))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    ranked
        .into_iter()
        .take(groups)
        .map(|(_, _, k)| {
            let ts = by.remove(&k).unwrap_or_default();
            (k, daily_pick(ts, now, n, local))
        })
        .collect()
}

/// The most played genres as mixes.
pub fn genre_mixes(lib: &LibraryModel, local: &HashMap<Id, u64>, now: i64, groups: usize, n: usize) -> Vec<(Arc<str>, Vec<Id>)> {
    group_mixes(lib, local, now, groups, n, |t| track_genre(lib, t).cloned())
}

/// The most played decades as mixes (keyed by the decade's first year, e.g. 1990).
pub fn decade_mixes(lib: &LibraryModel, local: &HashMap<Id, u64>, now: i64, groups: usize, n: usize) -> Vec<(i32, Vec<Id>)> {
    group_mixes(lib, local, now, groups, n, |t| {
        let y = track_year(lib, t);
        (y >= 1900).then_some(y / 10 * 10)
    })
}

/// Pick `n` tracks in a pseudo-random order that changes once a day, weighted towards rated and
/// played tracks, with at most [`PER_ARTIST`] tracks per artist (relaxed when that leaves too
/// few).
fn daily_pick(cands: Vec<&Track>, now: i64, n: usize, local: &HashMap<Id, u64>) -> Vec<Id> {
    let day = now.div_euclid(86_400);
    let mut keyed: Vec<(f64, &Track)> = cands
        .into_iter()
        .map(|t| {
            let plays = t.view_count as f64 + local.get(&t.id).copied().unwrap_or(0) as f64 * 2.0;
            let weight = 1.0 + f64::from(t.user_rating) * 0.3 + (1.0 + plays).ln() * 0.5;
            (mix_hash(t.id ^ day.wrapping_mul(0x5851_f42d)) / weight, t)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    let mut per: HashMap<Id, usize> = HashMap::new();
    let mut out = Vec::new();
    let mut rest = Vec::new();
    for (_, t) in keyed {
        let c = per.entry(t.artist_id).or_default();
        if *c < PER_ARTIST {
            *c += 1;
            out.push(t.id);
        } else {
            rest.push(t.id);
        }
        if out.len() == n {
            return out;
        }
    }
    out.extend(rest.into_iter().take(n - out.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plex::models::Metadata;

    fn track(id: i64, artist: i64, album: i64, plays: i64, last: i64, rating: f64, added: i64) -> Metadata {
        Metadata {
            rating_key: Some(id.to_string()),
            title: Some(format!("Track {id}")),
            kind: Some("track".into()),
            parent_rating_key: Some(album.to_string()),
            grandparent_rating_key: Some(artist.to_string()),
            parent_title: Some(format!("Album {album}")),
            grandparent_title: Some(format!("Artist {artist}")),
            index: Some(id),
            duration: Some(60_000),
            added_at: Some(added),
            view_count: Some(plays),
            last_viewed_at: Some(last),
            user_rating: Some(rating),
            ..Default::default()
        }
    }

    fn library(tracks: Vec<Metadata>) -> LibraryModel {
        let mut db = LibraryDb::open_in_memory().unwrap();
        let artists: Vec<Metadata> = (1..=4)
            .map(|i| Metadata {
                rating_key: Some(i.to_string()),
                title: Some(format!("Artist {i}")),
                kind: Some("artist".into()),
                ..Default::default()
            })
            .collect();
        db.upsert(crate::cache::db::Kind::Artist, &artists).unwrap();
        let albums: Vec<Metadata> = (1..=4)
            .map(|i| Metadata {
                rating_key: Some(i.to_string()),
                title: Some(format!("Album {i}")),
                kind: Some("album".into()),
                parent_rating_key: Some(i.to_string()),
                ..Default::default()
            })
            .collect();
        db.upsert(crate::cache::db::Kind::Album, &albums).unwrap();
        db.upsert(crate::cache::db::Kind::Track, &tracks).unwrap();
        LibraryModel::load(&db).unwrap()
    }

    #[test]
    fn most_played_weighs_local_plays_and_ratings() {
        let lib = library(vec![
            track(1, 1, 1, 5, 100, 0.0, 0),
            track(2, 1, 1, 5, 200, 8.0, 0),
            track(3, 2, 2, 0, 0, 0.0, 0),
            track(4, 2, 2, 3, 300, 0.0, 0),
        ]);
        let local = HashMap::from([(3, 6)]);
        assert_eq!(most_played(&lib, &local, 4), vec![3, 2, 1, 4]);
        // Unplayed tracks are left out.
        assert_eq!(most_played(&lib, &HashMap::new(), 1), vec![2]);
    }

    #[test]
    fn recently_played_keeps_history_order_without_duplicates() {
        let lib = library(vec![track(1, 1, 1, 0, 0, 0.0, 0), track(2, 1, 1, 0, 0, 0.0, 0), track(3, 1, 1, 0, 0, 0.0, 0)]);
        // Newest first, repeated, with an id the library does not know.
        assert_eq!(recently_played(&lib, &[3, 1, 3, 999, 2], 10), vec![3, 1, 2]);
        // No history: fall back to lastViewedAt.
        let lib2 = library(vec![track(1, 1, 1, 1, 50, 0.0, 0), track(2, 1, 1, 1, 90, 0.0, 0)]);
        assert_eq!(recently_played(&lib2, &[], 10), vec![2, 1]);
    }

    #[test]
    fn rediscover_skips_recently_played_tracks() {
        let now = 1_000_000_000;
        let lib = library(vec![
            track(1, 1, 1, 1, now - 100, 0.0, now), // played today
            track(2, 1, 1, 1, now - 40 * 86_400, 10.0, now),
            track(3, 2, 2, 0, 0, 2.0, now),
            track(4, 2, 2, 1, now - 60 * 86_400, 4.0, now),
        ]);
        let ids = rediscover(&lib, now, 10);
        assert!(!ids.contains(&1), "recently played track is not rediscovered: {ids:?}");
        assert_eq!(ids.len(), 3);
        // Stable for the same inputs.
        assert_eq!(ids, rediscover(&lib, now, 10));
    }

    #[test]
    fn mixes_load_without_a_database() {
        let lib = library(vec![track(1, 1, 1, 2, 5, 0.0, 0)]);
        let m = Mixes::load(&lib, None, 100);
        assert_eq!(m.get("most-played").map(|m| m.ids.clone()), Some(vec![1]));
        assert!(!m.is_empty());
        // Empty mixes are left out.
        assert!(m.get("favourites").is_none());
    }
}
