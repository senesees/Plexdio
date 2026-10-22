//! Radio stations: a queue of tracks that sound like a seed track. Plex's sonic `nearest`
//! endpoint (a Plex Pass feature) is the best source; when the server cannot answer — no Plex
//! Pass, old library, offline — the station falls back to the same artist and genre.

use crate::library_model::{Id, LibraryModel};
use crate::plex::server::PmsClient;
use std::collections::HashSet;

/// How many tracks a freshly started station holds. Autoplay extends it as it runs.
pub const STATION_LEN: usize = 40;
/// Sonic distance ceiling for `nearest` (Plex uses 0..1, smaller is more similar).
const MAX_DISTANCE: f64 = 0.6;

/// Ask the server for sonically similar tracks. Blocking: call from a worker thread.
/// Returns an empty list when sonic analysis is unavailable, which makes the caller use the
/// local fallback.
pub fn sonic_ids(pms: &PmsClient, seed: Id, limit: usize) -> Vec<Id> {
    match pms.nearest_tracks(&seed.to_string(), limit, MAX_DISTANCE) {
        Ok(items) => items
            .iter()
            .filter_map(|m| m.rating_key.as_deref().and_then(|k| k.parse::<Id>().ok()))
            .collect(),
        Err(e) => {
            log::debug!("sonic radio unavailable for {seed}: {e}");
            Vec::new()
        }
    }
}

/// A station seeded by `seed`: server suggestions first (when any are usable), then the local
/// fallback. `exclude` holds ids already queued or already part of the seed (album, artist).
pub fn station(
    lib: &LibraryModel,
    seed: Id,
    from_server: &[Id],
    exclude: &HashSet<Id>,
    limit: usize,
    salt: u64,
) -> Vec<Id> {
    let mut seen = exclude.clone();
    seen.insert(seed);
    let mut out = Vec::with_capacity(limit);
    for &id in from_server {
        if out.len() >= limit {
            return out;
        }
        if lib.track(id).is_some() && seen.insert(id) {
            out.push(id);
        }
    }
    for id in local_station(lib, seed, &seen, limit - out.len(), salt) {
        out.push(id);
    }
    out
}

/// Local fallback: same artist, then the same genre, then the rest of the library. Each group is
/// shuffled with a preference for rated and often played tracks, so consecutive stations differ
/// while good tracks surface more often.
pub fn local_station(lib: &LibraryModel, seed: Id, exclude: &HashSet<Id>, limit: usize, salt: u64) -> Vec<Id> {
    if limit == 0 {
        return Vec::new();
    }
    let Some(seed_track) = lib.track(seed) else { return Vec::new() };
    let mut groups: Vec<Vec<Id>> = Vec::new();
    groups.push(lib.artist_track_ids(seed_track.artist_id));
    if let Some(genre) = &seed_track.genre {
        let same: Vec<Id> = lib
            .tracks
            .iter()
            .filter(|t| t.genre.as_deref() == Some(&**genre))
            .map(|t| t.id)
            .collect();
        groups.push(same);
    }
    groups.push(lib.tracks.iter().map(|t| t.id).collect());

    let mut seen: HashSet<Id> = exclude.clone();
    seen.insert(seed);
    let mut out = Vec::with_capacity(limit);
    for group in groups {
        let mut ids: Vec<Id> = group.into_iter().filter(|id| !seen.contains(id)).collect();
        sort_weighted(lib, &mut ids, salt);
        for id in ids {
            if out.len() >= limit {
                return out;
            }
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    out
}

/// Shuffle by a stable hash divided by the track's weight: higher rated and more played tracks
/// tend to come first, but anything can lead.
fn sort_weighted(lib: &LibraryModel, ids: &mut [Id], salt: u64) {
    ids.sort_by(|a, b| {
        let key = |id: Id| {
            let w = lib.track(id).map_or(1.0, |t| 1.0 + f64::from(t.user_rating) * 0.2 + f64::from(t.view_count).ln_1p() * 0.5);
            shuffle_hash(id as u64 ^ salt) / w
        };
        key(*a).total_cmp(&key(*b)).then(a.cmp(b))
    });
}

fn shuffle_hash(mut x: u64) -> f64 {
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
    use crate::cache::db::LibraryDb;
    use crate::plex::models::Metadata;

    fn artist(id: i64) -> Metadata {
        Metadata {
            rating_key: Some(id.to_string()),
            title: Some(format!("Artist {id}")),
            kind: Some("artist".into()),
            ..Default::default()
        }
    }

    fn album(id: i64, artist: i64, genre: &str) -> Metadata {
        Metadata {
            rating_key: Some(id.to_string()),
            title: Some(format!("Album {id}")),
            kind: Some("album".into()),
            parent_rating_key: Some(artist.to_string()),
            ..Default::default()
        }
        .with_genre(genre)
    }

    trait WithGenre {
        fn with_genre(self, genre: &str) -> Self;
    }
    impl WithGenre for Metadata {
        fn with_genre(mut self, genre: &str) -> Self {
            self.genres = vec![crate::plex::models::Tag { tag: genre.into() }];
            self
        }
    }

    fn track(id: i64, album: i64, artist: i64, genre: &str, plays: i64, rating: f64) -> Metadata {
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
            view_count: Some(plays),
            user_rating: Some(rating),
            added_at: Some(0),
            ..Default::default()
        }
        .with_genre(genre)
    }

    fn library(tracks: &[Metadata]) -> LibraryModel {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert(crate::cache::db::Kind::Artist, &[artist(1), artist(2), artist(3)]).unwrap();
        db.upsert(
            crate::cache::db::Kind::Album,
            &[album(10, 1, "Rock"), album(11, 2, "Rock"), album(12, 3, "Jazz")],
        )
        .unwrap();
        db.upsert(crate::cache::db::Kind::Track, tracks).unwrap();
        LibraryModel::load(&db).unwrap()
    }

    fn lib() -> LibraryModel {
        library(&[
            track(1, 10, 1, "Rock", 5, 10.0),
            track(2, 10, 1, "Rock", 0, 0.0),
            track(3, 11, 2, "Rock", 2, 4.0),
            track(4, 12, 3, "Jazz", 1, 0.0),
        ])
    }

    #[test]
    fn local_station_prefers_the_artist_then_the_genre() {
        let lib = lib();
        let ids = local_station(&lib, 1, &HashSet::new(), 10, 7);
        assert_eq!(ids, vec![2, 3, 4], "same artist, then same genre, then the rest");
        assert_eq!(local_station(&lib, 1, &HashSet::new(), 1, 7), vec![2]);
    }

    #[test]
    fn station_filters_unknown_and_excluded_ids_and_fills_up() {
        let lib = lib();
        let exclude: HashSet<Id> = [2].into_iter().collect();
        // Only one server suggestion is usable; the rest fall back locally.
        let ids = station(&lib, 1, &[999, 3, 3], &exclude, 3, 7);
        assert_eq!(ids, vec![3, 4], "no seed, no duplicates, no unknown ids");
        // Server suggestions that cover the limit win.
        assert_eq!(station(&lib, 1, &[4, 3], &HashSet::new(), 2, 7), vec![4, 3]);
    }

    #[test]
    fn station_is_stable_for_the_same_salt() {
        let lib = lib();
        assert_eq!(
            local_station(&lib, 1, &HashSet::new(), 10, 42),
            local_station(&lib, 1, &HashSet::new(), 10, 42)
        );
    }
}
