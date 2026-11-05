//! Compute the static processing chain from measurements.
//! Live order: gain → [our VST3: mono, high-pass, EQ, de-esser] → OBS
//! expander → compressor → limiter.

use super::analyze::{AudioMetrics, NoiseCharacter};
use super::dsp::BAND_CENTERS;
use super::findings::MicContext;
use crate::Constraint;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqKind {
    Peak,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub kind: EqKind,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
    /// Plain-language reason ("less mud").
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expander {
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compressor {
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeEsser {
    pub freq_hz: f64,
    /// Share of total energy (dB) in the 'ess' band above which highs are
    /// turned down; relative, so it doesn't depend on the voice level.
    pub threshold_db: f64,
    pub max_reduction_db: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limiter {
    pub ceiling_db: f64,
    pub release_ms: f64,
}

/// The settings we apply (OBS filters + our VST3 for HPF/EQ/de-esser/mono).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingChain {
    /// Collapse to mono (interface input 1 arriving as left-only).
    pub mono_downmix: bool,
    /// Suggested change to the hardware gain knob/slider (dB, + = up). The app
    /// coaches this; it is not applied digitally.
    pub hardware_gain_change_db: f64,
    /// Digital gain applied at the start of the chain (dB).
    pub input_gain_db: f64,
    pub highpass_hz: f64,
    /// 12 or 24 dB/octave.
    pub highpass_slope_db_oct: u8,
    pub eq: Vec<EqBand>,
    pub expander: Expander,
    pub compressor: Compressor,
    pub deesser: Option<DeEsser>,
    pub limiter: Limiter,
    /// Target short-term loudness of the voice (LUFS).
    pub target_lufs: f64,
    /// Plain-language notes on choices made (incl. constraint workarounds).
    pub notes: Vec<String>,
}

/// Normal speech peaks should land here after gain (dBFS).
pub const TARGET_SPEECH_PEAK: f64 = -12.0;
/// Loudest laugh/shout must stay below this (dBFS).
pub const MAX_LOUD_PEAK: f64 = -6.0;
/// Voice loudness target (LUFS, short-term ≈ integrated over speech).
pub const TARGET_VOICE_LUFS: f64 = -16.0;
/// Never boost or cut more than this with EQ: beyond it the problem is physical.
pub const EQ_LIMIT_DB: f64 = 6.0;

pub fn compute_chain(m: &AudioMetrics, ctx: &MicContext, constraints: &[Constraint]) -> ProcessingChain {
    let has = |c: Constraint| constraints.contains(&c);
    let mut notes = Vec::new();

    // --- Gain staging: hardware first, digital only for what's left -------------
    let mut wanted = TARGET_SPEECH_PEAK - m.speech_peak_dbfs;
    if let Some(lp) = m.loud_peak_dbfs {
        wanted = wanted.min(MAX_LOUD_PEAK - lp);
    }
    if m.clipped_fraction > 1e-5 {
        // Clipped peaks under-read the true level; ask for a decisive cut.
        wanted = wanted.min(-6.0);
    }
    let hw_headroom_known = ctx.gain_at_max == Some(true) || has(Constraint::CantAddCleanGain);
    let (hardware_gain_change_db, input_gain_db) = if wanted > 0.0 && hw_headroom_known {
        notes.push("Your hardware gain is maxed, so the missing level is added digitally.".into());
        (0.0, wanted.min(24.0))
    } else if wanted.abs() < 1.5 {
        (0.0, 0.0)
    } else {
        (round1(wanted), 0.0)
    };
    // What the signal will look like after the user follows the knob advice.
    let gain_after = hardware_gain_change_db + input_gain_db;
    let speech_rms = m.speech_rms_dbfs + gain_after;
    let quiet = m.quiet_speech_dbfs + gain_after;
    let floor = m.noise_floor_dbfs + gain_after;

    // --- High-pass by voice pitch -------------------------------------------------
    let mut highpass_hz: f64 = match m.f0_hz {
        Some(f0) if f0 < 110.0 => 70.0,
        Some(f0) if f0 < 150.0 => 80.0,
        Some(f0) if f0 < 200.0 => 100.0,
        Some(_) => 120.0,
        None => 80.0,
    };
    if m.tone.boom_db > 5.0 {
        highpass_hz += 15.0;
    }
    if m.plosive_events > 0 || has(Constraint::CantFixPlosives) {
        highpass_hz = highpass_hz.max(90.0);
    }
    if m.noise_character == NoiseCharacter::Rumble || has(Constraint::CantRemoveNoiseSource) {
        highpass_hz = highpass_hz.max(100.0);
    }
    if m.tone.body_db < -8.0 {
        highpass_hz = highpass_hz.min(70.0);
    }
    // Desk / boom vibration: the fix is physical. Gain is never turned down to
    // hide it; a steeper low-cut is only the backup once the user can't fix it.
    let desk_vibration = super::vibration::assess(m).is_some();
    let steep_for_desk = desk_vibration && has(Constraint::CantFixDeskVibration);
    if steep_for_desk {
        highpass_hz = highpass_hz.max(120.0);
        notes.push("Desk thumps stay, so the low-cut filter is steeper. It softens them but can't remove them; freeing the arm from the desk would.".into());
    } else if desk_vibration {
        notes.push("Your mic is picking up desk shaking. Turning the gain down would only hide it, so we don't offer that as the fix: free the arm from the desk (or pad the clamp) and test again.".into());
    }
    let highpass_slope_db_oct = if steep_for_desk || m.plosive_events > 2 || m.noise_character == NoiseCharacter::Rumble { 24 } else { 12 };

    // --- EQ from the deviation curve -----------------------------------------------
    let eq = derive_eq(m, has(Constraint::CantMoveMicCloser) || has(Constraint::CantTreatRoom), highpass_hz);
    if has(Constraint::CantMoveMicCloser) {
        notes.push("Mic stays where it is: no bass boost (it would boost room mud), small clarity lift instead.".into());
    }

    // --- Expander: midway between noise floor and quiet speech ------------------------
    let mut exp_thr = (floor + quiet) / 2.0;
    if quiet - floor < 10.0 {
        exp_thr = quiet - 6.0; // little room: protect words first
        notes.push("Noise is close to your quiet speech, so the expander is set gently to avoid cutting words.".into());
    }
    let mut exp_ratio: f64 = 2.5;
    if has(Constraint::CantRemoveNoiseSource) || has(Constraint::CantAddCleanGain) || has(Constraint::CantTreatRoom) || has(Constraint::CantMoveMicCloser) {
        exp_ratio = 4.0;
    }
    if has(Constraint::MovesALot) {
        exp_ratio = exp_ratio.min(2.0);
        exp_thr -= 3.0;
    }
    let expander = Expander { threshold_db: round1(exp_thr), ratio: exp_ratio, attack_ms: 3.0, release_ms: 150.0 };

    // --- Compressor: ~3–6 dB of reduction on normal speech -----------------------------
    let (ratio, gr) = if has(Constraint::MovesALot) {
        (4.0, 6.0)
    } else if has(Constraint::CantMoveMicCloser) || has(Constraint::CantTreatRoom) {
        (2.5, 3.0)
    } else {
        (3.0, 4.0)
    };
    let threshold = speech_rms - gr / (1.0 - 1.0 / ratio);
    // Approximate loudness after compression: speech loses ~gr dB.
    let lufs_after_gain = m.speech_lufs + gain_after;
    let makeup = (TARGET_VOICE_LUFS - (lufs_after_gain - gr)).clamp(-6.0, 18.0);
    let compressor = Compressor { threshold_db: round1(threshold), ratio, attack_ms: 3.0, release_ms: 100.0, makeup_db: round1(makeup) };

    // --- De-esser only when measured -----------------------------------------------------
    let deesser = m.sibilance_db.filter(|s| *s > -3.0).map(|s| DeEsser {
        freq_hz: 6500.0,
        threshold_db: -8.0,
        max_reduction_db: (s + 6.0).clamp(4.0, 8.0),
    });

    ProcessingChain {
        mono_downmix: m.dead_channel.is_some(),
        hardware_gain_change_db,
        input_gain_db: round1(input_gain_db),
        highpass_hz,
        highpass_slope_db_oct,
        eq,
        expander,
        compressor,
        deesser,
        limiter: Limiter { ceiling_db: -1.5, release_ms: 60.0 },
        target_lufs: TARGET_VOICE_LUFS,
        notes,
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Up to 5 gentle, broad moves. Cuts preferred; boosts small and only where
/// physics allows (no low boosts when the mic must stay far / room is bad).
fn derive_eq(m: &AudioMetrics, distant: bool, hpf: f64) -> Vec<EqBand> {
    let dev = &m.band_deviation_db;
    let mut bands: Vec<EqBand> = Vec::new();
    // Regions: (lo, hi, label for cut, label for boost, allow boost, boost cap)
    let regions: [(f64, f64, &str, &str, bool, f64); 5] = [
        (200.0, 500.0, "less mud", "more warmth", !distant, 2.0),
        (400.0, 900.0, "less boxy", "", false, 0.0),
        (800.0, 1600.0, "less nasal", "", false, 0.0),
        (2000.0, 5000.0, "less harsh", "more clarity", true, if distant { 3.0 } else { 2.5 }),
        (6300.0, 10000.0, "less hiss/edge", "more air", true, 2.0),
    ];
    for (lo, hi, cut_label, boost_label, allow_boost, cap) in regions {
        let idx: Vec<usize> = (0..BAND_CENTERS.len()).filter(|i| BAND_CENTERS[*i] >= lo && BAND_CENTERS[*i] <= hi).collect();
        if idx.is_empty() {
            continue;
        }
        let avg = idx.iter().map(|i| dev[*i]).sum::<f64>() / idx.len() as f64;
        // Centre on the most deviating band in the region.
        let peak_i = *idx.iter().max_by(|a, b| dev[**a].abs().partial_cmp(&dev[**b].abs()).unwrap()).unwrap();
        let fc = BAND_CENTERS[peak_i];
        let q = 1.0;
        if avg > 3.0 {
            let g = -(avg * 0.7).min(EQ_LIMIT_DB);
            if bands.iter().any(|b| (b.freq_hz / fc).log2().abs() < 0.5) {
                continue;
            }
            bands.push(EqBand { kind: EqKind::Peak, freq_hz: fc, gain_db: round1(g), q, reason: cut_label.into() });
        } else if allow_boost && avg < -4.0 {
            let g = ((-avg) * 0.5).min(cap);
            let kind = if lo >= 6000.0 { EqKind::HighShelf } else { EqKind::Peak };
            bands.push(EqBand { kind, freq_hz: if kind == EqKind::HighShelf { 8000.0 } else { fc }, gain_db: round1(g), q: 0.8, reason: boost_label.into() });
        }
    }
    // Body boost only for thin voices close enough to the mic.
    if !distant && m.tone.body_db < -8.0 {
        bands.push(EqBand { kind: EqKind::LowShelf, freq_hz: 180.0_f64.max(hpf * 1.8), gain_db: 2.0, q: 0.7, reason: "more warmth".into() });
    }
    bands.truncate(5);
    bands
}
