//! Playlist editing against the mock server: create, add, move, remove, rename and delete, with
//! the cache refreshed the way the app does it after each edit.

use mockplex::{MockConfig, MockServer, MUSIC_SECTION, SERVER_ID, USER_TOKEN};
use plexdio::cache::db::LibraryDb;
use plexdio::cache::sync::{sync_library, sync_playlists};
use plexdio::library_model::{LibraryModel, Playlist};
use plexdio::plex::auth::PlexTv;
use plexdio::plex::resources::pick_best;
use plexdio::plex::server::PmsClient;
use plexdio::plex::{ClientInfo, Http};
use std::time::Duration;

fn http() -> Http {
    Http::new(ClientInfo::new("test-client")).with_backoff(Duration::from_millis(5))
}

fn setup() -> (MockServer, PmsClient, LibraryDb) {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    let pms = PmsClient::new(http(), conn);
    let mut db = LibraryDb::open_in_memory().unwrap();
    sync_library(&pms, &mut db, MUSIC_SECTION, true, &mut |_| {}).unwrap();
    (mock, pms, db)
}

/// Refresh like the app does after an edit, then read the cached playlist back.
fn refreshed(pms: &PmsClient, db: &mut LibraryDb, id: i64) -> Option<Playlist> {
    sync_playlists(pms, db, &[id]).unwrap();
    LibraryModel::load_playlists(db).unwrap().into_iter().find(|p| p.id == id)
}

#[test]
fn create_add_move_remove_rename_delete() {
    let (mock, pms, mut db) = setup();

    // Create with two tracks; the URI names this server.
    let id = pms.create_playlist("Road Trip", &[10_000, 10_001]).unwrap();
    let uri = mock.requests_to("/playlists").into_iter().find(|r| r.method == "POST").unwrap().query["uri"].clone();
    assert_eq!(uri, format!("server://{SERVER_ID}/com.plexapp.plugins.library/library/metadata/10000,10001"));
    let p = refreshed(&pms, &mut db, id).unwrap();
    assert_eq!(&*p.title, "Road Trip");
    assert_eq!(p.items, vec![10_000, 10_001]);
    assert!(p.item_ids.iter().all(Option::is_some), "server item ids are cached: {:?}", p.item_ids);

    // Add (duplicates are allowed by the server).
    pms.add_to_playlist(id, &[10_002, 10_000]).unwrap();
    let p = refreshed(&pms, &mut db, id).unwrap();
    assert_eq!(p.items, vec![10_000, 10_001, 10_002, 10_000]);

    // Move the third entry to the top, then the top one after the last.
    let ids: Vec<i64> = p.item_ids.iter().map(|i| i.unwrap()).collect();
    pms.move_playlist_item(id, ids[2], None).unwrap();
    pms.move_playlist_item(id, ids[2], Some(ids[3])).unwrap();
    let p = refreshed(&pms, &mut db, id).unwrap();
    assert_eq!(p.items, vec![10_000, 10_001, 10_000, 10_002]);

    // Remove the duplicate by its own item id (not the first entry with that track).
    let dup = p.item_ids[2].unwrap();
    pms.remove_playlist_item(id, dup).unwrap();
    let p = refreshed(&pms, &mut db, id).unwrap();
    assert_eq!(p.items, vec![10_000, 10_001, 10_002]);
    assert!(!p.item_ids.contains(&Some(dup)));

    pms.rename_playlist(id, "Night Drive").unwrap();
    assert_eq!(&*refreshed(&pms, &mut db, id).unwrap().title, "Night Drive");

    pms.delete_playlist(id).unwrap();
    assert!(refreshed(&pms, &mut db, id).is_none(), "deleted playlist leaves the cache");
    assert!(mock.with_library(|l| l.playlists.iter().all(|p| p.id as i64 != id)));
}

#[test]
fn empty_playlist_and_long_adds() {
    let (mock, pms, mut db) = setup();
    let id = pms.create_playlist("Empty", &[]).unwrap();
    assert!(refreshed(&pms, &mut db, id).unwrap().items.is_empty());
    // More ids than fit in one request go out in chunks, in order.
    let all: Vec<i64> = mock.with_library(|l| l.tracks.iter().map(|t| t.id as i64).collect());
    let many: Vec<i64> = all.iter().copied().cycle().take(450).collect();
    pms.add_to_playlist(id, &many).unwrap();
    assert_eq!(mock.requests_to(&format!("/playlists/{id}/items")).iter().filter(|r| r.method == "PUT").count(), 3);
    assert_eq!(refreshed(&pms, &mut db, id).unwrap().items, many);
}

#[test]
fn failed_edit_is_reported_and_not_retried() {
    let (mock, pms, _db) = setup();
    mock.set_server_down(true);
    assert!(pms.add_to_playlist(500, &[10_000]).is_err());
    // A retried add could add twice, so writes go out exactly once.
    assert_eq!(mock.requests_to("/playlists/500/items").iter().filter(|r| r.method == "PUT").count(), 1);
}
