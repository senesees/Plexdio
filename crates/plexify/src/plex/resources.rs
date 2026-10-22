//! Server connection selection: probe every advertised connection in parallel and pick the
//! best working one (local > remote > relay, https before http, then lowest latency).

use super::http::{ClientInfo, Http};
use super::models::{Connection, Resource};
use super::{PlexError, Result};
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A chosen, working connection to a media server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerConnection {
    pub server_id: String,
    pub server_name: String,
    pub uri: String,
    /// Token to use with this server (shared servers have their own access token).
    pub token: String,
    pub local: bool,
    pub relay: bool,
    pub latency_ms: u64,
}

/// Lower is better.
pub fn rank(c: &Connection) -> u8 {
    let class = if c.relay {
        2
    } else if c.local {
        0
    } else {
        1
    };
    class * 2 + u8::from(c.protocol.as_deref() != Some("https") && !c.uri.starts_with("https"))
}

/// Order connections best-first without probing.
pub fn ordered(conns: &[Connection]) -> Vec<Connection> {
    let mut v = conns.to_vec();
    v.sort_by_key(rank);
    v
}

/// Probe a single base URI (`GET /identity`). Returns latency.
pub fn probe(http: &Http, uri: &str, token: &str) -> Result<Duration> {
    let start = Instant::now();
    let url = format!("{}/identity", uri.trim_end_matches('/'));
    http.request(super::http::Method::Get, &url, Some(token), &[])?;
    Ok(start.elapsed())
}

/// Probe all connections of `server` in parallel and return the best working one.
/// `user_token` is used when the resource carries no access token.
pub fn pick_best(info: &ClientInfo, server: &Resource, user_token: &str, timeout: Duration) -> Result<ServerConnection> {
    let token = server.access_token.clone().unwrap_or_else(|| user_token.to_string());
    let conns = ordered(&server.connections);
    if conns.is_empty() {
        return Err(PlexError::Network(format!("server '{}' advertises no connections", server.name)));
    }
    let best_possible = conns.iter().map(rank).min().unwrap_or(0);
    let (tx, rx) = mpsc::channel();
    for (i, c) in conns.iter().enumerate() {
        let tx = tx.clone();
        let http = Http::with_timeout(info.clone(), timeout).without_retries();
        let uri = c.uri.clone();
        let token = token.clone();
        let _ = std::thread::Builder::new().name("probe".into()).spawn(move || {
            let res = probe(&http, &uri, &token);
            let _ = tx.send((i, res));
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout + Duration::from_millis(500);
    let mut ok: Vec<(usize, Duration)> = Vec::new();
    let mut last_err = None;
    let mut received = 0;
    while received < conns.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok((i, Ok(lat))) => {
                received += 1;
                log::info!("connection {} ok ({} ms)", conns[i].uri, lat.as_millis());
                ok.push((i, lat));
                // Can't do better than the best class; stop waiting for slower probes.
                if rank(&conns[i]) == best_possible {
                    break;
                }
            }
            Ok((i, Err(e))) => {
                received += 1;
                log::info!("connection {} failed: {e}", conns[i].uri);
                last_err = Some(e);
            }
            Err(_) => break,
        }
    }
    let best = ok
        .into_iter()
        .min_by_key(|(i, lat)| (rank(&conns[*i]), lat.as_millis()))
        .map(|(i, lat)| (conns[i].clone(), lat));
    match best {
        Some((c, lat)) => Ok(ServerConnection {
            server_id: server.client_identifier.clone(),
            server_name: server.name.clone(),
            uri: c.uri.trim_end_matches('/').to_string(),
            token,
            local: c.local,
            relay: c.relay,
            latency_ms: lat.as_millis() as u64,
        }),
        None => Err(match last_err {
            Some(PlexError::Unauthorized) => PlexError::Unauthorized,
            Some(e) if !e.is_transient() => e,
            _ => PlexError::Network(format!("no connection to '{}' is reachable", server.name)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(uri: &str, local: bool, relay: bool) -> Connection {
        Connection {
            protocol: Some(if uri.starts_with("https") { "https" } else { "http" }.into()),
            address: None,
            port: None,
            uri: uri.into(),
            local,
            relay,
            ipv6: false,
        }
    }

    #[test]
    fn ranking_prefers_local_then_remote_then_relay() {
        let v = ordered(&[
            conn("https://relay", false, true),
            conn("https://remote", false, false),
            conn("http://local-http", true, false),
            conn("https://local", true, false),
        ]);
        let uris: Vec<_> = v.iter().map(|c| c.uri.as_str()).collect();
        assert_eq!(uris, ["https://local", "http://local-http", "https://remote", "https://relay"]);
    }
}
