//! Audio cache downloads against the in-process mock server.

use mockplex::{MockConfig, MockServer, SERVER_TOKEN};
use plexify::audio::source::{AudioCache, SourceSpec};
use plexify::plex::{ClientInfo, Http};
use std::io::Read;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("plexify-audio-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn http() -> Http {
    Http::new(ClientInfo::new("test-client"))
}

#[test]
fn transcode_stream_downloads_completely() {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let dir = temp_dir("transcode");
    let cache = AudioCache::new(dir.clone(), 1 << 30, true);
    let url = format!(
        "{}/music/:/transcode/universal/start.mp3?path=%2Flibrary%2Fmetadata%2F10012&X-Plex-Token={SERVER_TOKEN}",
        mock.url
    );
    let mut src = cache
        .open(&http(), &SourceSpec::Http { url, key: "10012_t320.mp3".into() })
        .unwrap();
    let mut all = Vec::new();
    src.read_to_end(&mut all).unwrap();
    assert_eq!(all.len(), mockplex::library::AUDIO[0].bytes.len(), "full body, not a truncated chunk");
    assert_eq!(all, mockplex::library::AUDIO[0].bytes);
    drop(src);

    // A second open must be served from the completed cache file without hitting the network.
    let file = dir.join("10012_t320_mp3.audio");
    assert!(file.exists());
    let mut again = cache
        .open(&http(), &SourceSpec::Http { url: "http://127.0.0.1:1/nope".into(), key: "10012_t320.mp3".into() })
        .unwrap();
    let mut copied = Vec::new();
    again.read_to_end(&mut copied).unwrap();
    assert_eq!(copied, all, "cache hit returns the same bytes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn m4a_stream_decodes_while_it_is_still_downloading() {
    use plexify::audio::decode::TrackDecoder;

    let mock = MockServer::start(MockConfig::default()).unwrap();
    let dir = temp_dir("m4a");
    let cache = AudioCache::new(dir.clone(), 1 << 30, true);
    let url = format!("{}/library/parts/10010/1/t03.m4a?X-Plex-Token={SERVER_TOKEN}", mock.url);
    let spec = SourceSpec::Http { url, key: "10010.mp4".into() };

    // Opening an MP4 needs a seek to the end for its index, before the body is complete.
    let first = cache.open(&http(), &spec).unwrap();
    let mut dec = TrackDecoder::open(first, Some("mp4"), 44_100).unwrap();
    let mut frames = 0usize;
    loop {
        let block = dec.next_block(1024).unwrap();
        if block.is_empty() {
            break;
        }
        frames += block.len() / 2;
    }
    let seconds = frames as f64 / 44_100.0;
    assert!((7.5..8.5).contains(&seconds), "decoded {seconds:.2} s of the 8 s fixture");

    // A second reader (a prefetch of the track already playing) must not truncate the cache
    // file or cancel the transfer.
    let again = cache.open(&http(), &spec).unwrap();
    let mut second = TrackDecoder::open(again, Some("mp4"), 44_100).unwrap();
    let block = second.next_block(4096).unwrap();
    assert!(block.len() > 1000, "second reader decodes audio");
    let _ = std::fs::remove_dir_all(&dir);
}
