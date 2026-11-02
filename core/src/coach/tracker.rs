//! Progress, "hold steady" and stuck detection shared by every coach.

use super::Phase;

/// No meaningful progress for this long → Stuck (alternative explanation).
pub const STUCK_AFTER_S: f64 = 45.0;

#[derive(Debug, Clone)]
pub struct Tracker {
    pub hold_needed_s: f64,
    hold_s: f64,
    last_t: Option<f64>,
    best: f64,
    last_improve_t: f64,
    done: bool,
    pub stuck_after_s: f64,
}

/// Result of one tracker step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub phase: Phase,
    pub progress: f64,
    pub hold_s: f64,
}

impl Tracker {
    pub fn new(hold_needed_s: f64) -> Self {
        Tracker { hold_needed_s, hold_s: 0.0, last_t: None, best: 0.0, last_improve_t: 0.0, done: false, stuck_after_s: STUCK_AFTER_S }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Seconds held in the zone so far.
    pub fn hold_s(&self) -> f64 {
        self.hold_s
    }

    fn dt(&mut self, t: f64) -> f64 {
        let dt = self.last_t.map(|l| (t - l).max(0.0)).unwrap_or(0.0);
        self.last_t = Some(t);
        dt
    }

    fn stuck(&self, t: f64) -> bool {
        !self.done && t - self.last_improve_t > self.stuck_after_s
    }

    /// We have a measurement: `in_zone` says whether it's on target and
    /// `raw` (0..1) how close it is otherwise.
    pub fn measure(&mut self, t: f64, in_zone: bool, raw: f64) -> Step {
        let dt = self.dt(t);
        if self.done {
            return Step { phase: Phase::Done, progress: 1.0, hold_s: self.hold_s };
        }
        if in_zone {
            self.hold_s += dt;
        } else {
            self.hold_s = 0.0;
        }
        if in_zone && self.hold_s + 1e-9 >= self.hold_needed_s {
            self.done = true;
            return Step { phase: Phase::Done, progress: 1.0, hold_s: self.hold_s };
        }
        let progress = if in_zone {
            0.8 + 0.2 * (self.hold_s / self.hold_needed_s.max(1e-9)).min(1.0)
        } else {
            raw.clamp(0.0, 0.79)
        };
        if progress > self.best + 0.03 {
            self.best = progress;
            self.last_improve_t = t;
        }
        let phase = if in_zone {
            Phase::Hold
        } else if self.stuck(t) {
            Phase::Stuck
        } else if progress >= 0.6 {
            Phase::Almost
        } else {
            Phase::Working
        };
        Step { phase, progress, hold_s: self.hold_s }
    }

    /// No usable measurement right now (not talking, no face…). Holding is
    /// interrupted only by a measurement outside the zone, not by a pause.
    pub fn waiting(&mut self, t: f64) -> Step {
        self.dt(t);
        if self.done {
            return Step { phase: Phase::Done, progress: 1.0, hold_s: self.hold_s };
        }
        let phase = if self.stuck(t) { Phase::Stuck } else { Phase::Waiting };
        Step { phase, progress: self.best.min(0.79), hold_s: self.hold_s }
    }

    /// Mark done directly (e.g. three clean "pop"s in a row).
    pub fn finish(&mut self) {
        self.done = true;
    }

    /// Forget hold progress (after the app changed a setting itself).
    pub fn reset_hold(&mut self) {
        self.hold_s = 0.0;
    }
}
