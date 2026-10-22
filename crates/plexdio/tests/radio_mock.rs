//! Radio source against the mock server: sonic suggestions when Plex Pass is simulated, an
//! empty list (which triggers the local fallback in the app) when it is not.

use mockplex::{MockConfig, MockServer};
use plexdio::plex::{ClientInfo, Http};
use plexdio::plex::auth::PlexTv;
use plexdio::plex::resources::pick_best;
use plexdio::plex::server::PmsClient;
use plexdio::player::radio;
use std::time::Duration;

fn pms(mock: &MockServer) -> PmsClient {
    let http = Http::new(ClientInfo::new("test-client"));
    let tv = PlexTv::new(http.clone(), Some(&mock.url));
    let servers = tv.servers(mockplex::USER_TOKEN).unwrap();
    let conn = pick_best(&http.info, &servers[0], mockplex::USER_TOKEN, Duration::from_secs(2)).unwrap();
    PmsClient::new(http, conn)
}

#[test]
fn sonic_radio_returns_similar_tracks_without_the_seed() {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let client = pms(&mock);
    let ids = radio::sonic_ids(&client, 10_000, 10);
    assert!(!ids.is_empty(), "the mock answers /nearest");
    assert!(!ids.contains(&10_000), "the seed is not its own neighbour");
    assert!(ids.len() <= 10, "limit is respected");
}

#[test]
fn without_plex_pass_the_station_uses_the_local_fallback() {
    let mock = MockServer::start(MockConfig { sonic: false, ..MockConfig::default() }).unwrap();
    let client = pms(&mock);
    assert!(radio::sonic_ids(&client, 10_000, 10).is_empty(), "404 means local fallback");
}

#[test]
fn related_artists_come_from_the_artist_hubs() {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let client = pms(&mock);
    let lib = mockplex::library::Library::fixture();
    let seed = &lib.artists[0];
    let ids = plexdio::player::similar::server_ids(&client, seed.id as i64);
    assert!(!ids.contains(&(seed.id as i64)), "an artist is not similar to itself");
    let expected: Vec<i64> = lib.artists.iter().filter(|a| a.id != seed.id && a.genre == seed.genre).map(|a| a.id as i64).collect();
    assert!(!expected.is_empty(), "the fixture has artists sharing a genre");
    assert_eq!(ids, expected, "every artist from the similar-artists hub, in order");
}
