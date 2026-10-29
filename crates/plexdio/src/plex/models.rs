//! Serde models for the JSON flavour of the Plex APIs (`Accept: application/json`).
//! Fields are optional and lenient: servers in the wild differ in what they include, and some
//! numeric fields appear as strings.

use serde::{Deserialize, Deserializer, Serialize};

/// Accept a number, a numeric string, or null.
fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        N(f64),
        S(String),
    }
    Ok(match Option::<NumOrStr>::deserialize(d)? {
        Some(NumOrStr::N(n)) => Some(n as i64),
        Some(NumOrStr::S(s)) => s.trim().parse::<f64>().ok().map(|n| n as i64),
        None => None,
    })
}

fn lenient_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        N(f64),
        S(String),
    }
    Ok(match Option::<NumOrStr>::deserialize(d)? {
        Some(NumOrStr::N(n)) => Some(n),
        Some(NumOrStr::S(s)) => s.trim().parse::<f64>().ok(),
        None => None,
    })
}

/// Accept bool, 0/1, or "0"/"1".
fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Num(i64),
        Str(String),
    }
    Ok(match Option::<Flag>::deserialize(d)? {
        Some(Flag::Bool(b)) => b,
        Some(Flag::Num(n)) => n != 0,
        Some(Flag::Str(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        None => false,
    })
}

/// ratingKey and friends: string or number -> String.
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        S(String),
        N(i64),
    }
    Ok(match Option::<S>::deserialize(d)? {
        Some(S::S(s)) => Some(s),
        Some(S::N(n)) => Some(n.to_string()),
        None => None,
    })
}

// ---------------------------------------------------------------------------------------------
// plex.tv

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub id: i64,
    pub code: String,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlexUser {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub id: Option<i64>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
}

impl PlexUser {
    pub fn display_name(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| "Plex user".into())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub port: Option<i64>,
    pub uri: String,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub local: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub relay: bool,
    #[serde(default, rename = "IPv6", deserialize_with = "lenient_bool")]
    pub ipv6: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub product_version: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    pub client_identifier: String,
    #[serde(default)]
    pub provides: String,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub owned: bool,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub source_title: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub presence: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub https_required: bool,
    #[serde(default)]
    pub connections: Vec<Connection>,
}

impl Resource {
    pub fn is_server(&self) -> bool {
        self.provides.split(',').any(|p| p.trim() == "server")
    }
}

// ---------------------------------------------------------------------------------------------
// Plex Media Server

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ContainerEnvelope {
    #[serde(rename = "MediaContainer")]
    pub media_container: MediaContainer,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaContainer {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub size: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub total_size: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub offset: Option<i64>,
    #[serde(default)]
    pub machine_identifier: Option<String>,
    #[serde(default)]
    pub friendly_name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "Directory")]
    pub directories: Vec<Directory>,
    #[serde(default, rename = "Metadata")]
    pub metadata: Vec<Metadata>,
    #[serde(default, rename = "Hub")]
    pub hubs: Vec<Hub>,
}

/// A library section or a browse directory (genre, etc).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    #[serde(default, deserialize_with = "lenient_string")]
    pub key: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub updated_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub scanned_at: Option<i64>,
    #[serde(default)]
    pub fast_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct Tag {
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub stream_type: Option<i64>,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub gain: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub album_gain: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub peak: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub album_peak: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub loudness: Option<f64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub id: Option<i64>,
    /// `/library/streams/{id}`: where a lyrics stream's text is fetched from.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub sampling_rate: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub bit_depth: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub channels: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub bitrate: Option<i64>,
    /// Lyrics format: `lrc` (timed) or `txt`.
    #[serde(default)]
    pub format: Option<String>,
    /// Lyrics are time-synced (some servers send "1"/"0" strings).
    #[serde(default, deserialize_with = "lenient_bool")]
    pub timed: bool,
    /// Where lyrics come from (e.g. a sidecar file or LyricFind).
    #[serde(default)]
    pub provider: Option<String>,
}

pub mod stream_type {
    pub const AUDIO: i64 = 2;
    pub const LYRICS: i64 = 4;
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub id: Option<i64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub duration: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub size: Option<i64>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default, rename = "Stream")]
    pub streams: Vec<Stream>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub id: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub duration: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub bitrate: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub audio_channels: Option<i64>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default, rename = "Part")]
    pub parts: Vec<Part>,
}

/// Artist / album / track / playlist item. Which fields are present depends on `kind`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default, deserialize_with = "lenient_string")]
    pub rating_key: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_sort: Option<String>,
    #[serde(default)]
    pub original_title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default)]
    pub art: Option<String>,
    #[serde(default)]
    pub composite: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub parent_rating_key: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub grandparent_rating_key: Option<String>,
    #[serde(default)]
    pub parent_title: Option<String>,
    #[serde(default)]
    pub grandparent_title: Option<String>,
    #[serde(default)]
    pub parent_thumb: Option<String>,
    #[serde(default)]
    pub grandparent_thumb: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub parent_index: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub year: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub parent_year: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub duration: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub added_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub updated_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub last_viewed_at: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub view_count: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub skip_count: Option<i64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub user_rating: Option<f64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub leaf_count: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub child_count: Option<i64>,
    #[serde(default)]
    pub playlist_type: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub smart: bool,
    #[serde(default, deserialize_with = "lenient_i64", rename = "playlistItemID")]
    pub playlist_item_id: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64", rename = "librarySectionID")]
    pub library_section_id: Option<i64>,
    #[serde(default, rename = "Genre")]
    pub genres: Vec<Tag>,
    #[serde(default, rename = "Media")]
    pub media: Vec<Media>,
    /// Sonic distance for `nearest` results.
    #[serde(default, deserialize_with = "lenient_f64")]
    pub distance: Option<f64>,
}

impl Metadata {
    pub fn first_part(&self) -> Option<&Part> {
        self.media.first().and_then(|m| m.parts.first())
    }

    /// Audio stream of the first part (carries loudness/gain data when analysed).
    pub fn audio_stream(&self) -> Option<&Stream> {
        self.first_part()
            .and_then(|p| p.streams.iter().find(|s| s.stream_type == Some(2)))
    }

    /// The lyrics stream of the first part, if the server has lyrics for this track.
    pub fn lyrics_stream(&self) -> Option<&Stream> {
        self.first_part()
            .and_then(|p| p.streams.iter().find(|s| s.stream_type == Some(stream_type::LYRICS) && s.key.is_some()))
    }

    pub fn genre_names(&self) -> Vec<String> {
        self.genres.iter().map(|g| g.tag.clone()).filter(|t| !t.is_empty()).collect()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Hub {
    #[serde(default)]
    pub hub_identifier: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub size: Option<i64>,
    #[serde(default, rename = "Metadata")]
    pub metadata: Vec<Metadata>,
}

/// Plex library item types used in `?type=` filters.
pub mod item_type {
    pub const ARTIST: u32 = 8;
    pub const ALBUM: u32 = 9;
    pub const TRACK: u32 = 10;
}
