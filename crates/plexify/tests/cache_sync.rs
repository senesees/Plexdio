//! Cache + sync tests against the mock server.

use mockplex::library::Track;
use mockplex::{MockConfig, MockServer, MUSIC_SECTION, USER_TOKEN};
use plexify::cache::db::{Kind, LibraryDb};
use plexify::cache::sync::sync_library;
use plexify::library_model::LibraryModel;
use plexify::plex::auth::PlexTv;
use plexify::plex::resources::pick_best;
use plexify::plex::server::PmsClient;
use plexify::plex::{ClientInfo, Http};
use std::time::Duration;

fn http() -> Http {
    Http::new(ClientInfo::new("test-client")).with_backoff(Duration::from_millis(5))
}

fn setup() -> (MockServer, PmsClient) {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    (mock, PmsClient::new(http(), conn))
}

fn sync(pms: &PmsClient, db: &mut LibraryDb, full: bool) -> plexify::cache::sync::SyncReport {
    sync_library(pms, db, MUSIC_SECTION, full, &mut |_| {}).unwrap()
}

#[test]
fn first_sync_is_full_and_populates_everything() {
    let (mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let r = sync(&pms, &mut db, false);
    assert!(r.full);
    let (na, nal, nt) = mock.with_library(|l| (l.artists.len(), l.albums.len(), l.tracks.len()));
    assert_eq!(db.count(Kind::Artist).unwrap(), na);
    assert_eq!(db.count(Kind::Album).unwrap(), nal);
    assert_eq!(db.count(Kind::Track).unwrap(), nt);
    assert_eq!(r.playlists_refreshed, 2);
    let m = LibraryModel::load(&db).unwrap();
    assert_eq!(m.tracks.len(), nt);
    assert_eq!(m.playlists.len(), 2);
    assert_eq!(m.playlist(500).unwrap().items.len(), 5);
    // Album track order follows the index.
    let cont = m.albums.iter().find(|a| &*a.title == "Continuum").unwrap();
    let ids = m.album_track_ids(cont.id);
    assert_eq!(ids.len(), 2);
    assert_eq!(&*m.track(ids[0]).unwrap().title, "Continuum I");
    assert!(m.genres.iter().any(|(g, _)| &**g == "Rock"));
    // Unicode survives the round trip.
    assert!(m.tracks.iter().any(|t| &*t.title == "日本の夜"));
}

#[test]
fn delta_sync_picks_up_updates_additions_and_deletions() {
    let (mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync(&pms, &mut db, false);

    // Nothing changed -> delta with nothing deleted.
    let r = sync(&pms, &mut db, false);
    assert!(!r.full);
    assert_eq!(r.deleted, 0);
    assert!(r.swept_kinds.is_empty());
    assert_eq!(r.playlists_refreshed, 0);

    // Rename one track, add one, delete another.
    let (renamed, added, deleted) = mock.with_library(|l| {
        let t = l.tick();
        l.tracks[0].title = "Low Orbit (Remastered)".into();
        l.tracks[0].updated_at = t;
        let renamed = l.tracks[0].id;
        let mut nt: Track = l.tracks[1].clone();
        nt.id = 19_999;
        nt.title = "Bonus Track".into();
        nt.added_at = t;
        nt.updated_at = t;
        l.tracks.push(nt);
        let deleted = l.tracks.remove(5).id;
        (renamed as i64, 19_999i64, deleted as i64)
    });
    let r = sync(&pms, &mut db, false);
    assert!(!r.full);
    assert_eq!(r.swept_kinds, vec!["tracks"], "count mismatch triggers sweep");
    let m = LibraryModel::load(&db).unwrap();
    assert_eq!(&*m.track(renamed).unwrap().title, "Low Orbit (Remastered)");
    assert!(m.track(added).is_some());
    assert!(m.track(deleted).is_none());
    assert_eq!(m.tracks.len(), mock.with_library(|l| l.tracks.len()));
}

#[test]
fn delta_update_without_count_change_needs_no_sweep() {
    let (mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync(&pms, &mut db, false);
    let id = mock.with_library(|l| {
        let t = l.tick();
        l.albums[2].title = "Rust & Rivets (Deluxe)".into();
        l.albums[2].updated_at = t;
        l.albums[2].id as i64
    });
    let r = sync(&pms, &mut db, false);
    assert!(r.swept_kinds.is_empty());
    // The inclusive watermark re-fetches the newest album (+ its 4 tracks) and the changed one.
    assert!(r.upserted >= 1 && r.upserted <= 6, "only changed items are fetched, got {}", r.upserted);
    let m = LibraryModel::load(&db).unwrap();
    assert_eq!(&*m.album(id).unwrap().title, "Rust & Rivets (Deluxe)");
}

#[test]
fn playlist_changes_refetch_only_that_playlist() {
    let (mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync(&pms, &mut db, false);
    mock.with_library(|l| {
        let t = l.tick();
        let p = l.playlists.iter_mut().find(|p| p.id == 501).unwrap();
        p.items.push(10_000);
        p.updated_at = t;
    });
    let r = sync(&pms, &mut db, false);
    assert_eq!(r.playlists_refreshed, 1);
    let m = LibraryModel::load(&db).unwrap();
    assert_eq!(m.playlist(501).unwrap().items, vec![10_004, 10_005, 10_000]);
    // Deleting a playlist removes it locally.
    mock.with_library(|l| l.playlists.retain(|p| p.id != 500));
    sync(&pms, &mut db, false);
    assert!(LibraryModel::load(&db).unwrap().playlist(500).is_none());
}

#[test]
fn offline_restart_reads_cache_from_disk() {
    let dir = std::env::temp_dir().join(format!("plexify-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("srv_1.sqlite");
    let n = {
        let (mock, pms) = setup();
        let mut db = LibraryDb::open(&path).unwrap();
        sync(&pms, &mut db, false);
        mock.with_library(|l| l.tracks.len())
        // mock dropped here: server is gone
    };
    let db = LibraryDb::open(&path).unwrap();
    let m = LibraryModel::load(&db).unwrap();
    assert_eq!(m.tracks.len(), n);
    assert!(m.last_sync_at.is_some());
    // Clearing empties it.
    db.clear().unwrap();
    assert!(LibraryModel::load(&db).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_section_syncs_cleanly() {
    let (_mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    let r = sync_library(&pms, &mut db, mockplex::EMPTY_SECTION, false, &mut |_| {}).unwrap();
    assert_eq!(r.upserted, 0);
    let m = LibraryModel::load(&db).unwrap();
    assert!(m.tracks.is_empty() && m.albums.is_empty());
}

#[test]
fn server_failure_mid_sync_is_an_error_and_keeps_old_cache() {
    let (mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync(&pms, &mut db, false);
    let before = db.count(Kind::Track).unwrap();
    mock.set_server_down(true);
    let pms_fast = PmsClient::new(http().without_retries(), pms.conn.clone());
    assert!(sync_library(&pms_fast, &mut db, MUSIC_SECTION, false, &mut |_| {}).is_err());
    assert_eq!(db.count(Kind::Track).unwrap(), before);
}

#[test]
fn local_plays_and_ratings_are_recorded() {
    let (_mock, pms) = setup();
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync(&pms, &mut db, false);
    let before = LibraryModel::load(&db).unwrap().track(10_001).unwrap().view_count;
    db.record_play(10_001, 1_800_000_000).unwrap();
    db.set_track_rating(10_001, Some(6.0)).unwrap();
    let m = LibraryModel::load(&db).unwrap();
    let t = m.track(10_001).unwrap();
    assert_eq!(t.view_count, before + 1);
    assert_eq!(t.last_viewed_at, 1_800_000_000);
    assert_eq!(t.user_rating, 6.0);
}
