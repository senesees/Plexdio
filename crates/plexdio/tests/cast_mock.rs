//! Cast (remote control of other Plex players) against the mock server's fake players.

use mockplex::{MockConfig, MockServer, USER_TOKEN};
use plexdio::player::cast::{CastCmd, CastSession};
use plexdio::plex::auth::PlexTv;
use plexdio::plex::remote::discover;
use plexdio::plex::resources::pick_best;
use plexdio::plex::server::PmsClient;
use plexdio::plex::{ClientInfo, Http};
use std::time::{Duration, Instant};

fn http() -> Http {
    Http::new(ClientInfo::new("test-client")).with_backoff(Duration::from_millis(5))
}

fn setup() -> (MockServer, PlexTv, PmsClient) {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    (mock, tv, PmsClient::new(http(), conn))
}

/// Wait until `f` holds (the cast worker is asynchronous).
fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn discovery_merges_account_and_server_players() {
    let (_mock, tv, pms) = setup();
    let players = discover(&tv, USER_TOKEN, &pms);
    let names: Vec<&str> = players.iter().map(|p| p.name.as_str()).collect();
    // "Living Room TV" is on both lists but shows once.
    assert_eq!(names, vec!["Kitchen", "Living Room TV"]);
}

#[test]
fn multicast_play_pause_skip_and_stop() {
    let (mock, tv, pms) = setup();
    let players = discover(&tv, USER_TOKEN, &pms);
    let session = CastSession::start(pms, players, || {});
    session.send(CastCmd::Play { ids: vec![10_000, 10_001, 10_002], start: 10_001, offset_ms: 3000 });
    eventually("both players playing", || {
        ["mock-player-1", "mock-player-3"].iter().all(|id| mock.player(id).is_some_and(|p| p.state == "playing" && p.queue == vec![10_000, 10_001, 10_002]))
    });
    let p = mock.player("mock-player-1").unwrap();
    assert_eq!(p.queue[p.index], 10_001, "starts at the chosen song");
    assert!(p.position_ms() >= 3000, "starts at the offset");
    eventually("timeline mirrored", || session.status().timeline.is_some_and(|t| t.state == "playing" && t.track == Some(10_001)));

    session.send(CastCmd::Toggle);
    eventually("paused", || ["mock-player-1", "mock-player-3"].iter().all(|id| mock.player(id).unwrap().state == "paused"));
    eventually("timeline paused", || session.status().timeline.is_some_and(|t| t.state == "paused"));
    session.send(CastCmd::Next);
    eventually("skipped", || mock.player("mock-player-3").is_some_and(|p| p.queue[p.index] == 10_002));
    session.send(CastCmd::Volume(40));
    eventually("volume", || mock.player("mock-player-1").unwrap().volume == 40);
    session.send(CastCmd::Stop);
    eventually("stopped", || ["mock-player-1", "mock-player-3"].iter().all(|id| mock.player(id).unwrap().state == "stopped"));
    assert!(session.status().error.is_none(), "{:?}", session.status().error);
}
