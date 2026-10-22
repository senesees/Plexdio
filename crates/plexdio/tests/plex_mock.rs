//! Integration tests: Plex client against the in-process mock server.

use mockplex::{MockConfig, MockServer, MUSIC_SECTION, SERVER_TOKEN, USER_TOKEN};
use plexdio::plex::auth::PlexTv;
use plexdio::plex::models::item_type;
use plexdio::plex::resources::pick_best;
use plexdio::plex::server::{PlayState, PmsClient};
use plexdio::plex::{ClientInfo, Http, PlexError};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn http() -> Http {
    Http::new(ClientInfo::new("test-client")).with_backoff(Duration::from_millis(10))
}

fn start() -> MockServer {
    MockServer::start(MockConfig::default()).unwrap()
}

fn pms(mock: &MockServer) -> PmsClient {
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    PmsClient::new(http(), conn)
}

#[test]
fn pin_flow_until_authorized() {
    let mock = start();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let pin = tv.create_pin().unwrap();
    let url = tv.auth_url(&pin);
    assert!(url.starts_with(&format!("{}/auth#?clientID=test-client&code={}", mock.url, pin.code)), "{url}");
    assert!(url.contains("context%5Bdevice%5D%5Bproduct%5D=Plexdio"));
    // Pending on the first poll, then auto-approved by the mock.
    assert_eq!(tv.check_pin(&pin).unwrap(), None);
    let token = tv.wait_for_pin(&pin, Duration::from_millis(20), Duration::from_secs(5), &AtomicBool::new(false)).unwrap();
    assert_eq!(token, USER_TOKEN);
    let user = tv.user(&token).unwrap();
    assert_eq!(user.display_name(), "Mock User");
    // The mandatory headers were sent.
    assert_eq!(mock.requests_to("/api/v2/pins").len(), 1);
}

#[test]
fn pin_wait_can_be_cancelled_and_times_out() {
    let mock = MockServer::start(MockConfig { auto_approve_after: None, ..Default::default() }).unwrap();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let pin = tv.create_pin().unwrap();
    let err = tv.wait_for_pin(&pin, Duration::from_millis(10), Duration::from_millis(80), &AtomicBool::new(false)).unwrap_err();
    assert_eq!(err, PlexError::Timeout);
    let err = tv.wait_for_pin(&pin, Duration::from_millis(10), Duration::from_secs(5), &AtomicBool::new(true)).unwrap_err();
    assert_eq!(err, PlexError::Cancelled);
    // Manual approval via the browser page.
    mock.approve_all_pins();
    assert_eq!(tv.check_pin(&pin).unwrap().as_deref(), Some(USER_TOKEN));
}

#[test]
fn expired_token_is_unauthorized() {
    let mock = start();
    let tv = PlexTv::new(http(), Some(&mock.url));
    assert!(tv.user(USER_TOKEN).is_ok());
    mock.expire_user_token();
    assert_eq!(tv.user(USER_TOKEN).unwrap_err(), PlexError::Unauthorized);
    assert_eq!(tv.resources(USER_TOKEN).unwrap_err(), PlexError::Unauthorized);
}

#[test]
fn resources_and_connection_racing_skip_dead_connection() {
    let mock = start();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    assert_eq!(servers.len(), 2, "players are filtered out");
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    assert_eq!(conn.uri, mock.url);
    assert!(conn.local);
    assert_eq!(conn.token, SERVER_TOKEN, "server access token is preferred");
    // The shared server is unreachable -> clear error, no panic.
    let err = pick_best(&http().info, &servers[1], USER_TOKEN, Duration::from_millis(800)).unwrap_err();
    assert!(matches!(err, PlexError::Network(_)), "{err:?}");
}

#[test]
fn sections_paging_children_and_metadata() {
    let mock = start();
    let pms = pms(&mock);
    let sections = pms.music_sections().unwrap();
    assert_eq!(sections.len(), 2, "movie section filtered out");
    let total = pms.section_count(MUSIC_SECTION, item_type::TRACK).unwrap();
    assert!(total > 20);
    // Small pages to exercise the paging loop.
    let p1 = pms.section_items(MUSIC_SECTION, item_type::TRACK, 0, 10, &[]).unwrap();
    assert_eq!(p1.items.len(), 10);
    assert_eq!(p1.total, Some(total));
    let all = pms.section_items_all(MUSIC_SECTION, item_type::TRACK, &[], |_, _| {}).unwrap();
    assert_eq!(all.len(), total);
    let artists = pms.section_items_all(MUSIC_SECTION, item_type::ARTIST, &[], |_, _| {}).unwrap();
    assert_eq!(artists.len(), 5);
    let albums = pms.children(artists[0].rating_key.as_deref().unwrap()).unwrap();
    assert!(!albums.is_empty());
    let tracks = pms.children(albums[0].rating_key.as_deref().unwrap()).unwrap();
    assert!(tracks.windows(2).all(|w| w[0].index < w[1].index));
    let detail = pms.metadata(tracks[0].rating_key.as_deref().unwrap()).unwrap().unwrap();
    assert!(detail.audio_stream().unwrap().gain.is_some(), "detail carries loudness");
    // Empty library section is fine.
    assert_eq!(pms.section_count(mockplex::EMPTY_SECTION, item_type::TRACK).unwrap(), 0);
    assert!(pms.metadata("999999").is_err());
}

#[test]
fn delta_filter_returns_only_changed() {
    let mock = start();
    let pms = pms(&mock);
    let since = mock.with_library(|l| {
        let t = l.tick();
        l.tracks[3].title = "Renamed".into();
        l.tracks[3].updated_at = t;
        t
    });
    let s = since.to_string();
    let changed = pms.section_items_all(MUSIC_SECTION, item_type::TRACK, &[("updatedAt>>", &s)], |_, _| {}).unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].title.as_deref(), Some("Renamed"));
}

#[test]
fn search_playlists_genres_and_radio() {
    let mock = start();
    let pms = pms(&mock);
    let hubs = pms.search(MUSIC_SECTION, "iron", 10).unwrap();
    let tracks = hubs.iter().find(|h| h.kind.as_deref() == Some("track")).unwrap();
    assert_eq!(tracks.metadata.len(), 2);
    let pls = pms.playlists().unwrap();
    assert_eq!(pls.len(), 2);
    let items = pms.playlist_items(pls[0].rating_key.as_deref().unwrap()).unwrap();
    assert_eq!(items.len(), 5);
    assert!(items[0].playlist_item_id.is_some());
    assert!(!pms.genres(MUSIC_SECTION).unwrap().is_empty());
    let near = pms.nearest_tracks("10000", 5, 0.25).unwrap();
    assert!(!near.is_empty() && near.len() <= 5);
    assert!(near.iter().all(|t| t.rating_key.as_deref() != Some("10000")));
    let related = pms.related("10000").unwrap();
    assert_eq!(related.len(), 1);
}

#[test]
fn nearest_missing_without_plex_pass_is_not_found() {
    let mock = MockServer::start(MockConfig { sonic: false, ..Default::default() }).unwrap();
    let pms = pms(&mock);
    assert_eq!(pms.nearest_tracks("10000", 5, 0.25).unwrap_err(), PlexError::NotFound);
}

#[test]
fn reporting_scrobble_and_rating_reach_server() {
    let mock = start();
    let pms = pms(&mock);
    pms.timeline("10001", PlayState::Playing, 1500, 8000).unwrap();
    pms.scrobble("10001").unwrap();
    pms.rate("10001", Some(10)).unwrap();
    let tl = mock.requests_to("/:/timeline");
    assert_eq!(tl.len(), 1);
    assert_eq!(tl[0].query.get("state").map(String::as_str), Some("playing"));
    assert_eq!(tl[0].query.get("time").map(String::as_str), Some("1500"));
    let (views, rating) = mock.with_library(|l| {
        let t = l.track(10001).unwrap();
        (t.view_count, t.user_rating)
    });
    assert_eq!(views, 10001 % 7 + 1);
    assert_eq!(rating, Some(10.0));
    pms.rate("10001", None).unwrap();
    assert_eq!(mock.with_library(|l| l.track(10001).unwrap().user_rating), None);
}

#[test]
fn streaming_urls_and_range_requests() {
    let mock = start();
    let pms = pms(&mock);
    let detail = pms.metadata("10001").unwrap().unwrap();
    let url = pms.part_url(detail.first_part().unwrap().key.as_deref().unwrap());
    let (status, len, mut rd) = pms.http().get_stream(&url, None).unwrap();
    assert_eq!(status, 200);
    let mut all = Vec::new();
    rd.read_to_end(&mut all).unwrap();
    assert_eq!(len, Some(all.len() as u64));
    let (status, _, mut rd) = pms.http().get_stream(&url, Some(100)).unwrap();
    assert_eq!(status, 206);
    let mut tail = Vec::new();
    rd.read_to_end(&mut tail).unwrap();
    assert_eq!(&all[100..], &tail[..]);
    let art = pms.photo_url("/library/metadata/2000/thumb/1", 120, 120);
    let (status, _, mut rd) = pms.http().get_stream(&art, None).unwrap();
    assert_eq!(status, 200);
    let mut jpg = Vec::new();
    rd.read_to_end(&mut jpg).unwrap();
    assert_eq!(&jpg[..2], &[0xFF, 0xD8], "JPEG magic");
    let tc = pms.transcode_url("10004", 192, "sess1", "client");
    assert_eq!(pms.http().get_stream(&tc, None).unwrap().0, 200);
}

#[test]
fn server_down_retries_then_reports_http_error() {
    let mock = start();
    let pms = pms(&mock);
    mock.set_server_down(true);
    let before = mock.requests_to("/library/sections").len();
    let err = pms.music_sections().unwrap_err();
    assert_eq!(err, PlexError::Http(503));
    assert!(err.is_transient());
    assert_eq!(mock.requests_to("/library/sections").len() - before, 4, "1 try + 3 retries");
    mock.set_server_down(false);
    assert!(pms.music_sections().is_ok());
}

#[test]
fn bad_server_token_is_unauthorized() {
    let mock = start();
    let mut p = pms(&mock);
    p.conn.token = "wrong".into();
    assert_eq!(p.music_sections().unwrap_err(), PlexError::Unauthorized);
}

use std::io::Read;
