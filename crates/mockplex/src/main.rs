//! `mockplex [--port N] [--bind ADDR] [--advertise HOST] [--synthetic N] [--no-sonic]
//!           [--manual-approve] [--quiet]`
//!
//! Then start Plexify with `--plex-tv-url http://HOST:PORT`.

use mockplex::{MockConfig, MockServer};

fn main() {
    let mut cfg = MockConfig { port: 32500, verbose: true, ..Default::default() };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => cfg.port = args.next().and_then(|v| v.parse().ok()).expect("--port N"),
            "--bind" => cfg.bind = args.next().expect("--bind ADDR"),
            "--advertise" => cfg.advertise_host = args.next().expect("--advertise HOST"),
            "--synthetic" => cfg.synthetic_tracks = args.next().and_then(|v| v.parse().ok()),
            "--no-sonic" => cfg.sonic = false,
            "--manual-approve" => cfg.auto_approve_after = None,
            "--quiet" => cfg.verbose = false,
            "-h" | "--help" => {
                println!("mockplex [--port N] [--bind ADDR] [--advertise HOST] [--synthetic N] [--no-sonic] [--manual-approve] [--quiet]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let server = MockServer::start(cfg).expect("failed to start mock server");
    println!("mockplex listening on {}  (user token: {}, server token: {})", server.url, mockplex::USER_TOKEN, mockplex::SERVER_TOKEN);
    loop {
        std::thread::park();
    }
}
