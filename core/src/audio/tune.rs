//! "Make it sound better": compose an EQ / de-esser / dynamics adjustment for
//! the processing chain, then verify it offline (docs/voice-tuning-design.md,
//! sections 5 and 6). Plain filters only: no AI processing.
//!
//! The desired net EQ curve is `D = M + P + R` per third-octave:
//! * **M (mic)**: minus half the maker's published deviation from 1 kHz,
//!   smoothed to 2/3 octave, broad features only (|dev| >= 2 dB over >= 2/3
//!   octave), each point within +-3 dB. No published curve = no M. Weight 0.7
//!   once a curve is `verified`. Proximity is not corrected from the idealised
//!   model; it only limits low-end moves to +-2 dB when the predicted swing
//!   across the coached distance range is over 4 dB.
//! * **P (profile)**: the profile tone delta, 0 dB until a listening test.
//! * **R (residual / voice)**: today's region rules (`chain::derive_eq`) on the
//!   deviation left after M, only when the voice measurement is `Good` and no
//!   desk/wall reflection is measured (comb filtering must not be EQ'd).
//!   Otherwise M (and gain) only, with the reason reported.
//!
//! The net curve is capped at -6 / +3 dB on every third-octave 80 Hz-12.5 kHz,
//! evaluated on the actual biquads; R gives way first, then P, never M.
//!
//! Verification renders the test through the chain without and with the tune
//! (same high-pass and dynamics) and re-analyses both. The tune is applied only
//! when it does no harm (V2), keeps the level (V3), respects the cap (V4) and
//! moves the tone closer to the target. V1 (every region within +-3 dB of the
//! target or at least 50 % closer) decides "Pass" vs "Close" (kept gentle).
//! Verification is offline only: it checks the settings, not the live path.

use super::analyze::analyze;
use super::chain::{compute_chain, derive_eq, DeEsser, EqBand, EqKind, EqRules, ProcessingChain};
use super::dsp::{self, Biquad, BAND_CENTERS};
use super::findings::{mic_findings, MicContext};
use super::process::{calibrate_makeup_for, render, render_test};
use super::voice::{self, Confidence, ConfidenceReason, VoiceType};
use super::{AudioBuffer, MicTest, SegmentKind};
use crate::profiles::{proximity_boost_db, MicResponse};
use crate::setup_profile::{LoudnessMeasure, ProfileTuning};
use crate::Constraint;
use serde::{Deserialize, Serialize};

/// Net EQ limits (dB) on the evaluated curve (design 5.3).
pub const NET_CUT_LIMIT_DB: f64 = -6.0;
pub const NET_BOOST_LIMIT_DB: f64 = 3.0;
/// Mic correction never exceeds this per point.
pub const MIC_LIMIT_DB: f64 = 3.0;
/// Weight on a published mic curve: unverified / verified on hardware.
pub const MIC_WEIGHT_UNVERIFIED: f64 = 0.5;
pub const MIC_WEIGHT_VERIFIED: f64 = 0.7;
/// Only chart features at least this large (before weighting) are corrected.
pub const MIC_FEATURE_DB: f64 = 2.0;
/// Predicted proximity swing across the coached range above which low-end
/// moves are limited to +-`LOW_END_LIMIT_DB` (placeholder, design section 8).
pub const PROXIMITY_SWING_DB: f64 = 4.0;
pub const LOW_END_LIMIT_DB: f64 = 2.0;
/// The VST3 runs at most this many EQ bands (`kMaxEqBands`).
pub const MAX_BANDS: usize = 5;
/// De-esser centre range (design section 4).
pub const DEESS_MIN_HZ: f64 = 4500.0;
pub const DEESS_MAX_HZ: f64 = 9000.0;
/// Today's de-esser trigger on the "s" level (dB).
pub const DEESS_ON_ABOVE_DB: f64 = -3.0;
/// A de-esser on its own must lower the measured "s" by more than this (dB).
pub const DEESS_MIN_GAIN_DB: f64 = 0.5;

/// Third-octave points the cap is checked on: the analysis bands plus 12.5 kHz.
pub const CAP_FREQS: [f64; 23] = [
    80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0,
    3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0, 12500.0,
];

/// Tone regions judged by the verification (same as `ToneSummary`).
pub const REGIONS: [(&str, f64, f64); 7] = [
    ("boom", 80.0, 160.0),
    ("body", 125.0, 250.0),
    ("mud", 200.0, 500.0),
    ("box", 400.0, 800.0),
    ("nasal", 800.0, 1600.0),
    ("presence", 2000.0, 5000.0),
    ("air", 6300.0, 10000.0),
];

const TONAL_FINDINGS: [&str; 8] = ["mic.boomy", "mic.muddy", "mic.boxy", "mic.nasal", "mic.harsh", "mic.dull", "mic.thin", "mic.sibilance"];

/// What the composer is given.
#[derive(Debug, Clone)]
pub struct TuneInput<'a> {
    pub test: &'a MicTest,
    pub ctx: &'a MicContext,
    pub constraints: &'a [Constraint],
    /// The mic's published response (`MicProfile::response`); `None` = no correction.
    pub mic: Option<&'a MicResponse>,
    /// The user's switch position, as (control key, option index), if the mic has one.
    pub switch: Option<(String, String)>,
}

/// Where an EQ band came from (display only; the DSP is the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandOrigin {
    Mic,
    Profile,
    Voice,
}

/// Why the voice measurement was not used for the tone moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    /// The read was too short, noisy, band-limited or clipped (`VoiceType::confidence`).
    VoiceConfidence { confidence: Confidence, reason: Option<ConfidenceReason> },
    /// A desk/wall reflection combs the spectrum; it needs a physical fix, not EQ.
    Reflection,
    NoSignal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Verified and every region landed within the tolerance band.
    Pass,
    /// Verified and better; some region is still outside on purpose (moves kept gentle).
    Close,
    /// Nothing to change: the current settings stay.
    NothingToChange,
    /// Did not verify: not applied, the current settings stay.
    NotImproved,
}

impl Verdict {
    pub fn applied(self) -> bool {
        matches!(self, Verdict::Pass | Verdict::Close)
    }
}

/// The curves behind the EQ, per `CAP_FREQS` point (Advanced graph).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TuneCurves {
    pub freqs_hz: Vec<f64>,
    /// M as wanted (smoothed, weighted, clamped), before the band fit.
    pub mic_db: Vec<f64>,
    pub profile_db: Vec<f64>,
    /// Evaluated response of the voice (R) bands after the cap.
    pub voice_db: Vec<f64>,
    /// Evaluated response of all bands (what the plugin will do).
    pub net_db: Vec<f64>,
    /// RMS of (M + P + R wanted) minus net, 125 Hz-8 kHz (dB).
    pub fit_error_db: f64,
    /// Mic weight used (0 when there is no curve).
    pub mic_weight: f64,
    /// Predicted low-end swing across the coached distance range (dB at 100 Hz).
    pub proximity_swing_db: Option<f64>,
}

/// Today's high-pass vs the design's proposal `f0_p10 / 1.5`. Reported only;
/// the chain keeps today's rule (design 5.2 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HpfComparison {
    pub current_hz: f64,
    pub proposed_hz: Option<f64>,
    pub f0_p10_hz: Option<f64>,
    /// Loss at the voice's f0 p10 (dB, negative = cut).
    pub current_loss_db: Option<f64>,
    pub proposed_loss_db: Option<f64>,
}

/// Tone and level of one render, as the verification sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderCheck {
    /// Deviation from the voice-adjusted target per region in `REGIONS` (dB).
    pub regions_db: Vec<f64>,
    /// RMS of the deviation over the judged bands (dB).
    pub tone_rms_db: f64,
    pub sibilance_db: Option<f64>,
    pub plosive_events: usize,
    /// Read loudness on the profile's measure (LUFS or dB RMS).
    pub level: f64,
    pub peak_dbfs: f64,
    pub tonal_findings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    /// The chain without the tune (same high-pass and dynamics).
    pub before: RenderCheck,
    pub after: RenderCheck,
    /// V1: every judged region within +-3 dB or at least 50 % closer.
    pub v1_tone: bool,
    /// V2: no region > 1 dB further, no new tonal finding, "s" <= +1 dB, plosives not up.
    pub v2_no_harm: bool,
    /// V3: level inside the profile window and peaks under the ceiling (or not worse than without the tune).
    pub v3_level: bool,
    /// V4: evaluated net EQ within -6 / +3 dB.
    pub v4_cap: bool,
    /// Tone RMS went down.
    pub improved: bool,
    /// Bands dropped by the do-no-harm loop.
    pub dropped: Vec<EqBand>,
}

/// The composer's answer. `chain` is what to apply: the tune when it verified,
/// otherwise today's chain unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceTune {
    /// Voice measurement, with the known mic correction removed from the tilt.
    pub voice: VoiceType,
    pub fallback: Option<FallbackReason>,
    pub curves: TuneCurves,
    pub hpf: HpfComparison,
    /// The tuned chain as proposed (before verification decided).
    pub proposed: ProcessingChain,
    /// Origin of each band in `proposed.eq`.
    pub origins: Vec<BandOrigin>,
    /// Today's chain (what stays when the tune is not applied).
    pub base: ProcessingChain,
    pub verification: Option<Verification>,
    pub verdict: Verdict,
    /// The chain to apply.
    pub chain: ProcessingChain,
    /// Plain-language notes.
    pub notes: Vec<String>,
}

/// A proposed set of moves, before verification.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub bands: Vec<(EqBand, BandOrigin)>,
    pub deesser: Option<DeEsser>,
}

/// Compose and verify. Pure: the same input gives the same answer.
pub fn compose(input: &TuneInput) -> VoiceTune {
    let test = input.test;
    let sr = test.audio.sample_rate;
    let m = analyze(test);
    let tuning = input.ctx.profile.tuning_for(input.ctx.voiceover_target);
    let raw = compute_chain(&m, input.ctx, input.constraints);
    let mut base = raw.clone();
    if !m.no_signal {
        calibrate_makeup_for(test, &mut base, &tuning);
    }
    let mut notes = Vec::new();

    // --- M: the mic's published colouration ------------------------------------
    let distance = input.ctx.profile.distance_cm(input.ctx);
    let mic = input.mic.map(|r| mic_curve(r, input.switch.as_ref()));
    let mic_weight = input.mic.map_or(0.0, |r| if r.verified { MIC_WEIGHT_VERIFIED } else { MIC_WEIGHT_UNVERIFIED });
    let mut mic_want = mic.as_ref().map_or(vec![0.0; BAND_CENTERS.len()], |c| mic_correction(c, mic_weight));
    let swing = input.mic.and_then(|r| proximity_swing(r, distance));
    let low_limit = swing.is_some_and(|s| s > PROXIMITY_SWING_DB);
    let no_low_boost = input.constraints.contains(&Constraint::MovesALot);
    limit_low_end(&mut mic_want, low_limit, no_low_boost);
    if low_limit {
        notes.push("Your mic's bass changes a lot with distance, so we keep low-end changes small.".into());
    }
    let mut mic_bands = fit_curve(&mic_want, sr, MAX_BANDS, MIC_LIMIT_DB);
    limit_mic_sum(&mut mic_bands, sr);

    // --- Voice measurement, with what we know about the mic removed -----------
    let mut v = m.voice.clone();
    if mic.is_some() {
        let known: Vec<(f64, f64)> = BAND_CENTERS.iter().zip(&mic_want).map(|(f, g)| (*f, -*g)).collect();
        voice::remove_mic_correction(&mut v, &m, Some(&known));
    }
    let fallback = if m.no_signal {
        Some(FallbackReason::NoSignal)
    } else if v.confidence != Confidence::Good {
        Some(FallbackReason::VoiceConfidence { confidence: v.confidence, reason: v.confidence_reason })
    } else if mic_findings(&m, input.ctx).iter().any(|f| f.id == "mic.reflection") {
        Some(FallbackReason::Reflection)
    } else {
        None
    };
    match fallback {
        Some(FallbackReason::VoiceConfidence { .. }) => notes.push("The reading was too short or unclear to judge your voice, so we only corrected the mic.".into()),
        Some(FallbackReason::Reflection) => notes.push("A nearby surface is colouring your voice. EQ can't fix that, so we only corrected the mic: moving the mic away from the desk or wall would.".into()),
        _ => {}
    }

    // --- P: profile tone delta (0 dB until a listening test) --------------------
    let mut profile_want = vec![0.0; BAND_CENTERS.len()];
    let mut profile_bands = Vec::new();
    if let Some(d) = tuning.tone_delta.filter(|d| d.db.abs() > 0.0) {
        let db = d.db.clamp(-1.0, 1.0);
        let fc = (d.lo_hz * d.hi_hz).sqrt();
        let b = EqBand { kind: EqKind::Peak, freq_hz: fc, gain_db: db, q: 1.0, reason: "profile".into() };
        for (i, f) in BAND_CENTERS.iter().enumerate() {
            profile_want[i] = band_response(&b, sr, *f);
        }
        profile_bands.push(b);
    }

    // --- R: residual after M + P, by today's region rules -----------------------
    let mut voice_bands = Vec::new();
    if fallback.is_none() {
        let mp: Vec<EqBand> = mic_bands.iter().chain(&profile_bands).cloned().collect();
        let residual = residual_after(&m.band_deviation_db, &mp, sr);
        let body = mean_over(&residual, 125.0, 250.0);
        let distant = input.constraints.contains(&Constraint::CantMoveMicCloser) || input.constraints.contains(&Constraint::CantTreatRoom);
        voice_bands = derive_eq(&residual, body, EqRules { distant, hpf_hz: raw.highpass_hz, boost_above_7k: tuning.boost_above_7k });
        for b in voice_bands.iter_mut() {
            if b.freq_hz < 250.0 {
                if low_limit {
                    b.gain_db = b.gain_db.clamp(-LOW_END_LIMIT_DB, LOW_END_LIMIT_DB);
                }
                if no_low_boost {
                    b.gain_db = b.gain_db.min(0.0);
                }
            }
        }
        voice_bands.retain(|b| b.gain_db != 0.0);
    }

    // --- Combine, fit to the plugin's band count, cap ---------------------------
    let mut bands: Vec<(EqBand, BandOrigin)> = mic_bands
        .iter()
        .map(|b| (b.clone(), BandOrigin::Mic))
        .chain(profile_bands.iter().map(|b| (b.clone(), BandOrigin::Profile)))
        .chain(voice_bands.iter().map(|b| (b.clone(), BandOrigin::Voice)))
        .collect();
    limit_band_count(&mut bands);
    let capped = cap_bands(&mut bands, sr);
    if capped {
        notes.push("Part of the remaining tone problem is beyond what EQ should do; the physical fix still helps.".into());
    }
    for (b, _) in bands.iter_mut() {
        b.gain_db = trunc1(b.gain_db);
    }
    bands.retain(|(b, _)| b.gain_db != 0.0);

    // --- Curves for Advanced ----------------------------------------------------
    let at = |want: &[f64], f: f64| if f > 10000.0 { 0.0 } else { want[band_index(f)] };
    let only = |o: BandOrigin| bands.iter().filter(|x| x.1 == o).map(|x| x.0.clone()).collect::<Vec<_>>();
    let voice_final = only(BandOrigin::Voice);
    let all: Vec<EqBand> = bands.iter().map(|x| x.0.clone()).collect();
    let fit_error_db = {
        let sq: Vec<f64> = BAND_CENTERS
            .iter()
            .enumerate()
            .filter(|(_, f)| (125.0..=8000.0).contains(*f))
            .map(|(i, f)| (mic_want[i] + profile_want[i] + net_response(&voice_final, sr, *f) - net_response(&all, sr, *f)).powi(2))
            .collect();
        (sq.iter().sum::<f64>() / sq.len() as f64).sqrt()
    };
    let curves = TuneCurves {
        freqs_hz: CAP_FREQS.to_vec(),
        mic_db: CAP_FREQS.iter().map(|f| at(&mic_want, *f)).collect(),
        profile_db: CAP_FREQS.iter().map(|f| at(&profile_want, *f)).collect(),
        voice_db: CAP_FREQS.iter().map(|f| net_response(&voice_final, sr, *f)).collect(),
        net_db: CAP_FREQS.iter().map(|f| net_response(&all, sr, *f)).collect(),
        fit_error_db,
        mic_weight,
        proximity_swing_db: swing,
    };

    // --- De-esser: centre from the voice, on only if the EQ'd render needs it ----
    let deesser = if fallback.is_none() {
        let mut eqd = raw.clone();
        eqd.eq = all.clone();
        eqd.deesser = None;
        let sib = sibilance_after(test, &eqd);
        match (sib, v.sib_peak_hz) {
            (Some(s), Some(peak)) if s > DEESS_ON_ABOVE_DB => Some(DeEsser {
                freq_hz: peak.clamp(DEESS_MIN_HZ, DEESS_MAX_HZ).round(),
                threshold_db: -8.0,
                max_reduction_db: (s + 6.0).clamp(4.0, 8.0),
            }),
            (Some(s), None) if s > DEESS_ON_ABOVE_DB => raw.deesser.clone(),
            _ => None,
        }
    } else {
        raw.deesser.clone()
    };

    let hpf = hpf_comparison(raw.highpass_hz, raw.highpass_slope_db_oct, if fallback.is_none() { v.f0_p10_hz } else { None });
    let candidate = Candidate { bands, deesser };
    let mut proposed = raw.clone();
    proposed.eq = candidate.bands.iter().map(|x| x.0.clone()).collect();
    proposed.deesser = candidate.deesser.clone();
    let origins = candidate.bands.iter().map(|x| x.1).collect();

    if m.no_signal {
        return VoiceTune { voice: v, fallback, curves, hpf, proposed, origins, base: base.clone(), verification: None, verdict: Verdict::NothingToChange, chain: base, notes };
    }
    let (verdict, verification, chain) = verify_and_decide(test, input.ctx, &raw, &base, candidate, &tuning);
    match verdict {
        Verdict::NotImproved => notes.push("The change didn't measure better, so we kept your current settings.".into()),
        Verdict::Close => notes.push("Tuned toward published targets; we kept some moves gentle on purpose.".into()),
        Verdict::Pass => notes.push("Tuned toward published targets and checked.".into()),
        Verdict::NothingToChange => {}
    }
    VoiceTune { voice: v, fallback, curves, hpf, proposed, origins, base, verification, verdict, chain, notes }
}

/// Verify a candidate against the same chain without it, dropping the worst
/// band up to twice when it does harm. Returns the verdict, the last
/// verification and the chain to apply (`base` unless the tune verified).
pub fn verify_and_decide(
    test: &MicTest,
    ctx: &MicContext,
    raw: &ProcessingChain,
    base: &ProcessingChain,
    mut cand: Candidate,
    tuning: &ProfileTuning,
) -> (Verdict, Option<Verification>, ProcessingChain) {
    let sr = test.audio.sample_rate;
    let mut reference = raw.clone();
    reference.eq.clear();
    reference.deesser = None;
    calibrate_makeup_for(test, &mut reference, tuning);
    let before = check_render(test, ctx, &reference, tuning);
    let mut dropped = Vec::new();
    let mut last: Option<Verification> = None;
    for round in 0..=2 {
        if cand.bands.is_empty() && cand.deesser.is_none() {
            let verdict = if last.is_none() { Verdict::NothingToChange } else { Verdict::NotImproved };
            return (verdict, last, base.clone());
        }
        let mut tuned = raw.clone();
        tuned.eq = cand.bands.iter().map(|x| x.0.clone()).collect();
        tuned.deesser = cand.deesser.clone();
        calibrate_makeup_for(test, &mut tuned, tuning);
        let after = check_render(test, ctx, &tuned, tuning);
        let judged = judged_regions(tuning);
        let v1_tone = judged.iter().all(|&i| after.regions_db[i].abs() <= 3.0 || after.regions_db[i].abs() <= 0.5 * before.regions_db[i].abs());
        let worse: Vec<usize> = judged.iter().copied().filter(|&i| after.regions_db[i].abs() > before.regions_db[i].abs() + 1.0).collect();
        let new_finding = after.tonal_findings.iter().any(|f| !before.tonal_findings.contains(f));
        // "s" up by more than 1 dB counts as harm once it reaches the strong
        // range (where the de-esser and the sibilance finding act); a soft "s"
        // rising from -28 to -24 dB is not.
        let sib_worse = after.sibilance_db.is_some_and(|a| a > voice::SIB_STRONG_ABOVE_DB && before.sibilance_db.is_none_or(|b| a > b + 1.0));
        let v2_no_harm = worse.is_empty() && !new_finding && !sib_worse && after.plosive_events <= before.plosive_events;
        let accept = tuning.loudness.accept;
        let in_window = |c: &RenderCheck| c.level >= accept.0 && c.level <= accept.1;
        let peaks_ok = after.peak_dbfs <= tuning.limiter_ceiling_db + 0.1;
        // The make-up loop itself accepts up to 1.5 dB under target when the
        // peaks need it, so "within 1 dB of the chain without the tune" also passes.
        let v3_level = peaks_ok && (in_window(&after) || (after.level - before.level).abs() <= 1.0);
        let net: Vec<f64> = CAP_FREQS.iter().filter(|f| **f < 0.45 * sr as f64).map(|f| net_response(&tuned.eq, sr, *f)).collect();
        let v4_cap = net.iter().all(|g| *g >= NET_CUT_LIMIT_DB - 1e-6 && *g <= NET_BOOST_LIMIT_DB + 1e-6);
        // A de-esser-only candidate counts as better only when the "s" measurably
        // went down: its high cut alone can lower the tone RMS while the "s" rises.
        let sib_down = matches!((after.sibilance_db, before.sibilance_db), (Some(a), Some(b)) if a < b - DEESS_MIN_GAIN_DB);
        let improved = if tuned.eq.is_empty() { tuned.deesser.is_some() && sib_down } else { after.tone_rms_db < before.tone_rms_db - 0.05 };
        let ver = Verification { before: before.clone(), after, v1_tone, v2_no_harm, v3_level, v4_cap, improved, dropped: dropped.clone() };
        if v2_no_harm && v3_level && v4_cap && improved {
            let verdict = if v1_tone { Verdict::Pass } else { Verdict::Close };
            return (verdict, Some(ver), tuned);
        }
        if round == 2 {
            return (Verdict::NotImproved, Some(ver), base.clone());
        }
        // A de-esser that made the "s" or the level worse goes first.
        if (sib_worse || !v3_level) && cand.deesser.is_some() {
            cand.deesser = None;
            last = Some(ver);
            continue;
        }
        if cand.bands.is_empty() {
            return (Verdict::NotImproved, Some(ver), base.clone());
        }
        // Drop the band nearest the region that got worst (or the biggest boost
        // above 2 kHz when the "s" got worse; or the biggest move otherwise).
        let worst_region = worse.iter().copied().max_by(|a, b| {
            let da = ver.after.regions_db[*a].abs() - before.regions_db[*a].abs();
            let db = ver.after.regions_db[*b].abs() - before.regions_db[*b].abs();
            da.total_cmp(&db)
        });
        let idx = if let Some(r) = worst_region {
            let fc = (REGIONS[r].1 * REGIONS[r].2).sqrt();
            nearest_band(&cand.bands, fc)
        } else if sib_worse {
            cand.bands.iter().enumerate().filter(|(_, x)| x.0.freq_hz >= 2000.0 && x.0.gain_db > 0.0).max_by(|a, b| a.1 .0.gain_db.total_cmp(&b.1 .0.gain_db)).map(|x| x.0)
        } else {
            None
        }
        .or_else(|| cand.bands.iter().enumerate().max_by(|a, b| a.1 .0.gain_db.abs().total_cmp(&b.1 .0.gain_db.abs())).map(|x| x.0));
        match idx {
            Some(i) => dropped.push(cand.bands.remove(i).0),
            None => return (Verdict::NotImproved, Some(ver), base.clone()),
        }
        last = Some(ver);
    }
    (Verdict::NotImproved, last, base.clone())
}

fn nearest_band(bands: &[(EqBand, BandOrigin)], fc: f64) -> Option<usize> {
    bands
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 .0.freq_hz / fc).log2().abs().total_cmp(&(b.1 .0.freq_hz / fc).log2().abs()))
        .map(|x| x.0)
}

fn judged_regions(t: &ProfileTuning) -> Vec<usize> {
    (0..REGIONS.len())
        .filter(|&i| {
            let c = (REGIONS[i].1 * REGIONS[i].2).sqrt();
            c >= t.judge_hz.0 && c <= t.judge_hz.1
        })
        .collect()
}

/// Render, re-analyse and measure what the verification needs.
pub fn check_render(test: &MicTest, ctx: &MicContext, chain: &ProcessingChain, tuning: &ProfileTuning) -> RenderCheck {
    let out = render_test(test, chain);
    let m = analyze(&out);
    let regions_db = region_values(&m.band_deviation_db);
    let judged: Vec<f64> = BAND_CENTERS
        .iter()
        .zip(&m.band_deviation_db)
        .filter(|(f, _)| **f >= tuning.judge_hz.0.max(chain.highpass_hz * 1.5) && **f <= tuning.judge_hz.1)
        .map(|(_, d)| d * d)
        .collect();
    let tone_rms_db = if judged.is_empty() { 0.0 } else { (judged.iter().sum::<f64>() / judged.len() as f64).sqrt() };
    let (level, peak_dbfs) = read_level(test, chain, tuning.loudness.measure);
    let tonal_findings = mic_findings(&m, ctx).into_iter().map(|f| f.id).filter(|id| TONAL_FINDINGS.contains(&id.as_str())).collect();
    RenderCheck { regions_db, tone_rms_db, sibilance_db: m.sibilance_db, plosive_events: m.plosive_events, level, peak_dbfs, tonal_findings }
}

/// Level of the read rendered on its own, measured the way `calibrate_makeup` steers it.
fn read_level(t: &MicTest, chain: &ProcessingChain, measure: LoudnessMeasure) -> (f64, f64) {
    let Some(seg) = t.segments.iter().find(|s| s.kind == SegmentKind::Read) else { return (f64::NEG_INFINITY, f64::NEG_INFINITY) };
    let r = t.range(seg);
    let read = AudioBuffer { sample_rate: t.audio.sample_rate, channels: t.audio.channels.iter().map(|c| c[r.clone()].to_vec()).collect() };
    let out = render(&read, chain);
    let x = &out.channels[0];
    let level = match measure {
        LoudnessMeasure::Lufs => dsp::integrated_lufs(x, out.sample_rate),
        LoudnessMeasure::Rms => dsp::level_db(x),
    };
    (level, dsp::db_amp(dsp::peak_abs(x)))
}

fn sibilance_after(test: &MicTest, chain: &ProcessingChain) -> Option<f64> {
    analyze(&render_test(test, chain)).sibilance_db
}

/// Region averages of a deviation curve, in `REGIONS` order.
pub fn region_values(dev: &[f64]) -> Vec<f64> {
    REGIONS.iter().map(|(_, lo, hi)| mean_over(dev, *lo, *hi)).collect()
}

fn mean_over(dev: &[f64], lo: f64, hi: f64) -> f64 {
    let v: Vec<f64> = BAND_CENTERS.iter().zip(dev).filter(|(f, _)| **f >= lo && **f <= hi).map(|(_, d)| *d).collect();
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn band_index(f: f64) -> usize {
    (0..BAND_CENTERS.len()).min_by(|a, b| (BAND_CENTERS[*a] / f).log2().abs().total_cmp(&(BAND_CENTERS[*b] / f).log2().abs())).unwrap()
}

/// Response (dB) of one EQ band at `f`, exactly as the renderer builds it.
pub fn band_response(b: &EqBand, sr: u32, f: f64) -> f64 {
    let q = match b.kind {
        EqKind::Peak => Biquad::peaking(sr, b.freq_hz, b.q, b.gain_db),
        EqKind::LowShelf => Biquad::low_shelf(sr, b.freq_hz, b.gain_db),
        EqKind::HighShelf => Biquad::high_shelf(sr, b.freq_hz, b.gain_db),
    };
    q.response_db(sr, f)
}

/// Summed response (dB) of all bands at `f`.
pub fn net_response(bands: &[EqBand], sr: u32, f: f64) -> f64 {
    bands.iter().map(|b| band_response(b, sr, f)).sum()
}

/// The published curve to use: the switch position's when given and published.
fn mic_curve(r: &MicResponse, switch: Option<&(String, String)>) -> Vec<[f64; 2]> {
    switch
        .and_then(|(k, o)| r.switch_curves.get(k).and_then(|m| m.get(o)))
        .filter(|c| !c.is_empty())
        .cloned()
        .unwrap_or_else(|| r.points_hz_db.clone())
}

fn interp(pts: &[[f64; 2]], f: f64) -> f64 {
    let r = MicResponse {
        points_hz_db: pts.to_vec(),
        reference: String::new(),
        measured_at_cm: None,
        pattern_b: None,
        proximity: None,
        switch_curves: Default::default(),
        source: String::new(),
        digitised_precision_db: None,
        verified: false,
    };
    r.db_at(f).unwrap_or(0.0)
}

/// M per `BAND_CENTERS`: -w x (published deviation re 1 kHz), smoothed to 2/3
/// octave, broad features only, within +-3 dB. Bands outside the chart's
/// 100 Hz-10 kHz get nothing.
pub fn mic_correction(pts: &[[f64; 2]], weight: f64) -> Vec<f64> {
    let n = BAND_CENTERS.len();
    if pts.is_empty() {
        return vec![0.0; n];
    }
    let (lo, hi) = (pts[0][0], pts[pts.len() - 1][0]);
    let inside = |f: f64| f >= lo.max(100.0) * 0.99 && f <= hi.min(10000.0) * 1.01;
    let ref1k = interp(pts, 1000.0);
    let dev: Vec<Option<f64>> = BAND_CENTERS.iter().map(|f| inside(*f).then(|| interp(pts, *f) - ref1k)).collect();
    // 2/3-octave smoothing: mean of the band and its neighbours.
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            if dev[i].is_none() {
                return 0.0;
            }
            let v: Vec<f64> = (i.saturating_sub(1)..=(i + 1).min(n - 1)).filter_map(|j| dev[j]).collect();
            v.iter().sum::<f64>() / v.len() as f64
        })
        .collect();
    // Broad features: runs of >= 3 bands (2/3 octave) of one sign at >= 2 dB.
    // Judged on the chart itself, so smoothing can't widen a narrow wiggle.
    let raw: Vec<f64> = dev.iter().map(|x| x.unwrap_or(0.0)).collect();
    let mut keep = vec![false; n];
    let mut i = 0;
    while i < n {
        let s = raw[i];
        if s.abs() < MIC_FEATURE_DB {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < n && raw[j + 1].abs() >= MIC_FEATURE_DB && raw[j + 1].signum() == s.signum() {
            j += 1;
        }
        if j - i + 1 >= 3 {
            keep[i..=j].iter_mut().for_each(|k| *k = true);
        }
        i = j + 1;
    }
    (0..n).map(|i| if keep[i] { (-weight * smooth[i]).clamp(-MIC_LIMIT_DB, MIC_LIMIT_DB) } else { 0.0 }).collect()
}

/// Predicted low-end change (dB at 100 Hz) between the near and far ends of
/// the coached distance range, from the idealised first-order model.
pub fn proximity_swing(r: &MicResponse, distance: Option<(f64, f64)>) -> Option<f64> {
    let (near, far) = distance?;
    let (p, b) = (r.proximity?, r.pattern_b?);
    Some(proximity_boost_db(p, b, 100.0, near.max(1.0)) - proximity_boost_db(p, b, 100.0, far.max(1.0)))
}

fn limit_low_end(curve: &mut [f64], limit: bool, no_boost: bool) {
    for (i, f) in BAND_CENTERS.iter().enumerate() {
        if *f < 250.0 {
            if limit {
                curve[i] = curve[i].clamp(-LOW_END_LIMIT_DB, LOW_END_LIMIT_DB);
            }
            if no_boost {
                curve[i] = curve[i].min(0.0);
            }
        }
    }
}

/// Deviation left after applying `bands`, normalised like the analyser
/// (mean 0 over 500 Hz-2 kHz).
fn residual_after(dev: &[f64], bands: &[EqBand], sr: u32) -> Vec<f64> {
    let mut r: Vec<f64> = BAND_CENTERS.iter().zip(dev).map(|(f, d)| d + net_response(bands, sr, *f)).collect();
    let norm = mean_over(&r, 500.0, 2000.0);
    r.iter_mut().for_each(|x| *x -= norm);
    r
}

/// Greedy fit of a target curve (per `BAND_CENTERS`) with at most `max` biquads:
/// largest-area run of the remaining error first, shelves at the ends, peaks
/// Q 0.7-1.4. Gains within +-`limit`.
pub fn fit_curve(target: &[f64], sr: u32, max: usize, limit: f64) -> Vec<EqBand> {
    let n = BAND_CENTERS.len();
    let mut bands: Vec<EqBand> = Vec::new();
    for _ in 0..max {
        let err: Vec<f64> = (0..n).map(|i| target[i] - net_response(&bands, sr, BAND_CENTERS[i])).collect();
        // Runs of one sign with |err| >= 0.75 dB.
        let mut best: Option<(usize, usize, f64)> = None;
        let mut i = 0;
        while i < n {
            if err[i].abs() < 0.75 {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < n && err[j + 1].abs() >= 0.75 && err[j + 1].signum() == err[i].signum() {
                j += 1;
            }
            let area: f64 = err[i..=j].iter().map(|e| e.abs()).sum();
            if j > i && best.is_none_or(|b| area > b.2) {
                best = Some((i, j, area));
            }
            i = j + 1;
        }
        let Some((a, b, _)) = best else { break };
        let p = (a..=b).max_by(|x, y| err[*x].abs().total_cmp(&err[*y].abs())).unwrap();
        let first = target.iter().position(|t| *t != 0.0).unwrap_or(0);
        let last = target.iter().rposition(|t| *t != 0.0).unwrap_or(n - 1);
        let reason = |g: f64| if g < 0.0 { "mic correction (less)" } else { "mic correction (more)" }.to_string();
        let mut band = if a <= first && b < n - 1 && b - a >= 2 && a <= 1 {
            EqBand { kind: EqKind::LowShelf, freq_hz: BAND_CENTERS[b] * 2f64.powf(1.0 / 6.0), gain_db: err[p], q: 0.7, reason: String::new() }
        } else if b >= last && b >= n - 2 && b - a >= 2 {
            EqBand { kind: EqKind::HighShelf, freq_hz: BAND_CENTERS[a] / 2f64.powf(1.0 / 6.0), gain_db: err[p], q: 0.7, reason: String::new() }
        } else {
            let bw = (b - a + 1) as f64 / 3.0;
            let q = (1.0 / (2.0 * (std::f64::consts::LN_2 / 2.0 * bw).sinh())).clamp(0.7, 1.4);
            let fc = 2f64.powf((a..=b).map(|k| BAND_CENTERS[k].log2() * err[k].abs()).sum::<f64>() / (a..=b).map(|k| err[k].abs()).sum::<f64>());
            EqBand { kind: EqKind::Peak, freq_hz: fc.round(), gain_db: err[p], q: (q * 100.0).round() / 100.0, reason: String::new() }
        };
        // Least-squares gain over the run (the response is close to linear in gain).
        for _ in 0..4 {
            let r: Vec<f64> = (a..=b).map(|k| band_response(&band, sr, BAND_CENTERS[k])).collect();
            let num: f64 = (a..=b).zip(&r).map(|(k, rk)| err[k] * rk).sum();
            let den: f64 = r.iter().map(|x| x * x).sum();
            if den <= 1e-9 {
                break;
            }
            band.gain_db = (band.gain_db * num / den).clamp(-limit, limit);
        }
        band.reason = reason(band.gain_db);
        if band.gain_db.abs() < 0.3 {
            break;
        }
        bands.push(band);
    }
    bands
}

/// Hold the summed mic-band response within +-`MIC_LIMIT_DB` on every
/// `CAP_FREQS` point (overlapping bands can add past the per-band limit):
/// scale all mic bands by one factor, found by bisection. True when scaled.
pub fn limit_mic_sum(bands: &mut [EqBand], sr: u32) -> bool {
    let freqs: Vec<f64> = CAP_FREQS.iter().copied().filter(|f| *f < 0.45 * sr as f64).collect();
    let within = |b: &[EqBand]| freqs.iter().all(|f| net_response(b, sr, *f).abs() <= MIC_LIMIT_DB - 0.05);
    if within(bands) {
        return false;
    }
    let orig: Vec<f64> = bands.iter().map(|b| b.gain_db).collect();
    let scale = |b: &mut [EqBand], s: f64| b.iter_mut().zip(&orig).for_each(|(x, g)| x.gain_db = g * s);
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        scale(bands, mid);
        if within(bands) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    scale(bands, lo);
    true
}

/// Keep at most `MAX_BANDS`: drop the smallest moves, voice first, then
/// profile, then mic.
fn limit_band_count(bands: &mut Vec<(EqBand, BandOrigin)>) {
    while bands.len() > MAX_BANDS {
        let rank = |o: BandOrigin| match o {
            BandOrigin::Voice => 0,
            BandOrigin::Profile => 1,
            BandOrigin::Mic => 2,
        };
        let i = (0..bands.len())
            .min_by(|a, b| {
                let ka = (rank(bands[*a].1), bands[*a].0.gain_db.abs() / bands[*a].0.q);
                let kb = (rank(bands[*b].1), bands[*b].0.gain_db.abs() / bands[*b].0.q);
                ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
            })
            .unwrap();
        bands.remove(i);
    }
}

/// Enforce the net cap on the evaluated response (with 0.1 dB margin for
/// rounding): scale voice bands down first, then profile, then mic. Returns
/// true when anything had to give.
pub fn cap_bands(bands: &mut [(EqBand, BandOrigin)], sr: u32) -> bool {
    let freqs: Vec<f64> = CAP_FREQS.iter().copied().filter(|f| *f < 0.45 * sr as f64).collect();
    let within = |b: &[(EqBand, BandOrigin)]| {
        let eq: Vec<EqBand> = b.iter().map(|x| x.0.clone()).collect();
        freqs.iter().all(|f| {
            (NET_CUT_LIMIT_DB + 0.1..=NET_BOOST_LIMIT_DB - 0.1).contains(&net_response(&eq, sr, *f))
        })
    };
    if within(bands) {
        return false;
    }
    for origin in [BandOrigin::Voice, BandOrigin::Profile, BandOrigin::Mic] {
        let orig: Vec<f64> = bands.iter().map(|x| x.0.gain_db).collect();
        let scale = |b: &mut [(EqBand, BandOrigin)], s: f64| {
            for (k, x) in b.iter_mut().enumerate() {
                if x.1 == origin {
                    x.0.gain_db = orig[k] * s;
                }
            }
        };
        scale(bands, 0.0);
        if !within(bands) {
            continue; // even without this origin it is over: it gives way fully
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            scale(bands, mid);
            if within(bands) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        scale(bands, lo);
        return true;
    }
    true
}

/// Today's HPF vs `f0_p10 / 1.5`, as loss at f0 p10 (2nd-order Butterworth
/// per 12 dB/oct stage: |H|^2 = r^4 / (1 + r^4)).
pub fn hpf_comparison(current_hz: f64, slope: u8, f0_p10: Option<f64>) -> HpfComparison {
    let stages = if slope >= 24 { 2.0 } else { 1.0 };
    let loss = |fc: f64, f: f64| {
        let r4 = (f / fc).powi(4);
        stages * 10.0 * (r4 / (1.0 + r4)).log10()
    };
    let proposed = f0_p10.map(|p| (p / 1.5).round());
    HpfComparison {
        current_hz,
        proposed_hz: proposed,
        f0_p10_hz: f0_p10,
        current_loss_db: f0_p10.map(|p| loss(current_hz, p)),
        proposed_loss_db: f0_p10.zip(proposed).map(|(p, h)| loss(h, p)),
    }
}

fn trunc1(x: f64) -> f64 {
    (x * 10.0).trunc() / 10.0
}

/// Loudness-matched copy of `b` at the level of `a` (for the Before / After
/// toggle: "after" must not win by being louder).
pub fn loudness_match(a: &AudioBuffer, b: &AudioBuffer) -> AudioBuffer {
    let mono = |x: &AudioBuffer| x.channels.first().cloned().unwrap_or_default();
    let (la, lb) = (dsp::integrated_lufs(&mono(a), a.sample_rate), dsp::integrated_lufs(&mono(b), b.sample_rate));
    if !la.is_finite() || !lb.is_finite() {
        return b.clone();
    }
    let g = dsp::from_db(la - lb) as f32;
    AudioBuffer { sample_rate: b.sample_rate, channels: b.channels.iter().map(|c| c.iter().map(|v| v * g).collect()).collect() }
}
