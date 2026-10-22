//! plex.tv: PIN sign-in, account validation and resource (server) discovery.

use super::http::Http;
use super::models::{Pin, PlexUser, Resource};
use super::{PlexError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const PLEX_TV: &str = "https://plex.tv";
pub const PLEX_APP: &str = "https://app.plex.tv";

#[derive(Clone)]
pub struct PlexTv {
    http: Http,
    base: String,
    /// Where the user is sent to approve the PIN. The real plex.tv flow uses app.plex.tv,
    /// the mock serves its own `/auth` page.
    auth_base: String,
}

impl PlexTv {
    /// `base_override` points everything at a mock server (`--plex-tv-url`).
    pub fn new(http: Http, base_override: Option<&str>) -> Self {
        match base_override.map(|s| s.trim_end_matches('/')).filter(|s| !s.is_empty()) {
            Some(base) => Self { http, base: base.to_string(), auth_base: base.to_string() },
            None => Self { http, base: PLEX_TV.into(), auth_base: PLEX_APP.into() },
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Step 1: request a new strong PIN.
    pub fn create_pin(&self) -> Result<Pin> {
        let url = format!("{}/api/v2/pins", self.base);
        self.http.post_json(&url, None, &[("strong", "true")])
    }

    /// Step 2: URL to open in the browser so the user can approve the PIN.
    pub fn auth_url(&self, pin: &Pin) -> String {
        let info = &self.http.info;
        let enc = |s: &str| urlencoding::encode(s).into_owned();
        format!(
            "{}/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D={}&context%5Bdevice%5D%5Bplatform%5D={}&context%5Bdevice%5D%5BdeviceName%5D={}",
            self.auth_base,
            enc(&info.client_id),
            enc(&pin.code),
            enc(&info.product),
            enc(&info.platform),
            enc(&info.device_name),
        )
    }

    /// Step 3: poll the PIN. `Ok(Some(token))` once approved, `Ok(None)` while pending.
    /// An expired PIN comes back as 404 -> `PlexError::NotFound`.
    pub fn check_pin(&self, pin: &Pin) -> Result<Option<String>> {
        let url = format!("{}/api/v2/pins/{}", self.base, pin.id);
        let p: Pin = self.http.get_json(&url, None, &[("code", &pin.code)])?;
        Ok(p.auth_token.filter(|t| !t.is_empty()))
    }

    /// Poll until the PIN is approved, expires (`timeout`), or `cancel` is set.
    pub fn wait_for_pin(&self, pin: &Pin, interval: Duration, timeout: Duration, cancel: &AtomicBool) -> Result<String> {
        let start = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(PlexError::Cancelled);
            }
            match self.check_pin(pin) {
                Ok(Some(token)) => return Ok(token),
                Ok(None) => {}
                // Transient failures while polling are not fatal; keep polling.
                Err(e) if e.is_transient() => log::warn!("PIN poll failed: {e}"),
                Err(e) => return Err(e),
            }
            if start.elapsed() >= timeout {
                return Err(PlexError::Timeout);
            }
            // Sleep in small steps so cancellation is responsive.
            let until = Instant::now() + interval;
            while Instant::now() < until {
                if cancel.load(Ordering::Relaxed) {
                    return Err(PlexError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    /// Validate a token and fetch the account. `Unauthorized` means the token is invalid/expired.
    pub fn user(&self, token: &str) -> Result<PlexUser> {
        let url = format!("{}/api/v2/user", self.base);
        self.http.get_json(&url, Some(token), &[])
    }

    /// All resources (servers, players) the account can access, owned and shared.
    pub fn resources(&self, token: &str) -> Result<Vec<Resource>> {
        let url = format!("{}/api/v2/resources", self.base);
        self.http
            .get_json(&url, Some(token), &[("includeHttps", "1"), ("includeRelay", "1"), ("includeIPv6", "1")])
    }

    /// Only media servers.
    pub fn servers(&self, token: &str) -> Result<Vec<Resource>> {
        Ok(self.resources(token)?.into_iter().filter(Resource::is_server).collect())
    }
}
