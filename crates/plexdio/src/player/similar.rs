//! Similar artists for the artist page ("Fans also like"). Plex lists them in the artist's
//! `/related` hubs (from its metadata agent or sonic analysis); when the server has none, or
//! there is no server, artists sharing genres with this one are used instead.

use crate::library_model::{Id, LibraryModel};
use crate::plex::server::PmsClient;
use std::collections::{HashMap, HashSet};

/// How many similar artists the page shows.
pub const LIMIT: usize = 10;

/// Artist ids from the server's related hubs, best first. Blocking: call from a worker thread.
/// Returns an empty list when the server has nothing (the caller then uses the local fallback).
pub fn server_ids(pms: &PmsClient, artist: Id) -> Vec<Id> {
    match pms.related(&artist.to_string()) {
        Ok(hubs) => {
            let mut seen = HashSet::new();
            hubs.iter()
                .filter(|h| h.kind.as_deref() == Some("artist"))
                .flat_map(|h| h.metadata.iter())
                .filter_map(|m| m.rating_key.as_deref().and_then(|k| k.parse::<Id>().ok()))
                .filter(|id| *id != artist && seen.insert(*id))
                .collect()
        }
        Err(e) => {
            log::debug!("related artists unavailable for {artist}: {e}");
            Vec::new()
        }
    }
}

/// Similar artists for `artist`: server suggestions that exist in this library first, then
/// local genre matches, up to `limit`.
pub fn similar_artists(lib: &LibraryModel, artist: Id, from_server: &[Id], limit: usize) -> Vec<Id> {
    let mut seen: HashSet<Id> = HashSet::from([artist]);
    let mut out: Vec<Id> = from_server
        .iter()
        .copied()
        .filter(|id| lib.artist(*id).is_some() && seen.insert(*id))
        .take(limit)
        .collect();
    if out.len() < limit {
        out.extend(local_similar(lib, artist, &seen, limit - out.len()));
    }
    out
}

/// Genres of an artist: its own tag plus those of its albums.
fn genres_of(lib: &LibraryModel, artist: Id) -> HashSet<&str> {
    let mut g: HashSet<&str> = HashSet::new();
    if let Some(a) = lib.artist(artist).and_then(|a| a.genre.as_deref()) {
        g.insert(a);
    }
    for &i in lib.artist_albums.get(&artist).map(Vec::as_slice).unwrap_or(&[]) {
        if let Some(genre) = lib.albums[i].genre.as_deref() {
            g.insert(genre);
        }
    }
    g
}

/// Artists sharing the most genres with `artist`; ties go to the more played artist. Artists
/// with nothing in common are never suggested.
fn local_similar(lib: &LibraryModel, artist: Id, exclude: &HashSet<Id>, limit: usize) -> Vec<Id> {
    let mine = genres_of(lib, artist);
    if mine.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut plays: HashMap<Id, i64> = HashMap::new();
    for t in &lib.tracks {
        *plays.entry(t.artist_id).or_default() += t.view_count as i64;
    }
    let mut scored: Vec<(usize, i64, &str, Id)> = lib
        .artists
        .iter()
        .filter(|a| !exclude.contains(&a.id))
        .filter_map(|a| {
            let shared = genres_of(lib, a.id).intersection(&mine).count();
            (shared > 0).then(|| (shared, plays.get(&a.id).copied().unwrap_or(0), &*a.sort_key, a.id))
        })
        .collect();
    // Most shared genres, then most played, then by name so the order is stable.
    scored.sort_by(|x, y| y.0.cmp(&x.0).then(y.1.cmp(&x.1)).then(x.2.cmp(y.2)));
    scored.into_iter().take(limit).map(|s| s.3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::db::{Kind, LibraryDb};
    use crate::plex::models::{Metadata, Tag};

    fn item(id: i64, kind: &str, parent: Option<i64>, genre: &str) -> Metadata {
        Metadata {
            rating_key: Some(id.to_string()),
            title: Some(format!("{kind} {id}")),
            kind: Some(kind.into()),
            parent_rating_key: parent.map(|p| p.to_string()),
            genres: vec![Tag { tag: genre.into() }],
            ..Default::default()
        }
    }

    fn track(id: i64, album: i64, artist: i64, plays: i64) -> Metadata {
        Metadata {
            parent_rating_key: Some(album.to_string()),
            grandparent_rating_key: Some(artist.to_string()),
            parent_title: Some(format!("album {album}")),
            grandparent_title: Some(format!("artist {artist}")),
            index: Some(id),
            duration: Some(60_000),
            view_count: Some(plays),
            added_at: Some(0),
            ..item(id, "track", Some(album), "x")
        }
    }

    /// Artists 1, 2, 3 play Rock (2 more than 3); 4 plays Jazz.
    fn lib() -> LibraryModel {
        let mut db = LibraryDb::open_in_memory().unwrap();
        db.upsert(Kind::Artist, &[item(1, "artist", None, "Rock"), item(2, "artist", None, "Rock"), item(3, "artist", None, "Rock"), item(4, "artist", None, "Jazz")]).unwrap();
        db.upsert(Kind::Album, &[item(10, "album", Some(1), "Rock"), item(11, "album", Some(2), "Rock"), item(12, "album", Some(3), "Rock"), item(13, "album", Some(4), "Jazz")]).unwrap();
        db.upsert(Kind::Track, &[track(100, 10, 1, 0), track(101, 11, 2, 9), track(102, 12, 3, 1), track(103, 13, 4, 50)]).unwrap();
        LibraryModel::load(&db).unwrap()
    }

    #[test]
    fn local_fallback_matches_genre_and_prefers_played_artists() {
        let lib = lib();
        assert_eq!(similar_artists(&lib, 1, &[], 10), vec![2, 3], "Rock artists only, most played first");
        assert_eq!(similar_artists(&lib, 1, &[], 1), vec![2]);
    }

    #[test]
    fn server_suggestions_come_first_and_unknown_ids_are_dropped() {
        let lib = lib();
        // 4 is not a genre match but the server says so; 999 is not in the library; 1 is the seed.
        assert_eq!(similar_artists(&lib, 1, &[4, 999, 1, 4], 10), vec![4, 2, 3]);
        assert_eq!(similar_artists(&lib, 1, &[4, 3], 2), vec![4, 3], "server answer fills the limit");
    }
}
