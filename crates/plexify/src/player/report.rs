//! Playback reporting: `/:/timeline` while playing and `/:/scrobble` once a track counts as
//! played. Requests run on a dedicated thread so the UI and the audio thread never block on the
//! network.

use crate::plex::server::{PlayState, PmsClient};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

/// Report a track as played once it reached this fraction of its duration (Plex uses 90%).
const SCROBBLE_FRACTION: f64 = 0.9;
/// How often the position is reported while playing.
const TICK: Duration = Duration::from_secs(10);

/// One server call decided by [`ReportState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Timeline { id: i64, state: PlayState, time_ms: u64, duration_ms: u64 },
    Scrobble(i64),
}

#[derive(Clone)]
pub struct Reporter {
    tx: Sender<Action>,
}

impl Reporter {
    pub fn start(pms: PmsClient) -> Self {
        let (tx, rx) = mpsc::channel::<Action>();
        let spawned = std::thread::Builder::new().name("play-report".into()).spawn(move || {
            while let Ok(a) = rx.recv() {
                let res = match a {
                    Action::Timeline { id, state, time_ms, duration_ms } => {
                        pms.timeline(&id.to_string(), state, time_ms, duration_ms)
                    }
                    Action::Scrobble(id) => pms.scrobble(&id.to_string()),
                };
                if let Err(e) = res {
                    log::debug!("playback report failed: {e}");
                }
            }
        });
        if spawned.is_err() {
            log::warn!("cannot start the reporting thread");
        }
        Self { tx }
    }

    pub fn apply(&self, actions: &[Action]) {
        for a in actions {
            let _ = self.tx.send(a.clone());
        }
    }
}

/// Turns the engine's status stream into timeline and scrobble calls.
#[derive(Default)]
pub struct ReportState {
    track: Option<i64>,
    duration_ms: u64,
    state: Option<PlayState>,
    last_tick: Option<Instant>,
    /// Last position seen for the current track (the new status no longer carries it once the
    /// engine moves on).
    last_position: u64,
    scrobbled: bool,
    /// A restored queue starts paused; don't score it as a play until it actually advances.
    started: bool,
}

impl ReportState {
    /// Feed the latest status. Returns the calls to make (empty most frames).
    pub fn observe(
        &mut self,
        track: Option<i64>,
        state: PlayState,
        position_ms: u64,
        duration_ms: u64,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        if self.track != track {
            if let Some(prev) = self.track {
                // Leaving a track: it counts as played if it got far enough.
                let at = self.last_position;
                if self.started && !self.scrobbled && self.was_played(at) {
                    out.push(Action::Scrobble(prev));
                }
                out.push(Action::Timeline { id: prev, state: PlayState::Stopped, time_ms: at, duration_ms: self.duration_ms });
            }
            self.track = track;
            self.duration_ms = duration_ms;
            self.scrobbled = false;
            self.started = state == PlayState::Playing;
            self.state = track.map(|_| state);
            self.last_tick = track.map(|_| Instant::now());
            if let Some(id) = track {
                out.push(Action::Timeline { id, state, time_ms: position_ms, duration_ms });
            }
            self.last_position = position_ms;
            return out;
        }
        let Some(id) = track else { return out };
        self.last_position = position_ms;
        let playing = state == PlayState::Playing;
        if playing {
            self.started = true;
        }
        if self.state != Some(state) {
            self.state = Some(state);
            self.last_tick = Some(Instant::now());
            out.push(Action::Timeline { id, state, time_ms: position_ms, duration_ms });
        } else if playing && self.last_tick.is_none_or(|t| t.elapsed() >= TICK) {
            self.last_tick = Some(Instant::now());
            out.push(Action::Timeline { id, state, time_ms: position_ms, duration_ms });
        }
        if playing && !self.scrobbled && self.was_played(position_ms) {
            self.scrobbled = true;
            out.push(Action::Scrobble(id));
        }
        out
    }

    /// True once the track has passed the scrobble threshold.
    pub fn was_played(&self, position_ms: u64) -> bool {
        self.duration_ms > 0 && position_ms as f64 >= self.duration_ms as f64 * SCROBBLE_FRACTION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_start_pause_and_stop() {
        let mut st = ReportState::default();
        let a = st.observe(Some(1), PlayState::Playing, 0, 10_000);
        assert_eq!(a.len(), 1);
        assert!(matches!(&a[0], Action::Timeline { id: 1, state: PlayState::Playing, .. }));

        // Same state and no tick yet: nothing.
        assert!(st.observe(Some(1), PlayState::Playing, 2_000, 10_000).is_empty());

        let a = st.observe(Some(1), PlayState::Paused, 2_000, 10_000);
        assert!(matches!(&a[..], [Action::Timeline { state: PlayState::Paused, .. }]));

        // Switching tracks reports the old one stopped, then the new one playing.
        let a = st.observe(Some(2), PlayState::Playing, 0, 5_000);
        assert!(matches!(&a[..], [
            Action::Timeline { id: 1, state: PlayState::Stopped, .. },
            Action::Timeline { id: 2, state: PlayState::Playing, .. }
        ]));
    }

    #[test]
    fn scrobbles_once_at_ninety_percent() {
        let mut st = ReportState::default();
        st.observe(Some(7), PlayState::Playing, 0, 10_000);
        assert!(st.observe(Some(7), PlayState::Playing, 8_000, 10_000).iter().all(|a| !matches!(a, Action::Scrobble(_))));
        let a = st.observe(Some(7), PlayState::Playing, 9_000, 10_000);
        assert!(a.iter().any(|a| matches!(a, Action::Scrobble(7))), "scrobbled at 90%");
        assert!(st.observe(Some(7), PlayState::Playing, 9_500, 10_000).iter().all(|a| !matches!(a, Action::Scrobble(_))), "only once");
    }

    #[test]
    fn a_restored_track_is_not_scrobbled_before_it_plays() {
        let mut st = ReportState::default();
        // Startup restore reports a paused track near its end.
        st.observe(Some(3), PlayState::Paused, 9_500, 10_000);
        // Skipping it should not count as a play.
        let a = st.observe(Some(4), PlayState::Playing, 0, 10_000);
        assert!(a.iter().all(|a| !matches!(a, Action::Scrobble(_))));
    }

    #[test]
    fn a_finished_track_is_scrobbled_before_the_next_one_starts() {
        let mut st = ReportState::default();
        st.observe(Some(5), PlayState::Playing, 0, 10_000);
        let a = st.observe(Some(5), PlayState::Playing, 9_200, 10_000);
        assert!(a.iter().any(|a| matches!(a, Action::Scrobble(5))), "scrobbled at the threshold: {a:?}");
        let a = st.observe(Some(6), PlayState::Playing, 0, 10_000);
        assert_eq!(a[0], Action::Timeline { id: 5, state: PlayState::Stopped, time_ms: 9_200, duration_ms: 10_000 });
        assert!(a.iter().all(|a| !matches!(a, Action::Scrobble(_))), "not scrobbled twice");
    }
}
