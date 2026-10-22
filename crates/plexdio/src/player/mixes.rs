//! Home mixes built from the cached library and the local play history: Most played, Recently
//! played and Rediscover. All three are plain track-id lists, so the UI can start them without
//! any server round trip (offline included).

use crate::cache::db::LibraryDb;
use crate::library_model::{Id, LibraryModel};
use std::collections::{HashMap, HashSet};

/// Longest mix shown on Home.
pub const MIX_LEN: usize = 30;
/// Tracks played within this window never show up in Rediscover.
const REDISCOVER_AFTER_SECS: i64 = 30 * 24 * 3600;

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Mixes {
    pub most_played: Vec<Id>,
    pub recently_played: Vec<Id>,
    pub rediscover: Vec<Id>,
}

impl Mixes {
    pub fn is_empty(&self) -> bool {
        self.most_played.is_empty() && self.recently_played.is_empty() && self.rediscover.is_empty()
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
        Self {
            most_played: most_played(lib, &counts, MIX_LEN),
            recently_played: recently_played(lib, &recent, MIX_LEN),
            rediscover: rediscover(lib, now, MIX_LEN),
        }
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
        assert_eq!(m.most_played, vec![1]);
        assert!(!m.is_empty());
    }
}
