//! Structure-borne noise: vibration that travels desk -> boom arm / stand ->
//! mic body (thumps, bumps, hand or arm contact with the desk).
//!
//! Told apart from the other low-frequency things we measure:
//! * plosives: short bursts that sit right in front of a vowel (speech nearby),
//! * room rumble / fans: steady, not bursts (see `NoiseCharacter::Rumble`),
//! * mains hum: tonal at 50/60 Hz and harmonics (see `detect_hum`).
//!
//! Evidence, strongest first:
//! 1. the deliberate desk tap: how loud a tap is next to speech, how long the
//!    thud rings on (a boom arm that touches the desk keeps ringing) and
//!    whether it arrives as a low thud rather than an airborne click,
//! 2. keyboard thumps: low-frequency energy while typing,
//! 3. isolated low-frequency bumps in the quiet and in speech pauses, with no
//!    voice right next to them (so not plosives).
//!
//! Levels are relative to the user's speech peaks, so they do not depend on
//! the gain setting.

use super::analyze::AudioMetrics;
use super::dsp::{self, Biquad};
use serde::{Deserialize, Serialize};

/// What we measured about desk / boom-arm vibration reaching the mic.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct VibrationInfo {
    /// How long a desk tap's low-frequency thud keeps going until it has died
    /// away 20 dB (ms, capped at 800). A mic that touches the desk through its
    /// arm or stand keeps ringing.
    pub tap_ring_ms: Option<f64>,
    /// Full-band peak minus low-frequency peak of the tap (dB). Small = the
    /// tap arrives as a thud (through the stand), large = as a click through the air.
    pub tap_lf_gap_db: Option<f64>,
    /// Low-frequency peak while typing minus speech peak (dB).
    pub typing_lf_rel_db: Option<f64>,
    /// Same gap as for the tap, while typing.
    pub typing_lf_gap_db: Option<f64>,
    /// How far the tap's low-frequency peak stands above the steady low-frequency
    /// level of the room (dB). Steady rumble (fans, HVAC) must not count as a thud.
    pub tap_over_steady_db: Option<f64>,
    /// Same for the typing step.
    pub typing_over_steady_db: Option<f64>,
    /// Isolated low-frequency bumps outside speech (not plosives).
    pub thump_events: usize,
    /// Loudest such bump minus speech peak (dB).
    pub thump_worst_db: Option<f64>,
}

/// Low-frequency band where desk and arm vibration lives (20-120 Hz).
pub fn lf_band(x: &[f32], sr: u32) -> Vec<f32> {
    dsp::filtered(
        x,
        &[
            Biquad::highpass(sr, 20.0, 0.707),
            Biquad::highpass(sr, 20.0, 0.707),
            Biquad::lowpass(sr, 120.0, 0.707),
            Biquad::lowpass(sr, 120.0, 0.707),
        ],
    )
}

fn mid_band(x: &[f32], sr: u32) -> Vec<f32> {
    dsp::filtered(x, &[Biquad::highpass(sr, 300.0, 0.707), Biquad::lowpass(sr, 3000.0, 0.707)])
}

/// (low-frequency peak, full-band peak) in dBFS, ignoring the first 50 ms
/// (filter start-up).
fn peaks_db(x: &[f32], sr: u32) -> (f64, f64) {
    let lf = lf_band(x, sr);
    let skip = (sr as usize / 20).min(lf.len());
    (dsp::db_amp(dsp::peak_abs(&lf[skip..])), dsp::db_amp(dsp::peak_abs(&x[skip..])))
}

/// Ring time of the biggest thud: from its peak until the 10 ms low-frequency
/// level has first dropped 20 dB and stayed there for 40 ms (capped at 800 ms).
fn ring_ms(lf: &[f32], sr: u32) -> Option<f64> {
    let f = dsp::frames(lf, sr, 10.0);
    let n = f.peak_db.len();
    if n < 8 {
        return None;
    }
    // The first frames are filter start-up, not taps.
    let start = 5;
    let (pk, pv) = f.peak_db.iter().enumerate().skip(start).fold((start, f64::NEG_INFINITY), |a, (i, v)| if *v > a.1 { (i, *v) } else { a });
    if pv < -90.0 {
        return None;
    }
    // A 30 ms window, so the zero crossings of a low tone do not end the ring early.
    let env = |i: usize| -> f64 {
        let a = i.saturating_sub(1);
        let b = (i + 2).min(n);
        f.peak_db[a..b].iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    };
    let cap = 80usize;
    let mut i = pk + 1;
    while i < n.min(pk + cap) {
        if (i..(i + 4).min(n)).all(|j| env(j) < pv - 20.0) {
            break;
        }
        i += 1;
    }
    Some((i - pk) as f64 * f.frame_s * 1000.0)
}

/// Isolated low-frequency bumps: peaks of the low-frequency band well above its
/// typical level in this stretch, with no voice within +-60 ms (that would be a
/// plosive) and not in the first 100 ms (filter start-up). Returns the peak
/// level (dBFS) of each bump.
fn isolated_bumps(x: &[f32], sr: u32, speech_mid_db: f64, rise_db: f64) -> Vec<f64> {
    let lf = lf_band(x, sr);
    let mid = mid_band(x, sr);
    let lf_f = dsp::frames(&lf, sr, 10.0);
    let mid_f = dsp::frames(&mid, sr, 10.0);
    let n = lf_f.rms_db.len().min(mid_f.rms_db.len());
    if n < 30 {
        return Vec::new();
    }
    let typical = dsp::percentile(&lf_f.rms_db[10..n], 50.0);
    let mut peaks: Vec<(usize, f64)> = Vec::new();
    for i in 10..n - 1 {
        let v = lf_f.rms_db[i];
        if v > typical + rise_db && v >= lf_f.rms_db[i - 1] && v >= lf_f.rms_db[i + 1] {
            let a = i.saturating_sub(6);
            let b = (i + 7).min(n);
            if mid_f.rms_db[a..b].iter().any(|m| *m > speech_mid_db - 15.0) {
                continue;
            }
            match peaks.last_mut() {
                Some(last) if i - last.0 <= 15 => {
                    if lf_f.peak_db[i] > last.1 {
                        *last = (i, lf_f.peak_db[i]);
                    }
                }
                _ => peaks.push((i, lf_f.peak_db[i])),
            }
        }
    }
    peaks.into_iter().map(|p| p.1).collect()
}

/// Level (dB) of the 300-3000 Hz band during speech (median of active frames).
pub fn speech_mid_db(read: &[f32], sr: u32, active_thresh: f64) -> f64 {
    let mid = mid_band(read, sr);
    let f = dsp::frames(&mid, sr, 10.0);
    let all = dsp::frames(read, sr, 10.0);
    let act: Vec<f64> = f.rms_db.iter().zip(&all.rms_db).filter(|(_, a)| **a > active_thresh).map(|(m, _)| *m).collect();
    if act.is_empty() { f.rms_db.iter().cloned().fold(-120.0, f64::max) } else { dsp::median(&act) }
}

/// Segments and reference levels for [`measure`].
pub struct Inputs<'a> {
    pub sr: u32,
    pub speech_peak_dbfs: f64,
    pub speech_mid_db: f64,
    pub tap: Option<&'a [f32]>,
    pub typing: Option<&'a [f32]>,
    pub silence: Option<&'a [f32]>,
    pub read: &'a [f32],
}

pub fn measure(i: &Inputs) -> VibrationInfo {
    let sr = i.sr;
    // Steady low-frequency level: the quiet step if we have it, else the quietest part of the segment itself.
    let steady = |own: &[f32]| -> f64 {
        let src = i.silence.unwrap_or(own);
        let f = dsp::frames(&lf_band(src, sr), sr, 10.0);
        let skip = 5.min(f.rms_db.len());
        if f.rms_db.len() <= skip { return -120.0; }
        dsp::percentile(&f.rms_db[skip..], if i.silence.is_some() { 50.0 } else { 20.0 })
    };
    let tap_over_steady_db = i.tap.map(|t| peaks_db(t, sr).0 - steady(t));
    let typing_over_steady_db = i.typing.map(|t| peaks_db(t, sr).0 - steady(t));
    let tap_ring_ms = i.tap.and_then(|t| ring_ms(&lf_band(t, sr), sr));
    let tap_lf_gap_db = i.tap.map(|t| {
        let (lf, full) = peaks_db(t, sr);
        full - lf
    });
    let (typing_lf_rel_db, typing_lf_gap_db) = match i.typing {
        Some(t) => {
            let (lf, full) = peaks_db(t, sr);
            (Some(lf - i.speech_peak_dbfs), Some(full - lf))
        }
        None => (None, None),
    };
    let mut bumps: Vec<f64> = Vec::new();
    if let Some(s) = i.silence {
        bumps.extend(isolated_bumps(s, sr, i.speech_mid_db, 15.0));
    }
    bumps.extend(isolated_bumps(i.read, sr, i.speech_mid_db, 15.0));
    let thump_worst_db = bumps.iter().cloned().reduce(f64::max).map(|w| w - i.speech_peak_dbfs);
    VibrationInfo { tap_ring_ms, tap_lf_gap_db, typing_lf_rel_db, typing_lf_gap_db, tap_over_steady_db, typing_over_steady_db, thump_events: bumps.len(), thump_worst_db }
}

// Decision thresholds. Starting values: the "tap shows as loud as speech" one
// is the pre-existing desk-thump rule; the others come from synthetic
// scenarios (core/tests/mic_scenarios.rs) and need confirming on real
// recordings (see core/examples/analyze_wav.rs).
/// A tap this close to the speech peaks (dB) means real desk hits drown the voice.
pub const TAP_LOUD_DB: f64 = -10.0;
/// A thud-like tap this close to the speech peaks (dB) is a clue.
pub const TAP_THUD_REL_DB: f64 = -30.0;
/// A tap that rings at least this long (ms) ...
pub const TAP_RING_MS: f64 = 500.0;
/// ... widens that to this (dB below speech peaks): a ringing stand.
pub const TAP_RING_MIN_REL_DB: f64 = -35.0;
/// Thuds, not clicks: low-frequency peak within this many dB of the full-band peak.
pub const LF_GAP_MAX_DB: f64 = 12.0;
/// Typing thumps reach this close to the speech peaks (dB).
pub const TYPING_LF_REL_DB: f64 = -24.0;
/// Several smaller bumps count as a weaker clue above this (dB below speech peaks).
pub const BUMP_MIN_DB: f64 = -40.0;
/// Isolated bumps (at least 2) this close to the speech peaks (dB) count on their own.
pub const BUMP_LOUD_DB: f64 = -20.0;
/// A thud must stand this far above the steady low-frequency level (dB), so fans and HVAC don't count.
pub const OVER_STEADY_DB: f64 = 15.0;

/// Why we think vibration is reaching the mic.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// A tap or bump that is clearly loud on its own.
    pub strong: bool,
    /// Number of independent weaker clues (thud-like tap, typing thumps, small bumps).
    pub weak: usize,
    /// The tap step was done (otherwise the verdict rests on passive clues).
    pub tapped: bool,
}

impl Verdict {
    /// Major when a tap or bump is loud on its own, or three clues agree.
    pub fn major(&self) -> bool {
        self.strong || self.weak >= 3
    }
}

/// Flag desk / boom vibration: one strong clue, or at least two weaker clues
/// that agree. Plain loudness of the keyboard, rumble, hum and plosives alone
/// never trigger it.
pub fn assess(m: &AudioMetrics) -> Option<Verdict> {
    if m.no_signal {
        return None;
    }
    let v = &m.vibration;
    let tap_stands_out = v.tap_over_steady_db.map(|o| o >= OVER_STEADY_DB).unwrap_or(false);
    let typing_stands_out = v.typing_over_steady_db.map(|o| o >= OVER_STEADY_DB).unwrap_or(false);
    let tap_loud = tap_stands_out && m.tap_rel_db.map(|t| t > TAP_LOUD_DB).unwrap_or(false);
    let bumps_loud = v.thump_events >= 2 && v.thump_worst_db.map(|w| w > BUMP_LOUD_DB).unwrap_or(false);
    // The tap arrives as a low thud (not an airborne click) and is only a
    // moderate step below speech; a thud that also rings on counts a bit wider.
    let thud_tap = tap_stands_out && v.tap_lf_gap_db.map(|g| g <= LF_GAP_MAX_DB).unwrap_or(false)
        && m.tap_rel_db.map(|t| t > TAP_THUD_REL_DB || (v.tap_ring_ms.map(|r| r >= TAP_RING_MS).unwrap_or(false) && t > TAP_RING_MIN_REL_DB)).unwrap_or(false);
    let typing_thumps = typing_stands_out && v.typing_lf_rel_db.map(|t| t > TYPING_LF_REL_DB).unwrap_or(false)
        && v.typing_lf_gap_db.map(|g| g <= LF_GAP_MAX_DB).unwrap_or(false);
    let strong = tap_loud || bumps_loud;
    let weak = thud_tap as usize + typing_thumps as usize + (v.thump_events >= 2 && !bumps_loud && v.thump_worst_db.map(|w| w > BUMP_MIN_DB).unwrap_or(false)) as usize;
    if strong || weak >= 2 { Some(Verdict { strong, weak, tapped: m.tap_rel_db.is_some() }) } else { None }
}
