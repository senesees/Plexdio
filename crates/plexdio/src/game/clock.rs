//! The game's song clock: the position being *heard*, smooth enough to animate at 60 fps.
//!
//! The engine publishes the audible position (frames played minus what still waits in the
//! output buffer) about every 10 ms, in steps, and each report is already a little old when it
//! arrives. The clock estimates the position now (the last report plus the time since it came)
//! and follows that by running slightly fast or slow, at most [`SLEW`]. It never steps
//! backwards while playing: stepping towards each report made the road judder back and forth. A
//! big difference (a seek, a stall) still snaps.

/// Larger differences than this (seconds) are a jump, not drift.
const SNAP: f64 = 0.25;
/// Share of the remaining difference corrected per frame…
const FOLLOW: f64 = 0.1;
/// …but never faster than this share of the frame's own time (10 % fast or slow).
const SLEW: f64 = 0.1;
/// A report is not trusted to keep running for longer than this without a new one.
const MAX_AGE: f64 = 0.1;

#[derive(Debug, Clone, Default)]
pub struct SongClock {
    t: f64,
    started: bool,
    last_report: u64,
    /// Time since `last_report` changed.
    report_age: f64,
}

impl SongClock {
    /// Advance by `dt` seconds of frame time and take in the engine's latest position report.
    /// Returns the song time in seconds.
    pub fn update(&mut self, report_ms: u64, playing: bool, dt: f64) -> f64 {
        let reported = report_ms as f64 / 1000.0;
        if !self.started {
            self.started = true;
            self.t = reported;
            self.last_report = report_ms;
            self.report_age = 0.0;
            return self.t;
        }
        if report_ms != self.last_report {
            self.last_report = report_ms;
            self.report_age = 0.0;
        } else if playing {
            self.report_age = (self.report_age + dt).min(MAX_AGE);
        }
        if !playing {
            // Paused: settle on the reported position (seeks while paused still move it).
            let err = reported - self.t;
            self.t = if err.abs() > SNAP { reported } else { self.t + err * 0.3 };
            return self.t;
        }
        let estimate = reported + self.report_age;
        let predicted = self.t + dt;
        let err = estimate - predicted;
        self.t = if err.abs() > SNAP { estimate } else { predicted + (err * FOLLOW).clamp(-SLEW * dt, SLEW * dt) };
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
    fn never_steps_back_with_late_and_noisy_reports() {
        let mut c = SongClock::default();
        c.update(0, true, 0.0);
        let mut prev = 0.0;
        let mut noise = 0x1234_5678u32;
        let mut steps = Vec::new();
        for f in 1..=600 {
            let wall = f as f64 / 60.0;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            // Reports every ~10 ms, up to 25 ms late, with ±8 ms of buffer jitter.
            let jitter = (noise % 17) as f64 / 1000.0 - 0.008;
            let lag = (noise >> 8) as f64 % 25.0 / 1000.0;
            let report = (((wall - lag + jitter).max(0.0) * 1000.0) as u64 / 10) * 10;
            // Frames arrive a little unevenly too.
            let dt = 1.0 / 60.0 + if f % 7 == 0 { 0.004 } else { -0.0007 };
            let t = c.update(report, true, dt);
            assert!(t >= prev, "frame {f}: went back from {prev} to {t}");
            steps.push(t - prev);
            prev = t;
        }
        // Each frame advances close to a frame's worth (no judder).
        assert!(steps[60..].iter().all(|s| (0.012..0.025).contains(s)), "{:?}", &steps[60..80]);
        assert!((prev - 10.0).abs() < 0.06, "drifted: {prev}");
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
        // A seek while paused moves the clock too.
        assert!((c.update(20_000, false, 0.5) - 20.0).abs() < 1e-9);
    }
}
