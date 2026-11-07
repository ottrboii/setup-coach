//! Voice-type measurement: what kind of voice is this, as far as the read
//! segment of a guided test can tell (docs/voice-tuning-design.md, section 3.1).
//!
//! Pitch (median, p10, p90 over voiced frames), spectral tilt, "s" level and
//! peak frequency and plosive rate, plus a confidence. Plain signal
//! measurement only; nothing here changes the processing chain. When the test
//! holds too little speech the result says so (`Confidence::Insufficient`, all
//! numbers `None`) instead of guessing.

use super::analyze::AudioMetrics;
use super::dsp::{self, BAND_CENTERS, Biquad};
use super::{MicTest, analyze};
use serde::{Deserialize, Serialize};

/// Pitch class breakpoints (median f0, Hz). They reuse the HPF table in
/// `chain::compute_chain` (110/150/200 Hz), so no new threshold.
pub const PITCH_DEEP_BELOW_HZ: f64 = 110.0;
pub const PITCH_LOWMID_BELOW_HZ: f64 = 150.0;
pub const PITCH_HIGHMID_BELOW_HZ: f64 = 200.0;
/// Tilt display classes (dB/octave residual vs the average-speech curve).
/// Placeholder (design section 4): wording only, nothing is tuned from them.
pub const TILT_CLASS_DB_OCT: f64 = 1.5;
/// "s" classes on `sibilance_db`. Strong reuses the de-esser's -3 dB rule in
/// `chain.rs`; the Soft cut-off is a placeholder (wording only).
pub const SIB_STRONG_ABOVE_DB: f64 = -3.0;
pub const SIB_SOFT_BELOW_DB: f64 = -20.0;
/// Plosive classes in events per minute of the plosive-rich read (placeholder).
pub const PLOSIVES_MANY_PER_MIN: f64 = 6.0;

/// Less voiced speech than this and we do not report anything (seconds).
pub const MIN_VOICED_S: f64 = 1.5;
/// Below this the numbers are given but flagged `Low` (seconds of voiced frames).
/// The design asks for 8 s of speech; the in-app read is ~10 s and only about
/// half of it is voiced, so the bar is on voiced seconds, not total speech.
pub const GOOD_VOICED_S: f64 = 2.5;
/// Below this SNR (dB) the spectrum is mostly noise and the tilt is not trusted.
pub const MIN_SNR_DB: f64 = 15.0;
/// A tilt band must stand this far (dB) above the silence-step noise in that band.
pub const MIN_BAND_SNR_DB: f64 = 10.0;
/// Below this upper bandwidth (Hz) the 250 Hz - 8 kHz tilt is not meaningful.
pub const MIN_BANDWIDTH_HZ: f64 = 7000.0;
/// Above this share of clipped samples the spectrum is distorted.
pub const MAX_CLIPPED_FRACTION: f64 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PitchClass {
    Deep,
    LowMid,
    HighMid,
    High,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TiltClass {
    Dark,
    Neutral,
    Bright,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SibClass {
    Soft,
    Normal,
    Strong,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlosiveClass {
    None,
    Some,
    Many,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Not enough speech to say anything: every number is `None`.
    #[default]
    Insufficient,
    /// Numbers are given but a voice-based move should not use them.
    Low,
    Good,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceReason {
    TooLittleSpeech,
    Noisy,
    BandLimited,
    Clipped,
}

/// What the read says about the voice. Classes are for plain-language display;
/// later pieces use the continuous values.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceType {
    pub f0_median_hz: Option<f64>,
    pub f0_p10_hz: Option<f64>,
    pub f0_p90_hz: Option<f64>,
    pub pitch: PitchClass,
    /// Slope (dB per octave, 250 Hz - 8 kHz) of the deviation from the average
    /// speech curve, after removing the optional mic correction. 0 = average voice.
    pub tilt_db_per_oct: Option<f64>,
    pub tilt: TiltClass,
    /// Same as `AudioMetrics::sibilance_db`.
    pub sib_level_db: Option<f64>,
    /// Spectral peak of the "s" frames, 4-10 kHz.
    pub sib_peak_hz: Option<f64>,
    pub sibilance: SibClass,
    pub plosives_per_min: Option<f64>,
    pub plosives: PlosiveClass,
    /// Seconds of voiced speech the pitch statistics rest on.
    pub voiced_s: f64,
    pub confidence: Confidence,
    /// Why confidence is not `Good`.
    pub confidence_reason: Option<ConfidenceReason>,
}

/// What `analyze` already knows and the voice measurement reuses.
pub struct VoiceInputs<'a> {
    pub sr: u32,
    pub noise_floor_dbfs: f64,
    pub snr_db: f64,
    pub clipped_fraction: f64,
    pub upper_bandwidth_hz: f64,
    pub band_deviation_db: &'a [f64],
    /// Speech and silence third-octave levels (dB, same scale), to see which bands are noise.
    pub band_levels_db: &'a [f64],
    pub noise_band_db: Option<&'a [f64]>,
    pub sibilance_db: Option<f64>,
    pub plosive_events: usize,
}

/// Measure the voice in a mono read. `correction` is a known mic response
/// (Hz, dB re 1 kHz, as published, already weighted by the caller) that is
/// removed from the tilt; `None` for now.
pub fn measure_read(read: &[f32], inp: &VoiceInputs, correction: Option<&[(f64, f64)]>) -> VoiceType {
    let sr = inp.sr;
    let gate = noise_gate_dbfs(read, sr, inp.noise_floor_dbfs);
    let f0s = track_f0(read, sr, gate);
    let voiced_s = f0s.len() as f64 * F0_HOP_S;
    let mut v = VoiceType { voiced_s, ..VoiceType::default() };
    if voiced_s < MIN_VOICED_S {
        // Speech drowned in noise looks like "no speech": say which it is.
        let reason = if inp.snr_db < MIN_SNR_DB { ConfidenceReason::Noisy } else { ConfidenceReason::TooLittleSpeech };
        v.set_confidence(Confidence::Insufficient, Some(reason));
        return v;
    }
    let med = dsp::median(&f0s);
    v.f0_median_hz = Some(med);
    v.f0_p10_hz = Some(dsp::percentile(&f0s, 10.0));
    v.f0_p90_hz = Some(dsp::percentile(&f0s, 90.0));
    v.pitch = pitch_class(med);
    let noise_limited = tilt_bands_in_noise(inp.band_levels_db, inp.noise_band_db);

    let bandlimited = inp.upper_bandwidth_hz < MIN_BANDWIDTH_HZ;
    if !bandlimited {
        retilt(&mut v, inp.band_deviation_db, correction);
        v.sib_peak_hz = sibilant_peak(read, sr, gate);
    }
    v.sib_level_db = inp.sibilance_db;
    v.sibilance = sib_class(inp.sibilance_db);
    let minutes = read.len() as f64 / sr as f64 / 60.0;
    if minutes > 0.0 {
        let per_min = inp.plosive_events as f64 / minutes;
        v.plosives_per_min = Some(per_min);
        v.plosives = plosive_class(per_min);
    }

    let (conf, reason) = if bandlimited {
        (Confidence::Low, Some(ConfidenceReason::BandLimited))
    } else if inp.clipped_fraction > MAX_CLIPPED_FRACTION {
        (Confidence::Low, Some(ConfidenceReason::Clipped))
    } else if inp.snr_db < MIN_SNR_DB || noise_limited {
        (Confidence::Low, Some(ConfidenceReason::Noisy))
    } else if voiced_s < GOOD_VOICED_S {
        (Confidence::Low, Some(ConfidenceReason::TooLittleSpeech))
    } else {
        (Confidence::Good, None)
    };
    v.set_confidence(conf, reason);
    v
}

/// Measure a guided test, optionally with a known mic correction removed from
/// the tilt. Runs the full analysis; use `AudioMetrics::voice` when it already ran.
pub fn measure(test: &MicTest, correction: Option<&[(f64, f64)]>) -> VoiceType {
    let m = analyze(test);
    let mut v = m.voice.clone();
    if correction.is_some() {
        remove_mic_correction(&mut v, &m, correction);
    }
    v
}

/// Recompute the tilt of an already measured voice with a mic correction
/// removed (the later composer step: M known before R is read).
pub fn remove_mic_correction(v: &mut VoiceType, m: &AudioMetrics, correction: Option<&[(f64, f64)]>) {
    if v.tilt_db_per_oct.is_some() {
        retilt(v, &m.band_deviation_db, correction);
    }
}

fn retilt(v: &mut VoiceType, band_deviation_db: &[f64], correction: Option<&[(f64, f64)]>) {
    v.tilt_db_per_oct = tilt_db_per_oct(band_deviation_db, correction);
    v.tilt = v.tilt_db_per_oct.map_or(TiltClass::Unknown, tilt_class);
}

impl VoiceType {
    fn set_confidence(&mut self, c: Confidence, r: Option<ConfidenceReason>) {
        self.confidence = c;
        self.confidence_reason = r;
    }
}

/// True if any tilt band (250 Hz - 8 kHz) is less than MIN_BAND_SNR_DB above the
/// noise in the silence step: the slope would partly measure the noise.
fn tilt_bands_in_noise(band_levels_db: &[f64], noise_band_db: Option<&[f64]>) -> bool {
    let Some(noise) = noise_band_db else { return false };
    if noise.len() != BAND_CENTERS.len() || band_levels_db.len() != BAND_CENTERS.len() {
        return false;
    }
    (0..BAND_CENTERS.len())
        .filter(|i| (250.0..=8000.0).contains(&BAND_CENTERS[*i]))
        .any(|i| band_levels_db[i] - noise[i] < MIN_BAND_SNR_DB)
}

pub fn pitch_class(f0: f64) -> PitchClass {
    if f0 < PITCH_DEEP_BELOW_HZ {
        PitchClass::Deep
    } else if f0 < PITCH_LOWMID_BELOW_HZ {
        PitchClass::LowMid
    } else if f0 < PITCH_HIGHMID_BELOW_HZ {
        PitchClass::HighMid
    } else {
        PitchClass::High
    }
}

pub fn tilt_class(db_oct: f64) -> TiltClass {
    if db_oct <= -TILT_CLASS_DB_OCT {
        TiltClass::Dark
    } else if db_oct >= TILT_CLASS_DB_OCT {
        TiltClass::Bright
    } else {
        TiltClass::Neutral
    }
}

fn sib_class(sibilance_db: Option<f64>) -> SibClass {
    match sibilance_db {
        None => SibClass::Unknown,
        Some(s) if s > SIB_STRONG_ABOVE_DB => SibClass::Strong,
        Some(s) if s < SIB_SOFT_BELOW_DB => SibClass::Soft,
        Some(_) => SibClass::Normal,
    }
}

fn plosive_class(per_min: f64) -> PlosiveClass {
    if per_min <= 0.0 {
        PlosiveClass::None
    } else if per_min < PLOSIVES_MANY_PER_MIN {
        PlosiveClass::Some
    } else {
        PlosiveClass::Many
    }
}

/// Correction value (dB) at `f`: linear in log-frequency, flat beyond the ends.
fn correction_at(points: &[(f64, f64)], f: f64) -> f64 {
    let mut p: Vec<(f64, f64)> = points.iter().copied().filter(|(hz, db)| *hz > 0.0 && db.is_finite()).collect();
    if p.is_empty() {
        return 0.0;
    }
    p.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if f <= p[0].0 {
        return p[0].1;
    }
    if f >= p[p.len() - 1].0 {
        return p[p.len() - 1].1;
    }
    let i = p.iter().position(|(hz, _)| *hz >= f).unwrap();
    let ((f0, d0), (f1, d1)) = (p[i - 1], p[i]);
    let t = (f / f0).ln() / (f1 / f0).ln();
    d0 + t * (d1 - d0)
}

/// Least-squares slope (dB per octave) of the band deviation over
/// 250 Hz - 8 kHz, after subtracting a known mic correction. The correction is
/// normalised like `band_deviation_db` (zero mean over 500 Hz - 2 kHz) so a
/// curve published "re 1 kHz" lines up. `None` if the deviation is not the
/// 22 third-octave bands.
pub fn tilt_db_per_oct(band_deviation_db: &[f64], correction: Option<&[(f64, f64)]>) -> Option<f64> {
    if band_deviation_db.len() != BAND_CENTERS.len() {
        return None;
    }
    let corr: Vec<f64> = BAND_CENTERS.iter().map(|fc| correction.map_or(0.0, |c| correction_at(c, *fc))).collect();
    let norm_idx: Vec<usize> = (0..BAND_CENTERS.len()).filter(|i| (500.0..=2000.0).contains(&BAND_CENTERS[*i])).collect();
    let norm = norm_idx.iter().map(|i| corr[*i]).sum::<f64>() / norm_idx.len() as f64;
    let pts: Vec<(f64, f64)> = (0..BAND_CENTERS.len())
        .filter(|i| (250.0..=8000.0).contains(&BAND_CENTERS[*i]))
        .map(|i| (BAND_CENTERS[i].log2(), band_deviation_db[i] - (corr[i] - norm)))
        .collect();
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if sxx <= 0.0 {
        return None;
    }
    Some(pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx)
}

// --- Pitch tracker -------------------------------------------------------------

const F0_WIN_S: f64 = 0.04;
const F0_HOP_S: f64 = 0.02;
const F0_MIN_HZ: f64 = 60.0;
const F0_MAX_HZ: f64 = 400.0;
/// Normalised autocorrelation a frame needs to count as voiced.
const F0_MIN_PERIODICITY: f64 = 0.6;
/// A later (shorter-lag) peak this close to the best one wins: avoids picking a
/// sub-octave multiple of the period.
const F0_OCTAVE_PEAK_RATIO: f64 = 0.85;
/// A voiced frame carries at least this share of its power below 1 kHz (dB re
/// the full-band frame). "s" and noise frames fail it however well their
/// leftover low end correlates.
const F0_MIN_LOW_BAND_DB: f64 = -10.0;
/// Pitch may move at most this much between neighbouring frames (20 ms) inside
/// one voiced run; a bigger jump starts a new run.
const F0_MAX_STEP: f64 = 0.2;
/// Voiced runs shorter than this many frames (60 ms) are tracker slips.
const F0_MIN_RUN: usize = 3;
/// Frames further than this factor from the median pitch are tracker slips
/// (octave jumps, plosive thumps) and are dropped.
const F0_OUTLIER_FACTOR: f64 = 1.5;

/// Level (dBFS) a frame must reach to be looked at: well above the noise floor
/// and within 30 dB of the loud vowels.
fn noise_gate_dbfs(x: &[f32], sr: u32, noise_floor_dbfs: f64) -> f64 {
    let win = ((sr as f64 * F0_WIN_S) as usize).max(1);
    let levels: Vec<f64> = x.chunks(win).filter(|c| c.len() == win).map(dsp::level_db).collect();
    if levels.is_empty() {
        return f64::INFINITY;
    }
    (noise_floor_dbfs + 12.0).max(dsp::percentile(&levels, 90.0) - 30.0)
}

/// Pitch (Hz) of every voiced 40 ms frame (20 ms hop) above `gate_dbfs`:
/// normalised autocorrelation on a 4x decimated 60-1000 Hz band, first strong
/// peak (no sub-octave picks), parabolic refinement, then slips removed against
/// the median. Empty if nothing is voiced.
pub fn track_f0(x: &[f32], sr: u32, gate_dbfs: f64) -> Vec<f64> {
    let band = dsp::filtered(
        x,
        &[Biquad::highpass(sr, 50.0, 0.707), Biquad::lowpass(sr, 1000.0, 0.707), Biquad::lowpass(sr, 1000.0, 0.707)],
    );
    let dec: Vec<f64> = band.chunks(4).map(|c| c.iter().map(|v| *v as f64).sum::<f64>() / c.len() as f64).collect();
    let dsr = sr as f64 / 4.0;
    let win = (dsr * F0_WIN_S) as usize;
    let hop = ((dsr * F0_HOP_S) as usize).max(1);
    let min_lag = (dsr / F0_MAX_HZ).floor().max(2.0) as usize;
    let max_lag = (dsr / F0_MIN_HZ).ceil() as usize;
    let full_win = win * 4;
    let mut found: Vec<(usize, f64)> = Vec::new();
    if dec.len() < win + max_lag + 2 {
        return Vec::new();
    }
    let mut corr = vec![0.0f64; max_lag + 2];
    let mut start = 0usize;
    while start + win + max_lag + 1 < dec.len() {
        let full = x.get(start * 4..start * 4 + full_win).unwrap_or(&[]);
        if !full.is_empty() && dsp::level_db(full) >= gate_dbfs {
            let seg = &dec[start..start + win];
            let e0: f64 = seg.iter().map(|v| v * v).sum();
            let full_ms: f64 = full.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / full.len() as f64;
            let low_ok = e0 / win as f64 >= full_ms * 10f64.powf(F0_MIN_LOW_BAND_DB / 10.0);
            if e0 > 1e-12 && low_ok {
                let mut best = min_lag;
                for lag in min_lag..=max_lag + 1 {
                    let other = &dec[start + lag..start + lag + win];
                    let num: f64 = seg.iter().zip(other).map(|(a, b)| a * b).sum();
                    let e1: f64 = other.iter().map(|v| v * v).sum();
                    corr[lag] = num / (e0 * e1).sqrt().max(1e-12);
                    if lag <= max_lag && corr[lag] > corr[best] {
                        best = lag;
                    }
                }
                if corr[best] >= F0_MIN_PERIODICITY {
                    let mut chosen = best;
                    for lag in min_lag + 1..best {
                        if corr[lag] >= F0_OCTAVE_PEAK_RATIO * corr[best] && corr[lag] >= corr[lag - 1] && corr[lag] >= corr[lag + 1] {
                            chosen = lag;
                            break;
                        }
                    }
                    // Parabolic interpolation of the peak position.
                    let (a, b, c) = (corr[chosen - 1], corr[chosen], corr[chosen + 1]);
                    let denom = a - 2.0 * b + c;
                    let shift = if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
                    found.push((start / hop, dsr / (chosen as f64 + shift)));
                }
            }
        }
        start += hop;
    }
    // Keep only runs of consecutive frames whose pitch moves smoothly.
    let mut out = Vec::new();
    let mut run: Vec<f64> = Vec::new();
    let mut last: Option<(usize, f64)> = None;
    for &(i, f) in &found {
        let continues = last.is_some_and(|(li, lf)| li + 1 == i && (f / lf - 1.0).abs() <= F0_MAX_STEP);
        if !continues {
            if run.len() >= F0_MIN_RUN {
                out.append(&mut run);
            }
            run.clear();
        }
        run.push(f);
        last = Some((i, f));
    }
    if run.len() >= F0_MIN_RUN {
        out.append(&mut run);
    }
    if out.len() >= 5 {
        let m = dsp::median(&out);
        out.retain(|f| *f <= m * F0_OUTLIER_FACTOR && *f >= m / F0_OUTLIER_FACTOR);
    }
    out
}

// --- Sibilance peak ------------------------------------------------------------

/// Share of a frame's power that must lie at 4-10 kHz for it to count as an "s".
const SIB_MIN_HF_SHARE: f64 = 0.5;

/// Peak frequency of the "s" sounds: average spectrum of the loud frames whose
/// 4-10 kHz energy exceeds their 300 Hz - 3 kHz energy, smoothed over about
/// +-250 Hz, peak between 4 and 10 kHz. `None` below 22.05 kHz sample rate or
/// with fewer than 5 such frames (a voice that barely hisses).
pub fn sibilant_peak(x: &[f32], sr: u32, gate_dbfs: f64) -> Option<f64> {
    if sr < 22050 {
        return None;
    }
    let n = if sr > 64000 { 2048 } else { 1024 };
    let hop = n / 2;
    let mut acc: Vec<f64> = Vec::new();
    let mut count = 0usize;
    let mut bin_hz = 0.0;
    let mut start = 0usize;
    while start + n <= x.len() {
        let frame = &x[start..start + n];
        if dsp::level_db(frame) >= gate_dbfs {
            let spec = dsp::welch(frame, sr, n);
            let hf = spec.band_power(4000.0, 10000.0);
            let total: f64 = spec.power.iter().sum();
            // An "s" frame: 4-10 kHz beats 300 Hz - 3 kHz and holds most of the frame's power.
            if hf > spec.band_power(300.0, 3000.0) && hf > SIB_MIN_HF_SHARE * total {
                if acc.is_empty() {
                    acc = vec![0.0; spec.power.len()];
                    bin_hz = spec.bin_hz;
                }
                for (a, p) in acc.iter_mut().zip(&spec.power) {
                    *a += p;
                }
                count += 1;
            }
        }
        start += hop;
    }
    if count < 5 {
        return None;
    }
    let half = ((250.0 / bin_hz).round() as usize).max(1);
    let lo = (4000.0 / bin_hz).ceil() as usize;
    let hi = ((10000.0f64.min(sr as f64 * 0.45)) / bin_hz).floor() as usize;
    let mut best = (lo, f64::NEG_INFINITY);
    for k in lo..=hi.min(acc.len() - 1) {
        let a = k.saturating_sub(half);
        let b = (k + half + 1).min(acc.len());
        let avg = acc[a..b].iter().sum::<f64>() / (b - a) as f64;
        if avg > best.1 {
            best = (k, avg);
        }
    }
    Some(best.0 as f64 * bin_hz)
}
