//! Library sync: full (paged, mark & sweep) or delta (`updatedAt>>=` watermark + count check).

use super::db::{Kind, LibraryDb};
use crate::plex::models::item_type;
use crate::plex::server::PmsClient;
use crate::plex::PlexError;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncReport {
    pub full: bool,
    pub upserted: usize,
    pub deleted: usize,
    pub playlists_refreshed: usize,
    pub swept_kinds: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub stage: &'static str,
    pub done: usize,
    pub total: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Plex(#[from] PlexError),
    #[error("cache database error: {0}")]
    Db(#[from] rusqlite::Error),
}

fn plex_type(kind: Kind) -> u32 {
    match kind {
        Kind::Artist => item_type::ARTIST,
        Kind::Album => item_type::ALBUM,
        Kind::Track => item_type::TRACK,
    }
}

fn stage(kind: Kind) -> &'static str {
    match kind {
        Kind::Artist => "artists",
        Kind::Album => "albums",
        Kind::Track => "tracks",
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Fetch every item of `kind` and replace the cached set (deletions included).
fn full_kind(pms: &PmsClient, db: &mut LibraryDb, section: &str, kind: Kind, progress: &mut dyn FnMut(SyncProgress)) -> Result<(usize, usize), SyncError> {
    db.mark_unseen(kind)?;
    let mut upserted = 0;
    let mut start = 0;
    loop {
        let page = pms.section_items(section, plex_type(kind), start, crate::plex::server::PAGE_SIZE, &[])?;
        let n = page.items.len();
        upserted += db.upsert(kind, &page.items)?;
        start += n;
        progress(SyncProgress { stage: stage(kind), done: start, total: page.total });
        if n < crate::plex::server::PAGE_SIZE || page.total.is_some_and(|t| start >= t) || n == 0 {
            break;
        }
    }
    let deleted = db.sweep_unseen(kind)?;
    Ok((upserted, deleted))
}

/// Sync `section` of `pms` into `db`. `force_full` ignores the watermark.
pub fn sync_library(
    pms: &PmsClient,
    db: &mut LibraryDb,
    section: &str,
    force_full: bool,
    progress: &mut dyn FnMut(SyncProgress),
) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();
    let last_sync = db.meta_i64("last_sync_at");
    let full = force_full || last_sync.is_none();
    report.full = full;
    let started = std::time::Instant::now();

    if full {
        for kind in [Kind::Artist, Kind::Album, Kind::Track] {
            let (u, d) = full_kind(pms, db, section, kind, progress)?;
            report.upserted += u;
            report.deleted += d;
        }
    } else {
        // Delta: everything touched since the newest timestamp we know about. The filter is
        // inclusive, so items sharing the watermark second are re-fetched (upserts are idempotent).
        let wm = db.watermark()?.to_string();
        for kind in [Kind::Artist, Kind::Album, Kind::Track] {
            let items = pms.section_items_all(section, plex_type(kind), &[("updatedAt>>", &wm)], |done, total| {
                progress(SyncProgress { stage: stage(kind), done, total })
            })?;
            report.upserted += db.upsert(kind, &items)?;
            // Deletions don't show up in a delta; detect them by comparing counts.
            let remote = pms.section_count(section, plex_type(kind))?;
            let local = db.count(kind)?;
            if remote != local {
                log::info!("{} count mismatch (server {remote}, cache {local}); full sweep", stage(kind));
                let (u, d) = full_kind(pms, db, section, kind, progress)?;
                report.upserted += u;
                report.deleted += d;
                report.swept_kinds.push(stage(kind));
            }
        }
    }

    // Playlists are few: refresh the list, then items only for changed playlists.
    progress(SyncProgress { stage: "playlists", done: 0, total: None });
    report.playlists_refreshed = sync_playlists(pms, db, &[])?;

    db.set_meta("last_sync_at", &now_secs().to_string())?;
    log::info!(
        "sync {} done in {:?}: {} upserted, {} deleted, {} playlists refreshed",
        if full { "full" } else { "delta" },
        started.elapsed(),
        report.upserted,
        report.deleted,
        report.playlists_refreshed
    );
    Ok(report)
}

/// Refresh the playlist list, then the items of every changed playlist plus those in `force`
/// (a playlist just edited from this app). Returns how many playlists had their items fetched.
pub fn sync_playlists(pms: &PmsClient, db: &mut LibraryDb, force: &[i64]) -> Result<usize, SyncError> {
    let playlists = pms.playlists()?;
    let mut stale = db.upsert_playlists(&playlists)?;
    let known: Vec<i64> = playlists.iter().filter_map(|m| m.rating_key.as_deref()?.parse().ok()).collect();
    for id in force {
        if known.contains(id) && !stale.contains(id) {
            stale.push(*id);
        }
    }
    for id in &stale {
        let items = pms.playlist_items(&id.to_string())?;
        db.set_playlist_items(*id, &items, now_secs())?;
    }
    Ok(stale.len())
}
