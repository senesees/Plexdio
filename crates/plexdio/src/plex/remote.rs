//! Remote control of other Plex players ("Cast"): find players, hand them a play queue and send
//! transport commands. Everything goes through the media server, which forwards `/player/...`
//! calls to the player named in `X-Plex-Target-Client-Identifier` (so the player does not need
//! to be reachable from this computer). The calls follow python-plexapi's `PlexClient`.

use super::auth::PlexTv;
use super::http::Method;
use super::server::PmsClient;
use super::Result;
use serde_json::Value;

/// A player that can be cast to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    /// Client identifier (the target of commands).
    pub id: String,
    pub name: String,
    /// "Plexamp", "Plex for Android (TV)", ...
    pub product: String,
}

/// What a player reports about its music playback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteTimeline {
    /// "playing", "paused", "buffering" or "stopped".
    pub state: String,
    pub track: Option<i64>,
    pub time_ms: u64,
    pub duration_ms: u64,
    pub volume: Option<u32>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn n(v: &Value, k: &str) -> Option<u64> {
    s(v, k)?.parse::<f64>().ok().map(|f| f.max(0.0) as u64)
}

/// Players on the account (plex.tv resources that provide "player") plus those the server sees
/// (`/clients`), without duplicates. Either source may fail; the other still counts.
pub fn discover(tv: &PlexTv, user_token: &str, pms: &PmsClient) -> Vec<PlayerInfo> {
    let mut out: Vec<PlayerInfo> = Vec::new();
    let mut add = |p: PlayerInfo| {
        if !out.iter().any(|x| x.id == p.id) {
            out.push(p);
        }
    };
    match pms.clients() {
        Ok(list) => list.into_iter().for_each(&mut add),
        Err(e) => log::debug!("server player list failed: {e}"),
    }
    match tv.resources(user_token) {
        Ok(res) => res
            .into_iter()
            .filter(|r| r.provides.split(',').any(|p| p.trim() == "player") && r.presence)
            .for_each(|r| add(PlayerInfo { id: r.client_identifier, name: r.name, product: r.product.unwrap_or_default() })),
        Err(e) => log::debug!("plex.tv player list failed: {e}"),
    }
    // This app is not a cast target.
    out.retain(|p| p.id != pms.http().info.client_id);
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

impl PmsClient {
    fn remote(&self, method: Method, path: &str, query: &[(&str, &str)], target: Option<&str>) -> Result<Value> {
        let url = format!("{}{}", self.conn.uri, path);
        let extra: Vec<(&str, &str)> = target.map(|t| vec![("X-Plex-Target-Client-Identifier", t)]).unwrap_or_default();
        let body = self.http().request_once_with(method, &url, Some(&self.conn.token), query, &extra)?;
        if body.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// Players connected to this server.
    pub fn clients(&self) -> Result<Vec<PlayerInfo>> {
        let v = self.remote(Method::Get, "/clients", &[], None)?;
        let list = v["MediaContainer"]["Server"].as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|c| {
                Some(PlayerInfo { id: s(c, "machineIdentifier")?, name: s(c, "name").unwrap_or_else(|| "Player".into()), product: s(c, "product").unwrap_or_default() })
            })
            .collect())
    }

    /// A server-side play queue of `ids` (players play queues, not track lists). Returns its id.
    pub fn create_play_queue(&self, ids: &[i64]) -> Result<i64> {
        // Long lists are cut so the request stays a sensible length.
        let ids: Vec<String> = ids.iter().take(300).map(i64::to_string).collect();
        let uri = format!("server://{}/com.plexapp.plugins.library/library/metadata/{}", self.conn.server_id, ids.join(","));
        let key = format!("/library/metadata/{}", ids.first().map(String::as_str).unwrap_or(""));
        let v = self.remote(Method::Post, "/playQueues", &[("type", "audio"), ("uri", &uri), ("key", &key), ("shuffle", "0"), ("repeat", "0"), ("continuous", "0")], None)?;
        n(&v["MediaContainer"], "playQueueID").map(|id| id as i64).ok_or_else(|| super::PlexError::Parse("no playQueueID".into()))
    }

    /// Send `/player/playback/<command>` to a player.
    pub fn player_command(&self, target: &str, command: &str, query: &[(&str, &str)], command_id: u64) -> Result<()> {
        let id = command_id.to_string();
        let mut q: Vec<(&str, &str)> = query.to_vec();
        q.push(("commandID", &id));
        q.push(("type", "music"));
        self.remote(Method::Get, &format!("/player/playback/{command}"), &q, Some(target)).map(|_| ())
    }

    /// Start play queue `queue` on a player at `track`, `offset_ms` in.
    pub fn play_media(&self, target: &str, queue: i64, track: i64, offset_ms: u64, command_id: u64) -> Result<()> {
        let uri = url_parts(&self.conn.uri);
        let container = format!("/playQueues/{queue}?window=100&own=1");
        let key = format!("/library/metadata/{track}");
        let offset = offset_ms.to_string();
        let port = uri.port.to_string();
        self.player_command(
            target,
            "playMedia",
            &[
                ("containerKey", &container),
                ("key", &key),
                ("offset", &offset),
                ("machineIdentifier", &self.conn.server_id),
                ("providerIdentifier", "com.plexapp.plugins.library"),
                ("address", &uri.host),
                ("port", &port),
                ("protocol", &uri.scheme),
                ("token", &self.conn.token),
            ],
            command_id,
        )
    }

    /// The player's music timeline (state, track, position).
    pub fn player_timeline(&self, target: &str, command_id: u64) -> Result<RemoteTimeline> {
        let id = command_id.to_string();
        let v = self.remote(Method::Get, "/player/timeline/poll", &[("wait", "0"), ("commandID", &id)], Some(target))?;
        let list = v["MediaContainer"]["Timeline"].as_array().cloned().unwrap_or_default();
        let t = list.iter().find(|t| t["type"] == "music").cloned().unwrap_or(Value::Null);
        Ok(RemoteTimeline {
            state: s(&t, "state").unwrap_or_else(|| "stopped".into()),
            track: s(&t, "ratingKey").and_then(|k| k.parse().ok()),
            time_ms: n(&t, "time").unwrap_or(0),
            duration_ms: n(&t, "duration").unwrap_or(0),
            volume: n(&t, "volume").map(|v| v as u32),
        })
    }
}

struct UrlParts {
    scheme: String,
    host: String,
    port: u16,
}

/// Scheme, host and port of the server URI (players connect back to it).
fn url_parts(uri: &str) -> UrlParts {
    let (scheme, rest) = uri.split_once("://").unwrap_or(("http", uri));
    let hostport = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match hostport.rsplit_once(':').and_then(|(h, p)| Some((h.to_string(), p.parse::<u16>().ok()?))) {
        Some(hp) => hp,
        None => (hostport.to_string(), if scheme == "https" { 443 } else { 80 }),
    };
    UrlParts { scheme: scheme.to_string(), host, port }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_uri_parts() {
        let u = url_parts("https://10-0-0-5.abc.plex.direct:32400");
        assert_eq!((u.scheme.as_str(), u.host.as_str(), u.port), ("https", "10-0-0-5.abc.plex.direct", 32400));
        let u = url_parts("http://127.0.0.1:32500/");
        assert_eq!((u.host.as_str(), u.port), ("127.0.0.1", 32500));
        let u = url_parts("https://plex.example.com");
        assert_eq!(u.port, 443);
    }
}
