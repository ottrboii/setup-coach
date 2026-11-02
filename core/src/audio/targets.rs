//! Target checks for the report card: ✅ met / ⚠️ close / ❌ missed.

use super::analyze::AudioMetrics;
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

pub fn check_targets(m: &AudioMetrics) -> Vec<TargetCheck> {
    vec![
        check("speech_peaks", "Normal talking level", Some(m.speech_peak_dbfs), "−18 to −12 dBFS", |v| (-18.0..=-12.0).contains(&v), |v| (-24.0..=-10.0).contains(&v)),
        check("loud_peaks", "Loudest moments", m.loud_peak_dbfs, "≤ −6 dBFS", |v| v <= -6.0, |v| v <= -3.0),
        check("noise_floor", "Background noise", Some(m.noise_floor_dbfs), "≤ −60 dBFS", |v| v <= -60.0, |v| v <= -50.0),
        check("snr", "Voice above noise", Some(m.snr_db), "≥ 50 dB", |v| v >= 50.0, |v| v >= 40.0),
        check("clipping", "No distortion", Some(m.clipped_fraction * 100.0), "0 %", |v| v == 0.0, |v| v < 0.001),
        check("one_ear", "Both ears", Some(if m.dead_channel.is_some() { 1.0 } else { 0.0 }), "both channels", |v| v == 0.0, |_| false),
        check("room", "Room echo", m.reverb_decay_s, "≤ 0.35 s", |v| v <= 0.35, |v| v <= 0.5),
        check("voice_loudness", "Voice loudness", Some(m.speech_lufs), "≈ −16 LUFS (−18 to −14)", |v| (-18.0..=-14.0).contains(&v), |v| (-21.0..=-12.0).contains(&v)),
    ]
}
