//! The game's song clock: the position being *heard*, smooth enough to animate at 60 fps.
//!
//! The engine publishes the audible position (frames played minus what still waits in the
//! output buffer) about every 10 ms, in steps. The clock runs on frame time between reports and
//! eases towards each report, so notes neither stutter nor drift; a big difference (a seek, a
//! stall) snaps.

/// Larger differences than this (seconds) are a jump, not drift.
const SNAP: f64 = 0.25;
/// How much of the difference to a new report is taken per frame.
const FOLLOW: f64 = 0.15;

#[derive(Debug, Clone, Default)]
pub struct SongClock {
    t: f64,
    started: bool,
    last_report: u64,
}

impl SongClock {
    /// Advance by `dt` seconds of frame time and take in the engine's latest position report.
    /// Returns the song time in seconds.
    pub fn update(&mut self, report_ms: u64, playing: bool, dt: f64) -> f64 {
        let reported = report_ms as f64 / 1000.0;
        if !self.started {
            self.started = true;
            self.t = reported;
        } else {
            if playing {
                self.t += dt;
            }
            let err = reported - self.t;
            if err.abs() > SNAP {
                self.t = reported;
            } else if report_ms != self.last_report || !playing {
                self.t += err * FOLLOW;
            }
        }
        self.last_report = report_ms;
        self.t
    }

    pub fn now(&self) -> f64 {
        self.t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_smoothly_between_stepped_reports() {
        let mut c = SongClock::default();
        c.update(10_000, true, 0.0);
        // 60 fps frames; the engine reports in 20 ms steps.
        let mut prev = c.now();
        for f in 1..=120 {
            let wall = f as f64 / 60.0;
            let report = 10_000 + ((wall * 1000.0) as u64 / 20) * 20;
            let t = c.update(report, true, 1.0 / 60.0);
            assert!(t > prev, "time must keep moving");
            assert!((t - (10.0 + wall)).abs() < 0.03, "frame {f}: {t} vs {}", 10.0 + wall);
            prev = t;
        }
    }

    #[test]
    fn snaps_on_seeks_and_holds_while_paused() {
        let mut c = SongClock::default();
        c.update(5_000, true, 0.0);
        c.update(5_016, true, 0.016);
        assert!((c.update(60_000, true, 0.016) - 60.0).abs() < 1e-9);
        let paused = c.update(60_000, false, 0.5);
        assert!((paused - 60.0).abs() < 0.01);
        assert!((c.update(60_000, false, 0.5) - 60.0).abs() < 0.01);
    }
}
