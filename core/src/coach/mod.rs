//! Live coaching: the user changes something (mic distance, angle, gain knob,
//! a lamp, the camera height…) while we measure continuously, show guidance
//! that updates several times a second, and confirm success ourselves — no
//! "did you do it?" questions.
//!
//! Pure computation: the app feeds audio chunks / camera frames and renders
//! the [`CoachUpdate`]s (animated diagrams, meters, "hold steady" rings).
//! Time always comes from sample counts and frame timestamps, never the wall
//! clock, so every behaviour is reproducible in tests.

mod camera;
mod mic;
mod tracker;

#[cfg(test)]
mod tests;

pub use camera::{Arrow, LiveCameraCoach};
pub use mic::{DistanceGoal, LiveMicCoach};
pub use tracker::{STUCK_AFTER_S, Tracker};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What we're coaching right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoachTaskKind {
    // mic
    /// Talking level into the green zone (−18…−12 dBFS peaks).
    SetGain,
    /// Closer/farther vs. how you sounded at the start.
    FindDistance,
    /// Say "pop pop pop" until the bursts stop.
    AngleOffAxis,
    /// Live noise floor while silent.
    QuietRoom,
    /// Type while silent; rotate the mic until typing drops.
    KeyboardNull,
    /// Side-address mic: talk into the logo side.
    TalkIntoLogo,
    // camera
    LightFace,
    LightDirection,
    WhiteBalance,
    CameraHeight,
    Framing,
    Flicker,
    /// Verified by a settings snapshot (Windows / OBS / vendor app).
    CheckSetting,
}

impl CoachTaskKind {
    pub fn is_camera(self) -> bool {
        matches!(
            self,
            CoachTaskKind::LightFace
                | CoachTaskKind::LightDirection
                | CoachTaskKind::WhiteBalance
                | CoachTaskKind::CameraHeight
                | CoachTaskKind::Framing
                | CoachTaskKind::Flicker
        )
    }
    pub fn is_mic(self) -> bool {
        !self.is_camera() && self != CoachTaskKind::CheckSetting
    }
}

/// The zone the main reading should land in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachTarget {
    pub min: f64,
    pub max: f64,
    pub unit: String,
}

impl CoachTarget {
    pub fn new(min: f64, max: f64, unit: &str) -> Self {
        CoachTarget { min, max, unit: unit.to_string() }
    }
    pub fn contains(&self, v: f64) -> bool {
        v >= self.min && v <= self.max
    }
    /// Distance outside the zone (0 inside); positive = above.
    pub fn signed_miss(&self, v: f64) -> f64 {
        if v > self.max {
            v - self.max
        } else if v < self.min {
            v - self.min
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// We need input first (start talking / be quiet / sit in frame).
    Waiting,
    /// Moving toward the target.
    Working,
    /// Close to the target.
    Almost,
    /// In the zone: hold steady while the ring fills.
    Hold,
    Done,
    /// No progress for a while: show the alternative explanation and offer "I can't".
    Stuck,
}

/// Which way the animation should point. Camera directions are in the
/// preview's screen space (as the user sees the preview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintKind {
    Closer,
    Farther,
    TurnUp,
    TurnDown,
    Stop,
    AngleAway,
    FaceLogo,
    MoveLightLeft,
    MoveLightRight,
    MoveLightUp,
    AddLight,
    DimBackground,
    RaiseCamera,
    LowerCamera,
    MoveLeft,
    MoveRight,
    MoveBack,
    MoveForward,
    KeepTalking,
    BeQuiet,
    TypeNow,
    SayPops,
    Wait,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    pub kind: HintKind,
    /// 0..1: how far to go (drives animation size/speed).
    pub amount: f64,
    pub text: String,
}

impl Hint {
    pub fn new(kind: HintKind, amount: f64, text: impl Into<String>) -> Self {
        Hint { kind, amount: amount.clamp(0.0, 1.0), text: text.into() }
    }
    pub fn none() -> Self {
        Hint::new(HintKind::None, 0.0, "")
    }
}

/// One live coaching frame for the UI (~8 per second).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoachUpdate {
    pub task: CoachTaskKind,
    pub phase: Phase,
    /// 0..1 toward done (rings, bars).
    pub progress: f64,
    /// Main reading in `target.unit`.
    pub value: Option<f64>,
    pub target: Option<CoachTarget>,
    /// Big friendly line, e.g. "A little closer…".
    pub headline: String,
    /// Why / how, one sentence.
    pub detail: String,
    pub hint: Hint,
    pub hold_s: f64,
    pub hold_needed_s: f64,
    /// Numbers for the Advanced drawer.
    pub readings: BTreeMap<String, f64>,
    /// The app itself is moving the setting.
    pub auto_adjusting: bool,
    /// Shown when `phase == Stuck`.
    pub stuck_alternative: Option<String>,
    /// Seconds since coaching started (from samples / frame timestamps).
    pub t_s: f64,
}

impl CoachUpdate {
    /// A plain update (used for `CheckSetting` steps driven by the app).
    pub fn simple(task: CoachTaskKind, phase: Phase, headline: &str, detail: &str) -> Self {
        CoachUpdate {
            task,
            phase,
            progress: if phase == Phase::Done { 1.0 } else { 0.0 },
            value: None,
            target: None,
            headline: headline.to_string(),
            detail: detail.to_string(),
            hint: Hint::none(),
            hold_s: 0.0,
            hold_needed_s: 0.0,
            readings: BTreeMap::new(),
            auto_adjusting: false,
            stuck_alternative: None,
            t_s: 0.0,
        }
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}
