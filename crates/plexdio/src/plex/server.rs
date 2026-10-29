//! Plex Media Server client: library browsing, search, playback URLs, reporting and rating.

use super::http::{Http, Method};
use super::models::{ContainerEnvelope, Directory, Hub, MediaContainer, Metadata};
use super::resources::ServerConnection;
use super::Result;

pub const PAGE_SIZE: usize = 1000;
const LIBRARY_IDENTIFIER: &str = "com.plexapp.plugins.library";
/// Items per playlist add request (keeps the `uri` parameter a sensible length).
const PLAYLIST_CHUNK: usize = 200;

#[derive(Clone)]
pub struct PmsClient {
    http: Http,
    pub conn: ServerConnection,
}

/// A page of results with the server-reported total.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub items: Vec<Metadata>,
    pub total: Option<usize>,
}

/// Playback states for `/:/timeline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
    Buffering,
}

impl PlayState {
    pub fn as_str(self) -> &'static str {
        match self {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Stopped => "stopped",
            PlayState::Buffering => "buffering",
        }
    }
}

impl PmsClient {
    pub fn new(http: Http, conn: ServerConnection) -> Self {
        Self { http, conn }
    }

    pub fn base(&self) -> &str {
        &self.conn.uri
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.conn.uri, path)
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<MediaContainer> {
        let env: ContainerEnvelope = self.http.get_json(&self.url(path), Some(&self.conn.token), query)?;
        Ok(env.media_container)
    }

    pub fn identity(&self) -> Result<MediaContainer> {
        self.get("/identity", &[])
    }

    /// Music library sections (`type == "artist"`).
    pub fn music_sections(&self) -> Result<Vec<Directory>> {
        let mc = self.get("/library/sections", &[])?;
        Ok(mc
            .directories
            .into_iter()
            .filter(|d| d.kind.as_deref() == Some("artist"))
            .collect())
    }

    /// One page of `/library/sections/{id}/all?type=...`, with optional extra filters such as
    /// `("updatedAt>>", "1700000000")` for delta sync.
    pub fn section_items(&self, section: &str, item_type: u32, start: usize, size: usize, filters: &[(&str, &str)]) -> Result<Page> {
        let t = item_type.to_string();
        let start_s = start.to_string();
        let size_s = size.to_string();
        let mut q: Vec<(&str, &str)> = vec![
            ("type", &t),
            ("X-Plex-Container-Start", &start_s),
            ("X-Plex-Container-Size", &size_s),
            ("includeGuids", "0"),
        ];
        q.extend_from_slice(filters);
        let mc = self.get(&format!("/library/sections/{section}/all"), &q)?;
        Ok(Page { total: mc.total_size.or(mc.size).map(|n| n.max(0) as usize), items: mc.metadata })
    }

    /// Fetch every item of a type, page by page. `progress(done, total)` is called per page.
    pub fn section_items_all(
        &self,
        section: &str,
        item_type: u32,
        filters: &[(&str, &str)],
        mut progress: impl FnMut(usize, Option<usize>),
    ) -> Result<Vec<Metadata>> {
        let mut out = Vec::new();
        loop {
            let page = self.section_items(section, item_type, out.len(), PAGE_SIZE, filters)?;
            let n = page.items.len();
            out.extend(page.items);
            progress(out.len(), page.total);
            let done = n < PAGE_SIZE || page.total.is_some_and(|t| out.len() >= t);
            if done || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    /// Number of items of a type (cheap: asks for a zero-size page).
    pub fn section_count(&self, section: &str, item_type: u32) -> Result<usize> {
        let page = self.section_items(section, item_type, 0, 0, &[])?;
        Ok(page.total.unwrap_or(0))
    }

    pub fn metadata(&self, rating_key: &str) -> Result<Option<Metadata>> {
        let mc = self.get(&format!("/library/metadata/{rating_key}"), &[("includeLoudness", "1")])?;
        Ok(mc.metadata.into_iter().next())
    }

    /// Track metadata including its lyrics stream (if any).
    pub fn track_with_lyrics(&self, rating_key: &str) -> Result<Option<Metadata>> {
        let mc = self.get(&format!("/library/metadata/{rating_key}"), &[("includeLyrics", "1"), ("includeLoudness", "1")])?;
        Ok(mc.metadata.into_iter().next())
    }

    /// Raw content of a stream (`/library/streams/{id}`): the lyrics file text.
    pub fn stream_content(&self, key: &str) -> Result<Vec<u8>> {
        self.http.request(Method::Get, &self.url(key), Some(&self.conn.token), &[])
    }

    pub fn children(&self, rating_key: &str) -> Result<Vec<Metadata>> {
        Ok(self.get(&format!("/library/metadata/{rating_key}/children"), &[])?.metadata)
    }

    pub fn genres(&self, section: &str) -> Result<Vec<Directory>> {
        Ok(self.get(&format!("/library/sections/{section}/genre"), &[("type", "9")])?.directories)
    }

    pub fn playlists(&self) -> Result<Vec<Metadata>> {
        Ok(self.get("/playlists", &[("playlistType", "audio")])?.metadata)
    }

    pub fn playlist_items(&self, playlist_key: &str) -> Result<Vec<Metadata>> {
        let mut out = Vec::new();
        loop {
            let start = out.len().to_string();
            let size = PAGE_SIZE.to_string();
            let mc = self.get(
                &format!("/playlists/{playlist_key}/items"),
                &[("X-Plex-Container-Start", &start), ("X-Plex-Container-Size", &size)],
            )?;
            let n = mc.metadata.len();
            let total = mc.total_size.map(|t| t as usize);
            out.extend(mc.metadata);
            if n < PAGE_SIZE || total.is_some_and(|t| out.len() >= t) || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    /// `server://…/library/metadata/1,2,3` URI that playlist calls take for a list of items.
    fn items_uri(&self, ids: &[i64]) -> String {
        let ids: Vec<String> = ids.iter().map(i64::to_string).collect();
        format!("server://{}/{LIBRARY_IDENTIFIER}/library/metadata/{}", self.conn.server_id, ids.join(","))
    }

    /// Playlist edits are not idempotent (a retried add would add twice), so they run once.
    fn write(&self, method: Method, path: &str, query: &[(&str, &str)]) -> Result<Vec<u8>> {
        self.http.clone().without_retries().request(method, &self.url(path), Some(&self.conn.token), query)
    }

    /// Create a regular audio playlist holding `ids` (may be empty). Returns its ratingKey.
    pub fn create_playlist(&self, title: &str, ids: &[i64]) -> Result<i64> {
        let first = &ids[..ids.len().min(PLAYLIST_CHUNK)];
        let uri = self.items_uri(first);
        let mut q: Vec<(&str, &str)> = vec![("type", "audio"), ("title", title), ("smart", "0")];
        if !first.is_empty() {
            q.push(("uri", &uri));
        }
        let body = self.write(Method::Post, "/playlists", &q)?;
        let env: ContainerEnvelope = super::http::parse_json(&body)?;
        let id = env
            .media_container
            .metadata
            .first()
            .and_then(|m| m.rating_key.as_deref())
            .and_then(|k| k.parse().ok())
            .ok_or_else(|| super::PlexError::Parse("create playlist: no ratingKey in the response".into()))?;
        if ids.len() > first.len() {
            self.add_to_playlist(id, &ids[first.len()..])?;
        }
        Ok(id)
    }

    /// Append `ids` to a playlist (in chunks, so the URI stays short).
    pub fn add_to_playlist(&self, playlist: i64, ids: &[i64]) -> Result<()> {
        for chunk in ids.chunks(PLAYLIST_CHUNK) {
            let uri = self.items_uri(chunk);
            self.write(Method::Put, &format!("/playlists/{playlist}/items"), &[("uri", &uri)])?;
        }
        Ok(())
    }

    /// Remove one entry by its `playlistItemID`.
    pub fn remove_playlist_item(&self, playlist: i64, item_id: i64) -> Result<()> {
        self.write(Method::Delete, &format!("/playlists/{playlist}/items/{item_id}"), &[]).map(|_| ())
    }

    /// Move an entry after another one (`None` moves it to the top).
    pub fn move_playlist_item(&self, playlist: i64, item_id: i64, after: Option<i64>) -> Result<()> {
        let after = after.map(|a| a.to_string());
        let q: Vec<(&str, &str)> = after.as_deref().map(|a| vec![("after", a)]).unwrap_or_default();
        self.write(Method::Put, &format!("/playlists/{playlist}/items/{item_id}/move"), &q).map(|_| ())
    }

    pub fn rename_playlist(&self, playlist: i64, title: &str) -> Result<()> {
        self.write(Method::Put, &format!("/playlists/{playlist}"), &[("title", title)]).map(|_| ())
    }

    pub fn delete_playlist(&self, playlist: i64) -> Result<()> {
        self.write(Method::Delete, &format!("/playlists/{playlist}"), &[]).map(|_| ())
    }

    /// Server-side search across the section.
    pub fn search(&self, section: &str, query: &str, limit: usize) -> Result<Vec<Hub>> {
        let limit = limit.to_string();
        Ok(self
            .get("/hubs/search", &[("query", query), ("sectionId", section), ("limit", &limit)])?
            .hubs)
    }

    /// Sonically similar tracks (requires Plex Pass + sonic analysis; may 404 or be empty).
    pub fn nearest_tracks(&self, rating_key: &str, limit: usize, max_distance: f64) -> Result<Vec<Metadata>> {
        let limit = limit.to_string();
        let dist = format!("{max_distance:.2}");
        Ok(self
            .get(
                &format!("/library/metadata/{rating_key}/nearest"),
                &[("limit", &limit), ("maxDistance", &dist), ("excludeParentID", "-1")],
            )?
            .metadata)
    }

    /// Related hubs for an item (similar artists, more from the album, ...).
    pub fn related(&self, rating_key: &str) -> Result<Vec<Hub>> {
        Ok(self.get(&format!("/library/metadata/{rating_key}/related"), &[])?.hubs)
    }

    /// Report playback progress. `time_ms`/`duration_ms` in milliseconds.
    pub fn timeline(&self, rating_key: &str, state: PlayState, time_ms: u64, duration_ms: u64) -> Result<()> {
        let key = format!("/library/metadata/{rating_key}");
        let time = time_ms.to_string();
        let dur = duration_ms.to_string();
        self.http.send(
            Method::Get,
            &self.url("/:/timeline"),
            Some(&self.conn.token),
            &[
                ("ratingKey", rating_key),
                ("key", &key),
                ("state", state.as_str()),
                ("time", &time),
                ("duration", &dur),
                ("hasMDE", "1"),
            ],
        )
    }

    /// Mark a track as played.
    pub fn scrobble(&self, rating_key: &str) -> Result<()> {
        self.http.send(
            Method::Get,
            &self.url("/:/scrobble"),
            Some(&self.conn.token),
            &[("key", rating_key), ("identifier", LIBRARY_IDENTIFIER)],
        )
    }

    /// Rate 0..=10 (Plex stars are rating/2); `None` clears the rating.
    pub fn rate(&self, rating_key: &str, rating: Option<u8>) -> Result<()> {
        let r = rating.map_or("-1".to_string(), |r| r.min(10).to_string());
        self.http.send(
            Method::Put,
            &self.url("/:/rate"),
            Some(&self.conn.token),
            &[("key", rating_key), ("identifier", LIBRARY_IDENTIFIER), ("rating", &r)],
        )
    }

    /// Direct-play URL for a media part key (e.g. `/library/parts/1/123/file.flac`).
    pub fn part_url(&self, part_key: &str) -> String {
        format!("{}{}?X-Plex-Token={}", self.conn.uri, part_key, urlencoding::encode(&self.conn.token))
    }

    /// Server-side transcode to MP3 (used for codecs we can't decode and for bitrate caps).
    pub fn transcode_url(&self, rating_key: &str, max_kbps: u32, session: &str, client_id: &str) -> String {
        let path = urlencoding::encode(&format!("/library/metadata/{rating_key}")).into_owned();
        format!(
            "{}/music/:/transcode/universal/start.mp3?path={path}&mediaIndex=0&partIndex=0&protocol=http\
             &directPlay=0&directStream=0&musicBitrate={max_kbps}&maxAudioBitrate={max_kbps}\
             &session={}&X-Plex-Client-Identifier={}&X-Plex-Product=Plexdio&X-Plex-Platform=Windows&X-Plex-Token={}",
            self.conn.uri,
            urlencoding::encode(session),
            urlencoding::encode(client_id),
            urlencoding::encode(&self.conn.token)
        )
    }

    /// Sized thumbnail via the photo transcoder (JPEG).
    pub fn photo_url(&self, thumb: &str, width: u32, height: u32) -> String {
        format!(
            "{}/photo/:/transcode?width={width}&height={height}&minSize=1&upscale=1&format=jpeg&url={}&X-Plex-Token={}",
            self.conn.uri,
            urlencoding::encode(thumb),
            urlencoding::encode(&self.conn.token)
        )
    }

    pub fn http(&self) -> &Http {
        &self.http
    }
}
