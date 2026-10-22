//! Diagnostic: `tls_probe [url]` performs one HTTPS GET with the app's HTTP stack
//! (used to verify rustls/ring and the bundled root store work on a given machine/Wine).
fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "https://plex.tv/api/v2/ping".into());
    let http = plexify::plex::Http::new(plexify::plex::ClientInfo::new("tls-probe")).without_retries();
    match http.request(plexify::plex::http::Method::Get, &url, None, &[]) {
        Ok(body) => println!("OK {} bytes: {}", body.len(), String::from_utf8_lossy(&body[..body.len().min(120)])),
        Err(e) => println!("ERR {e}"),
    }
}
