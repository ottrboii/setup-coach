//! Target checks for the report card: ✅ met / ⚠️ close / ❌ missed.

use super::analyze::AudioMetrics;
use crate::SetupProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Met,
    Close,
    Missed,
    NotMeasured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetCheck {
    pub id: String,
    pub label: String,
    pub value: Option<f64>,
    pub target: String,
    pub status: Status,
}

fn check(id: &str, label: &str, value: Option<f64>, target: &str, met: impl Fn(f64) -> bool, close: impl Fn(f64) -> bool) -> TargetCheck {
    let status = match value {
        None => Status::NotMeasured,
        Some(v) if met(v) => Status::Met,
        Some(v) if close(v) => Status::Close,
        Some(_) => Status::Missed,
    };
    TargetCheck { id: id.into(), label: label.into(), value, target: target.into(), status }
}

/// Target checks with the streaming pass bars (the behaviour before setup profiles).
pub fn check_targets(m: &AudioMetrics) -> Vec<TargetCheck> {
    check_targets_for(m, SetupProfile::Streaming)
}

/// Format a dB/dBFS bar the way the report card shows it ("−60", not "-60").
fn num(v: f64) -> String {
    let s = if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    s.replace('-', "−")
}

/// Target checks with one setup profile's pass bars. Ids and labels are the
/// same for every profile; only `target` text and the verdicts move.
pub fn check_targets_for(m: &AudioMetrics, profile: SetupProfile) -> Vec<TargetCheck> {
    let t = profile.targets();
    let (sp, spc, lp, nf, snr, room) = (t.speech_peak, t.speech_peak_close, t.loud_peak, t.noise_floor, t.snr, t.room_decay);
    vec![
        check("speech_peaks", "Normal talking level", Some(m.speech_peak_dbfs), &format!("{} to {} dBFS", num(sp.0), num(sp.1)), |v| (sp.0..=sp.1).contains(&v), |v| (spc.0..=spc.1).contains(&v)),
        check("loud_peaks", "Loudest moments", m.loud_peak_dbfs, &format!("≤ {} dBFS", num(lp.met)), |v| v <= lp.met, |v| v <= lp.close),
        check("noise_floor", "Background noise", Some(m.noise_floor_dbfs), &format!("≤ {} dBFS", num(nf.met)), |v| v <= nf.met, |v| v <= nf.close),
        check("snr", "Voice above noise", Some(m.snr_db), &format!("≥ {} dB", num(snr.met)), |v| v >= snr.met, |v| v >= snr.close),
        check("clipping", "No distortion", Some(m.clipped_fraction * 100.0), "0 %", |v| v == 0.0, |v| v < 0.001),
        check("one_ear", "Both ears", Some(if m.dead_channel.is_some() { 1.0 } else { 0.0 }), "both channels", |v| v == 0.0, |_| false),
        check("room", "Room echo", m.reverb_decay_s, &format!("≤ {} s", num(room.met)), |v| v <= room.met, |v| v <= room.close),
        check("voice_loudness", "Voice loudness", Some(m.speech_lufs), "≈ −16 LUFS (−18 to −14)", |v| (-18.0..=-14.0).contains(&v), |v| (-21.0..=-12.0).contains(&v)),
    ]
}
