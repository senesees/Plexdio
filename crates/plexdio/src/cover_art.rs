//! Public album-cover lookup for Discord Rich Presence.
//!
//! Discord can only show images from public https URLs. Plex artwork lives on the user's own
//! server (often only reachable on the LAN) and needs the Plex token, which must never leave
//! the machine. So the cover is looked up by artist and album name in public catalogues
//! instead: Deezer first, then the iTunes Search API. Neither needs an API key. Only results
//! whose artist *and* album match are used, so a wrong cover is never shown.

use serde_json::Value;
use std::io::Read;
use std::time::Duration;

/// Find a public cover URL for `artist` / `album`. Blocking (call from a worker thread).
pub fn lookup(artist: &str, album: &str) -> Option<String> {
    if artist.trim().is_empty() || album.trim().is_empty() {
        return None;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(4)))
        .http_status_as_error(false)
        .user_agent(concat!("Plexdio/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let deezer = format!(
        "https://api.deezer.com/search/album?limit=10&q={}",
        // A plain query: Deezer's `artist:"…" album:"…"` syntax misses on the album endpoint.
        urlencoding::encode(&format!("{artist} {album}"))
    );
    if let Some(url) = get_json(&agent, &deezer).and_then(|v| pick_deezer(&v, artist, album)) {
        return Some(url);
    }
    let itunes = format!(
        "https://itunes.apple.com/search?entity=album&limit=10&term={}",
        urlencoding::encode(&format!("{artist} {album}"))
    );
    get_json(&agent, &itunes).and_then(|v| pick_itunes(&v, artist, album))
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    let resp = agent.get(url).call().map_err(|e| log::debug!("cover lookup {url}: {e}")).ok()?;
    if !resp.status().is_success() {
        log::debug!("cover lookup {url}: HTTP {}", resp.status());
        return None;
    }
    let mut body = Vec::new();
    resp.into_body().into_reader().take(1 << 20).read_to_end(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Lower-case letters and digits only, so "AC/DC" == "acdc" and "Café" == "café".
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Album title without trailing "(Deluxe Edition)" / "[Remastered]" / " - Single" decorations.
fn norm_album(s: &str) -> String {
    let mut t = s;
    if let Some(i) = t.find(['(', '[']) {
        if i > 0 {
            t = &t[..i];
        }
    }
    if let Some(i) = t.find(" - ") {
        if i > 0 {
            t = &t[..i];
        }
    }
    norm(t)
}

fn matches(artist: &str, album: &str, want_artist: &str, want_album: &str) -> bool {
    let (a, wa) = (norm(artist), norm(want_artist));
    let (b, wb) = (norm_album(album), norm_album(want_album));
    !wa.is_empty() && !wb.is_empty() && (a == wa || a.contains(&wa) || wa.contains(&a)) && b == wb
}

fn pick_deezer(v: &Value, artist: &str, album: &str) -> Option<String> {
    v.get("data")?.as_array()?.iter().find_map(|item| {
        let title = item.get("title")?.as_str()?;
        let name = item.pointer("/artist/name")?.as_str()?;
        if !matches(name, title, artist, album) {
            return None;
        }
        ["cover_xl", "cover_big", "cover_medium"]
            .iter()
            .find_map(|k| item.get(*k).and_then(Value::as_str))
            .filter(|u| u.starts_with("https://"))
            .map(str::to_string)
    })
}

fn pick_itunes(v: &Value, artist: &str, album: &str) -> Option<String> {
    v.get("results")?.as_array()?.iter().find_map(|item| {
        let title = item.get("collectionName")?.as_str()?;
        let name = item.get("artistName")?.as_str()?;
        if !matches(name, title, artist, album) {
            return None;
        }
        // The API returns 100 px art; the same path serves larger sizes.
        let url = item.get("artworkUrl100")?.as_str()?;
        url.starts_with("https://").then(|| url.replace("100x100bb", "600x600bb"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deezer_picks_the_matching_album_only() {
        let v = json!({ "data": [
            { "title": "Other Album", "artist": { "name": "Daft Punk" }, "cover_xl": "https://e/wrong.jpg" },
            { "title": "Discovery (Remastered)", "artist": { "name": "Daft Punk" }, "cover_xl": "https://e/right.jpg" },
        ]});
        assert_eq!(pick_deezer(&v, "Daft Punk", "Discovery").as_deref(), Some("https://e/right.jpg"));
        assert_eq!(pick_deezer(&v, "Justice", "Discovery"), None, "artist must match too");
        assert_eq!(pick_deezer(&json!({ "data": [] }), "A", "B"), None);
    }

    #[test]
    fn itunes_upsizes_the_artwork_and_normalises_names() {
        let v = json!({ "results": [
            { "collectionName": "Highway to Hell", "artistName": "AC/DC", "artworkUrl100": "https://i/abc/100x100bb.jpg" },
        ]});
        assert_eq!(pick_itunes(&v, "ACDC", "Highway To Hell").as_deref(), Some("https://i/abc/600x600bb.jpg"));
        assert_eq!(pick_itunes(&v, "AC/DC", "Back in Black"), None);
    }

    /// Hits the real Deezer/iTunes APIs: `cargo test -p plexdio live_cover -- --ignored`.
    #[test]
    #[ignore]
    fn live_cover_lookup() {
        let found = lookup("Daft Punk", "Discovery");
        println!("Daft Punk — Discovery: {found:?}");
        assert!(found.is_some_and(|u| u.starts_with("https://")));
        assert_eq!(lookup("Mila Varga", "Blue Hours"), None, "made-up album: no cover");
    }

    #[test]
    fn album_decorations_are_ignored_but_titles_must_match() {
        assert!(matches("Mila Varga", "Blue Hours - Single", "Mila Varga", "Blue Hours"));
        assert!(matches("Déjà Vu Collective", "Café Électrique [Deluxe]", "déjà vu collective", "Café Électrique"));
        assert!(!matches("Mila Varga", "Blue Hours Live", "Mila Varga", "Blue Hours"));
    }
}
