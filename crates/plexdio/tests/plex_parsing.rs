//! Parsing tests against recorded-shape Plex JSON responses.

use plexdio::plex::http::parse_json;
use plexdio::plex::models::{ContainerEnvelope, Pin, PlexUser, Resource};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn pin_pending_and_authorized() {
    let p: Pin = parse_json(&fixture("pin_created.json")).unwrap();
    assert_eq!(p.id, 2104529281);
    assert_eq!(p.code, "4zjy7nr5k3a1x9m2q8w6v0b3c");
    assert_eq!(p.auth_token, None);
    assert_eq!(p.expires_in, Some(1800));
    let p: Pin = parse_json(&fixture("pin_authorized.json")).unwrap();
    assert_eq!(p.auth_token.as_deref(), Some("xYzAbC123-token"));
}

#[test]
fn user_account() {
    let u: PlexUser = parse_json(&fixture("user.json")).unwrap();
    assert_eq!(u.username.as_deref(), Some("musicfan"));
    assert_eq!(u.display_name(), "Music Fan");
}

#[test]
fn resources_servers_and_lenient_fields() {
    let r: Vec<Resource> = parse_json(&fixture("resources.json")).unwrap();
    assert_eq!(r.len(), 3);
    let servers: Vec<_> = r.iter().filter(|r| r.is_server()).collect();
    assert_eq!(servers.len(), 2);
    let nas = servers[0];
    assert!(nas.owned);
    assert_eq!(nas.connections.len(), 3);
    assert!(nas.connections[0].local);
    assert!(nas.connections[2].relay);
    // Shared server uses string-encoded booleans/ports.
    let shared = servers[1];
    assert!(!shared.owned);
    assert!(shared.presence);
    assert!(shared.https_required);
    assert_eq!(shared.connections[0].port, Some(32400));
    assert!(!shared.connections[0].local);
    assert_eq!(shared.source_title.as_deref(), Some("alex"));
}

#[test]
fn library_sections_filters_music() {
    let env: ContainerEnvelope = parse_json(&fixture("sections.json")).unwrap();
    let music: Vec<_> = env
        .media_container
        .directories
        .iter()
        .filter(|d| d.kind.as_deref() == Some("artist"))
        .collect();
    assert_eq!(music.len(), 2);
    assert_eq!(music[0].key.as_deref(), Some("5"));
    assert_eq!(music[1].updated_at, Some(1700000700));
}

#[test]
fn track_page_with_mixed_types() {
    let env: ContainerEnvelope = parse_json(&fixture("tracks_page.json")).unwrap();
    let mc = env.media_container;
    assert_eq!(mc.total_size, Some(2412));
    assert_eq!(mc.metadata.len(), 2);
    let t = &mc.metadata[0];
    assert_eq!(t.rating_key.as_deref(), Some("48211"));
    assert_eq!(t.grandparent_title.as_deref(), Some("Radiohead"));
    assert_eq!(t.user_rating, Some(8.0));
    assert_eq!(t.view_count, Some(17));
    assert_eq!(t.genre_names(), ["Alternative", "Rock"]);
    assert_eq!(t.first_part().unwrap().key.as_deref(), Some("/library/parts/61234/1650000000/file.flac"));
    // Second track: numeric ratingKey and string numbers.
    let t2 = &mc.metadata[1];
    assert_eq!(t2.rating_key.as_deref(), Some("48212"));
    assert_eq!(t2.index, Some(3));
    assert_eq!(t2.duration, Some(267000));
    assert_eq!(t2.media[0].bitrate, Some(320));
}

#[test]
fn track_detail_loudness() {
    let env: ContainerEnvelope = parse_json(&fixture("track_detail.json")).unwrap();
    let t = &env.media_container.metadata[0];
    let s = t.audio_stream().unwrap();
    assert_eq!(s.gain, Some(-8.37));
    assert_eq!(s.album_gain, Some(-7.91));
    assert!((s.peak.unwrap() - 0.999969).abs() < 1e-9);
}

#[test]
fn search_hubs_including_empty_hub() {
    let env: ContainerEnvelope = parse_json(&fixture("search_hubs.json")).unwrap();
    let hubs = env.media_container.hubs;
    assert_eq!(hubs.len(), 3);
    assert_eq!(hubs[0].metadata[0].title.as_deref(), Some("Radiohead"));
    assert_eq!(hubs[1].metadata[0].year, Some(1997));
    assert!(hubs[2].metadata.is_empty());
}

#[test]
fn playlists_audio() {
    let env: ContainerEnvelope = parse_json(&fixture("playlists.json")).unwrap();
    let p = &env.media_container.metadata;
    assert_eq!(p.len(), 2);
    assert!(p[0].smart);
    assert_eq!(p[0].leaf_count, Some(2412));
    assert_eq!(p[1].playlist_type.as_deref(), Some("audio"));
}

#[test]
fn empty_container_is_fine() {
    let env: ContainerEnvelope = parse_json(&fixture("empty_container.json")).unwrap();
    assert!(env.media_container.metadata.is_empty());
    assert_eq!(env.media_container.total_size, Some(0));
}

#[test]
fn garbage_is_parse_error() {
    let r: Result<ContainerEnvelope, _> = parse_json(b"<html>502 Bad Gateway</html>");
    assert!(matches!(r, Err(plexdio::plex::PlexError::Parse(_))));
}
