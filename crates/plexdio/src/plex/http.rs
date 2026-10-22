//! Thin blocking HTTP layer over ureq: X-Plex-* headers, JSON decoding, error mapping and
//! retry with exponential backoff.

use super::{PlexError, Result};
use serde::de::DeserializeOwned;
use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;
use ureq::tls::TlsConfig;
use ureq::Agent;

/// Identity sent with every request (the X-Plex-* headers).
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub client_id: String,
    pub product: String,
    pub version: String,
    pub platform: String,
    pub device_name: String,
}

impl ClientInfo {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            product: "PlexDio".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: "Windows".into(),
            device_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "PlexDio PC".into()),
        }
    }

    pub fn headers(&self) -> [(&'static str, &str); 6] {
        [
            ("X-Plex-Client-Identifier", &self.client_id),
            ("X-Plex-Product", &self.product),
            ("X-Plex-Version", &self.version),
            ("X-Plex-Platform", &self.platform),
            ("X-Plex-Device-Name", &self.device_name),
            ("X-Plex-Device", "PC"),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

/// Shared HTTP client. Cheap to clone (agents are reference counted).
#[derive(Clone)]
pub struct Http {
    pub info: ClientInfo,
    strict: Agent,
    /// Only used for literal private/loopback IP hosts over https, where a Plex server presents a
    /// certificate for its *.plex.direct name rather than the IP (Plex's "allow insecure local").
    lan: Agent,
    retries: u32,
    backoff: Duration,
}

fn build_agent(timeout: Duration, insecure: bool) -> Agent {
    let tls = TlsConfig::builder().disable_verification(insecure).build();
    Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(tls)
        .user_agent(concat!("PlexDio/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Is `url`'s host a literal private, link-local or loopback IP address?
pub fn is_lan_ip_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = if let Some(h) = host.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => false,
    }
}

impl Http {
    pub fn new(info: ClientInfo) -> Self {
        Self::with_timeout(info, Duration::from_secs(30))
    }

    pub fn with_timeout(info: ClientInfo, timeout: Duration) -> Self {
        Self {
            info,
            strict: build_agent(timeout, false),
            lan: build_agent(timeout, true),
            retries: 3,
            backoff: Duration::from_millis(400),
        }
    }

    /// Disable retries (used for connection probing and tests).
    pub fn without_retries(mut self) -> Self {
        self.retries = 0;
        self
    }

    pub fn with_backoff(mut self, base: Duration) -> Self {
        self.backoff = base;
        self
    }

    fn agent_for(&self, url: &str) -> &Agent {
        if url.starts_with("https://") && is_lan_ip_url(url) {
            &self.lan
        } else {
            &self.strict
        }
    }

    /// Perform a request once. Returns the response body bytes for 2xx responses.
    fn request_once(&self, method: Method, url: &str, token: Option<&str>, query: &[(&str, &str)]) -> Result<Vec<u8>> {
        let agent = self.agent_for(url);
        macro_rules! decorate {
            ($req:expr) => {{
                let mut req = $req.header("Accept", "application/json");
                for (k, v) in self.info.headers() {
                    req = req.header(k, v);
                }
                if let Some(t) = token {
                    req = req.header("X-Plex-Token", t);
                }
                for (k, v) in query {
                    req = req.query(*k, *v);
                }
                req
            }};
        }
        let res = match method {
            Method::Get => decorate!(agent.get(url)).call(),
            Method::Delete => decorate!(agent.delete(url)).call(),
            Method::Post => decorate!(agent.post(url)).send_empty(),
            Method::Put => decorate!(agent.put(url)).send_empty(),
        };
        let resp = res.map_err(map_ureq_error)?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {
                let mut body = Vec::new();
                resp.into_body()
                    .into_with_config()
                    .limit(256 * 1024 * 1024)
                    .reader()
                    .read_to_end(&mut body)
                    .map_err(|e| PlexError::Network(e.to_string()))?;
                Ok(body)
            }
            401 | 403 => Err(PlexError::Unauthorized),
            404 => Err(PlexError::NotFound),
            s => Err(PlexError::Http(s)),
        }
    }

    /// Request with retry/backoff on transient failures.
    pub fn request(&self, method: Method, url: &str, token: Option<&str>, query: &[(&str, &str)]) -> Result<Vec<u8>> {
        let mut attempt = 0;
        loop {
            match self.request_once(method, url, token, query) {
                Ok(b) => return Ok(b),
                Err(e) if e.is_transient() && attempt < self.retries => {
                    let delay = self.backoff * 2u32.pow(attempt) + jitter(self.backoff / 2);
                    log::warn!("{method:?} {} failed ({e}); retry {} in {delay:?}", redact(url), attempt + 1);
                    std::thread::sleep(delay);
                    attempt += 1;
                }
                Err(e) => {
                    log::debug!("{method:?} {} -> {e}", redact(url));
                    return Err(e);
                }
            }
        }
    }

    pub fn get_json<T: DeserializeOwned>(&self, url: &str, token: Option<&str>, query: &[(&str, &str)]) -> Result<T> {
        let body = self.request(Method::Get, url, token, query)?;
        parse_json(&body)
    }

    pub fn post_json<T: DeserializeOwned>(&self, url: &str, token: Option<&str>, query: &[(&str, &str)]) -> Result<T> {
        let body = self.request(Method::Post, url, token, query)?;
        parse_json(&body)
    }

    /// Fire a request whose response body we don't need.
    pub fn send(&self, method: Method, url: &str, token: Option<&str>, query: &[(&str, &str)]) -> Result<()> {
        self.request(method, url, token, query).map(|_| ())
    }

    /// Raw GET with an optional byte range; returns (status, content-length, reader).
    /// Used for audio streaming and image downloads.
    pub fn get_stream(&self, url: &str, range_start: Option<u64>) -> Result<(u16, Option<u64>, Box<dyn Read + Send>)> {
        let agent = self.agent_for(url);
        let mut req = agent.get(url);
        for (k, v) in self.info.headers() {
            req = req.header(k, v);
        }
        if let Some(start) = range_start.filter(|s| *s > 0) {
            req = req.header("Range", format!("bytes={start}-"));
        }
        let resp = req.call().map_err(map_ureq_error)?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {
                let len = resp
                    .headers()
                    .get("content-length")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok());
                let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
                Ok((status, len, Box::new(reader)))
            }
            401 | 403 => Err(PlexError::Unauthorized),
            404 => Err(PlexError::NotFound),
            s => Err(PlexError::Http(s)),
        }
    }
}

pub fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|e| PlexError::Parse(e.to_string()))
}

fn map_ureq_error(e: ureq::Error) -> PlexError {
    match e {
        ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => PlexError::Unauthorized,
        ureq::Error::StatusCode(404) => PlexError::NotFound,
        ureq::Error::StatusCode(s) => PlexError::Http(s),
        ureq::Error::Timeout(_) => PlexError::Timeout,
        other => PlexError::Network(other.to_string()),
    }
}

fn jitter(max: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    let max_ms = max.as_millis().max(1) as u64;
    Duration::from_millis(nanos % max_ms)
}

/// Strip tokens from URLs before logging.
pub fn redact(url: &str) -> String {
    match url.find("X-Plex-Token=") {
        Some(i) => {
            let end = url[i..].find('&').map_or(url.len(), |j| i + j);
            format!("{}X-Plex-Token=***{}", &url[..i], &url[end..])
        }
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_ip_detection() {
        assert!(is_lan_ip_url("https://192.168.1.20:32400/identity"));
        assert!(is_lan_ip_url("https://10.0.0.5:32400"));
        assert!(is_lan_ip_url("http://127.0.0.1:32400/x?y=1"));
        assert!(is_lan_ip_url("https://[fe80::1]:32400/"));
        assert!(!is_lan_ip_url("https://8.8.8.8:32400/"));
        assert!(!is_lan_ip_url("https://192-168-1-20.abcdef.plex.direct:32400/"));
        assert!(!is_lan_ip_url("https://plex.tv/api/v2/user"));
    }

    #[test]
    fn redacts_tokens() {
        assert_eq!(redact("http://h/a?X-Plex-Token=secret&b=1"), "http://h/a?X-Plex-Token=***&b=1");
        assert_eq!(redact("http://h/a?b=1&X-Plex-Token=secret"), "http://h/a?b=1&X-Plex-Token=***");
    }

    #[test]
    fn transient_classification() {
        assert!(PlexError::Http(503).is_transient());
        assert!(PlexError::Http(429).is_transient());
        assert!(!PlexError::Http(400).is_transient());
        assert!(!PlexError::Unauthorized.is_transient());
        assert!(PlexError::Network("x".into()).is_transient());
    }
}
