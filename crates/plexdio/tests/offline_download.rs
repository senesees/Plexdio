//! Downloads for offline listening against the mock server: a full download, a resumed one, and
//! the recorded file in the library database.

use mockplex::{MockConfig, MockServer, MUSIC_SECTION, USER_TOKEN};
use plexdio::cache::db::LibraryDb;
use plexdio::cache::offline::{wanted_tracks, Downloader, Event, Job, PinKind};
use plexdio::cache::sync::sync_library;
use plexdio::library_model::LibraryModel;
use plexdio::plex::auth::PlexTv;
use plexdio::plex::resources::pick_best;
use plexdio::plex::server::PmsClient;
use plexdio::plex::{ClientInfo, Http};
use std::sync::mpsc;
use std::time::Duration;

fn http() -> Http {
    Http::new(ClientInfo::new("test-client")).with_backoff(Duration::from_millis(5))
}

struct Setup {
    _mock: MockServer,
    pms: PmsClient,
    dir: std::path::PathBuf,
    db_path: std::path::PathBuf,
    lib: LibraryModel,
}

fn setup(name: &str) -> Setup {
    let mock = MockServer::start(MockConfig::default()).unwrap();
    let tv = PlexTv::new(http(), Some(&mock.url));
    let servers = tv.servers(USER_TOKEN).unwrap();
    let conn = pick_best(&http().info, &servers[0], USER_TOKEN, Duration::from_secs(2)).unwrap();
    let pms = PmsClient::new(http(), conn);
    let root = std::env::temp_dir().join(format!("plexdio-dl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("lib.sqlite");
    let mut db = LibraryDb::open(&db_path).unwrap();
    sync_library(&pms, &mut db, MUSIC_SECTION, true, &mut |_| {}).unwrap();
    let lib = LibraryModel::load(&db).unwrap();
    Setup { _mock: mock, pms, dir: root.join("offline"), db_path, lib }
}

/// Download `track` and wait for the worker's verdict.
fn download(s: &Setup, track: i64) -> Event {
    let t = s.lib.track(track).unwrap();
    let ext = t.container.as_deref().unwrap_or("bin").to_ascii_lowercase();
    let (tx, rx) = mpsc::channel();
    let d = Downloader::start(&http(), s.dir.clone(), s.db_path.clone(), move |ev| {
        if !matches!(ev, Event::Progress { .. }) {
            let _ = tx.send(ev);
        }
    });
    d.queue(Job { track, url: s.pms.part_url(t.part_key.as_deref().unwrap()), ext });
    rx.recv_timeout(Duration::from_secs(20)).expect("download finished")
}

#[test]
fn downloads_record_the_file_and_resume_partial_ones() {
    let s = setup("full");
    // The first song of an album pin.
    let album = s.lib.albums.iter().find(|a| &*a.title == "Night Signals").unwrap().id;
    let wanted = wanted_tracks(&s.lib, &[(PinKind::Album, album)]);
    assert_eq!(wanted, s.lib.album_track_ids(album));
    let track = wanted[0];

    let Event::Finished { file, bytes, .. } = download(&s, track) else { panic!("download failed") };
    let full = std::fs::read(s.dir.join(&file)).unwrap();
    assert_eq!(full.len() as u64, bytes);
    assert!(!s.dir.join(format!("{file}.part")).exists(), "part file renamed");
    let db = LibraryDb::open(&s.db_path).unwrap();
    assert_eq!(db.offline_files().unwrap()[&track], (file.clone(), bytes));

    // A partial file is continued with a Range request and ends up identical.
    std::fs::remove_file(s.dir.join(&file)).unwrap();
    std::fs::write(s.dir.join(format!("{file}.part")), &full[..full.len() / 3]).unwrap();
    let Event::Finished { file: again, .. } = download(&s, track) else { panic!("resume failed") };
    assert_eq!(std::fs::read(s.dir.join(again)).unwrap(), full);
    let _ = std::fs::remove_dir_all(s.dir.parent().unwrap());
}

#[test]
fn a_missing_file_fails_cleanly() {
    let s = setup("missing");
    let (tx, rx) = mpsc::channel();
    let d = Downloader::start(&http(), s.dir.clone(), s.db_path.clone(), move |ev| {
        let _ = tx.send(ev);
    });
    d.queue(Job { track: 1, url: s.pms.part_url("/library/parts/1/1/nope.flac"), ext: "flac".into() });
    match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
        Event::Failed { track, .. } => assert_eq!(track, 1),
        other => panic!("expected a failure, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(s.dir.parent().unwrap());
}
