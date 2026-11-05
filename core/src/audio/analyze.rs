//! Turn a recorded guided test into measurements.

use super::dsp::{self, Biquad, BAND_CENTERS, Spectrum};
use super::vibration::{self, VibrationInfo};
use super::{MicTest, SegmentKind};
use serde::{Deserialize, Serialize};

/// Approximate long-term average speech spectrum (third-octave, dB, arbitrary
/// reference), after Byrne et al. 1994 (combined male/female). Used as the
/// "natural voice balance" target. To be validated by an audio engineer.
pub const REFERENCE_LTASS: [f64; 22] = [
    43.5, 54.4, 57.7, 56.8, 58.2, 59.7, 60.0, 62.4, 62.6, 59.2, 55.4, 53.3, 50.8, 47.6, 48.0, 47.4,
    46.4, 45.2, 44.0, 42.4, 40.6, 38.6,
];

/// Reference curve adjusted for the speaker's pitch: deep voices naturally
/// carry more energy at 100–160 Hz, high voices much less below 160 Hz.
pub fn reference_for_f0(f0: Option<f64>) -> Vec<f64> {
    let mut r = REFERENCE_LTASS.to_vec();
    if let Some(f0) = f0 {
        for (i, fc) in BAND_CENTERS.iter().enumerate() {
            if f0 < 130.0 && (100.0..=160.0).contains(fc) {
                r[i] += 3.0;
            }
            if f0 > 180.0 && *fc <= 125.0 {
                r[i] -= 6.0;
            }
        }
    }
    r
}

/// Average deviation (dB) from the reference in a few tonal regions.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ToneSummary {
    /// 80–160 Hz (boom, proximity effect).
    pub boom_db: f64,
    /// 125–250 Hz (body / warmth).
    pub body_db: f64,
    /// 200–500 Hz (mud).
    pub mud_db: f64,
    /// 400–800 Hz (boxiness).
    pub box_db: f64,
    /// 800–1600 Hz (nasal / honk).
    pub nasal_db: f64,
    /// 2–5 kHz (presence; + harsh, − dull).
    pub presence_db: f64,
    /// 6.3–10 kHz (air).
    pub air_db: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseCharacter {
    /// Flat/high-frequency heavy: preamp or electrical hiss.
    Hiss,
    /// Low-frequency heavy: fan, AC, traffic rumble.
    Rumble,
    /// Neither dominates.
    Broadband,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HumInfo {
    /// 50 or 60.
    pub mains_hz: f64,
    /// Prominence of the strongest mains harmonic over neighbouring noise (dB).
    pub prominence_db: f64,
    /// Level of the hum components (dBFS).
    pub level_dbfs: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReflectionInfo {
    /// Delay of the reflected copy (ms).
    pub delay_ms: f64,
    /// Extra path length of the reflection (cm), e.g. mic → desk → mic.
    pub path_cm: f64,
    /// Peak-to-peak comb ripple in the spectrum (dB).
    pub ripple_db: f64,
}

/// All measurements from one test. Levels are dBFS unless stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioMetrics {
    pub sample_rate: u32,
    pub channels: usize,
    /// Essentially digital silence (muted, wrong input, no phantom power).
    pub no_signal: bool,
    /// RMS of each channel during the read.
    pub channel_levels_db: Vec<f64>,
    /// Index of a silent channel while another carries the voice.
    pub dead_channel: Option<usize>,
    /// Correlation between L and R when both are live (≈1 = mono mic).
    pub stereo_correlation: Option<f64>,
    pub dc_offset: f64,
    pub noise_floor_dbfs: f64,
    pub noise_character: NoiseCharacter,
    /// Median level of active speech frames during the read.
    pub speech_rms_dbfs: f64,
    /// Typical speech peaks (95th percentile of 20 ms frame peaks).
    pub speech_peak_dbfs: f64,
    /// Quiet speech (10th percentile of active frames).
    pub quiet_speech_dbfs: f64,
    /// Loudness of the read (BS.1770).
    pub speech_lufs: f64,
    pub snr_db: f64,
    pub loud_peak_dbfs: Option<f64>,
    /// Share of samples at (or stuck near) full scale.
    pub clipped_fraction: f64,
    pub f0_hz: Option<f64>,
    pub band_levels_db: Vec<f64>,
    /// Deviation from the pitch-adjusted reference, normalised on 500 Hz–2 kHz.
    pub band_deviation_db: Vec<f64>,
    pub tone: ToneSummary,
    /// Highest frequency the voice meaningfully reaches.
    pub upper_bandwidth_hz: f64,
    pub hum: Option<HumInfo>,
    /// Decay time estimated from syllable endings (s). ~0.1–0.2 dry, >0.45 roomy.
    pub reverb_decay_s: Option<f64>,
    pub reflection: Option<ReflectionInfo>,
    pub plosive_events: usize,
    /// Worst low-frequency burst above the speech level (dB).
    pub plosive_worst_db: Option<f64>,
    /// Loudest "s" energy (5–9 kHz) relative to the speech level (dB).
    pub sibilance_db: Option<f64>,
    /// Typing level relative to speech (dB).
    pub typing_rel_db: Option<f64>,
    /// Desk-tap thump peak relative to speech peaks (dB).
    pub tap_rel_db: Option<f64>,
    /// Desk / boom-arm vibration reaching the mic (taps, thumps, arm touching the desk).
    #[serde(default)]
    pub vibration: VibrationInfo,
    pub dropouts: usize,
    pub crackles: usize,
    /// How much quieter you get while moving (dB).
    pub moving_drop_db: Option<f64>,
    /// Extra spread of levels while moving vs the read (p90 − p10 difference, dB).
    pub moving_spread_db: Option<f64>,
    /// Loud-segment level minus read level (dB). Near zero suggests auto-level.
    pub loud_vs_read_db: Option<f64>,
    /// Noise in speech pauses minus the silence-step noise (dB): gain riding.
    pub pause_floor_rise_db: Option<f64>,
}

fn segment_mono(test: &MicTest, kind: SegmentKind, live: &[usize]) -> Option<Vec<f32>> {
    let mut out = Vec::new();
    for seg in test.segments.iter().filter(|s| s.kind == kind) {
        let r = test.range(seg);
        for i in r {
            let mut acc = 0.0f32;
            for &c in live {
                acc += test.audio.channels[c][i];
            }
            out.push(acc / live.len() as f32);
        }
    }
    if out.len() < (test.audio.sample_rate as usize / 10) { None } else { Some(out) }
}

fn channel_segment(test: &MicTest, kind: SegmentKind, ch: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for seg in test.segments.iter().filter(|s| s.kind == kind) {
        out.extend_from_slice(&test.audio.channels[ch][test.range(seg)]);
    }
    out
}

/// Analyse a guided test.
pub fn analyze(test: &MicTest) -> AudioMetrics {
    let sr = test.audio.sample_rate;
    let nch = test.audio.channels.len().max(1);
    let all_peak = test.audio.channels.iter().map(|c| dsp::peak_abs(c)).fold(0.0, f64::max);
    let no_signal = all_peak < 1e-4;

    // --- Channels ---------------------------------------------------------
    let read_kind = if test.has(SegmentKind::Read) { SegmentKind::Read } else { SegmentKind::Loud };
    let channel_levels_db: Vec<f64> = (0..test.audio.channels.len())
        .map(|c| dsp::level_db(&channel_segment(test, read_kind, c)))
        .collect();
    let max_ch = channel_levels_db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut dead_channel = None;
    let mut live: Vec<usize> = Vec::new();
    for (c, l) in channel_levels_db.iter().enumerate() {
        if nch > 1 && *l < max_ch - 30.0 {
            dead_channel = Some(c);
        } else {
            live.push(c);
        }
    }
    if live.is_empty() {
        live.push(0);
    }
    let stereo_correlation = if live.len() >= 2 {
        let a = channel_segment(test, read_kind, live[0]);
        let b = channel_segment(test, read_kind, live[1]);
        let num: f64 = a.iter().zip(&b).map(|(x, y)| *x as f64 * *y as f64).sum();
        let den = (dsp::mean_square(&a) * dsp::mean_square(&b)).sqrt() * a.len() as f64;
        Some(num / den.max(1e-20))
    } else {
        None
    };

    let mono_all: Vec<f32> = (0..test.audio.len())
        .map(|i| live.iter().map(|&c| test.audio.channels[c][i]).sum::<f32>() / live.len() as f32)
        .collect();
    let dc_offset = mono_all.iter().map(|v| *v as f64).sum::<f64>() / mono_all.len().max(1) as f64;
    let clipped = mono_all.iter().filter(|v| v.abs() >= 0.999).count();
    let clipped_fraction = clipped as f64 / mono_all.len().max(1) as f64;

    // --- Segments -----------------------------------------------------------
    let read = segment_mono(test, read_kind, &live).unwrap_or_else(|| mono_all.clone());
    let silence = segment_mono(test, SegmentKind::Silence, &live);

    let read_frames = dsp::frames(&read, sr, 20.0);
    let noise_floor_dbfs = match &silence {
        Some(s) => dsp::level_db(s).max(-140.0),
        None => dsp::percentile(&read_frames.rms_db, 5.0).max(-140.0),
    };
    let active_thresh = (noise_floor_dbfs + 15.0).max(-70.0);
    let active_rms: Vec<f64> = read_frames.rms_db.iter().copied().filter(|l| *l > active_thresh).collect();
    let active_peak: Vec<f64> = read_frames
        .rms_db
        .iter()
        .zip(&read_frames.peak_db)
        .filter(|(r, _)| **r > active_thresh)
        .map(|(_, p)| *p)
        .collect();
    let speech_rms_dbfs = if active_rms.is_empty() { dsp::level_db(&read) } else { dsp::median(&active_rms) };
    let speech_peak_dbfs = if active_peak.is_empty() {
        dsp::db_amp(dsp::peak_abs(&read))
    } else {
        dsp::percentile(&active_peak, 95.0)
    };
    let quiet_speech_dbfs = if active_rms.is_empty() { speech_rms_dbfs } else { dsp::percentile(&active_rms, 10.0) };
    let snr_db = speech_rms_dbfs - noise_floor_dbfs;
    let speech_lufs = dsp::integrated_lufs(&read, sr);

    let loud = segment_mono(test, SegmentKind::Loud, &live);
    let loud_peak_dbfs = loud.as_ref().map(|l| dsp::db_amp(dsp::peak_abs(l)));
    let loud_vs_read_db = loud.as_ref().map(|l| {
        let lf = dsp::frames(l, sr, 20.0);
        dsp::percentile(&lf.rms_db, 90.0) - dsp::percentile(&read_frames.rms_db, 90.0)
    });

    // --- Noise character & hum --------------------------------------------
    let (noise_character, hum) = match &silence {
        Some(s) => {
            let spec = dsp::welch(s, sr, 16384);
            let low = spec.band_db(100.0, 500.0) - 10.0 * (400.0f64).log10();
            let high = spec.band_db(4000.0, 8000.0) - 10.0 * (4000.0f64).log10();
            // Compare per-Hz density.
            let character = if high > low - 6.0 {
                NoiseCharacter::Hiss
            } else if low > high + 20.0 {
                NoiseCharacter::Rumble
            } else {
                NoiseCharacter::Broadband
            };
            (character, detect_hum(&spec))
        }
        None => (NoiseCharacter::Broadband, None),
    };

    // --- Tone ------------------------------------------------------------------
    let f0_hz = dsp::estimate_f0(&read, sr);
    let spec = dsp::welch(&read, sr, 8192);
    let band_levels_db = dsp::third_octave_db(&spec);
    let reference = reference_for_f0(f0_hz);
    let raw_dev: Vec<f64> = band_levels_db.iter().zip(&reference).map(|(l, r)| l - r).collect();
    let norm_idx: Vec<usize> = (0..BAND_CENTERS.len()).filter(|i| (500.0..=2000.0).contains(&BAND_CENTERS[*i])).collect();
    let norm = norm_idx.iter().map(|i| raw_dev[*i]).sum::<f64>() / norm_idx.len() as f64;
    let band_deviation_db: Vec<f64> = raw_dev.iter().map(|d| d - norm).collect();
    let region = |lo: f64, hi: f64| {
        let v: Vec<f64> = BAND_CENTERS
            .iter()
            .zip(&band_deviation_db)
            .filter(|(fc, _)| **fc >= lo && **fc <= hi)
            .map(|(_, d)| *d)
            .collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let tone = ToneSummary {
        boom_db: region(80.0, 160.0),
        body_db: region(125.0, 250.0),
        mud_db: region(200.0, 500.0),
        box_db: region(400.0, 800.0),
        nasal_db: region(800.0, 1600.0),
        presence_db: region(2000.0, 5000.0),
        air_db: region(6300.0, 10000.0),
    };

    // Bandwidth: highest band not collapsed far below the reference curve
    // (a codec/low sample rate shows as a cliff; hiss or dull mics don't).
    let mut upper_bandwidth_hz = BAND_CENTERS[0];
    for (i, fc) in BAND_CENTERS.iter().enumerate() {
        if band_deviation_db[i] > -20.0 {
            upper_bandwidth_hz = *fc;
        }
    }
    if (sr as f64) / 2.0 < upper_bandwidth_hz * 1.1 {
        upper_bandwidth_hz = upper_bandwidth_hz.min(sr as f64 / 2.0);
    }

    // --- Room & reflections -------------------------------------------------
    let reverb_decay_s = estimate_decay(&read, sr, noise_floor_dbfs, speech_rms_dbfs);
    let reflection = detect_reflection(&spec, f0_hz);

    // --- Plosives & sibilance ----------------------------------------------
    let (plosive_events, plosive_worst_db) = plosives(&read, sr, speech_rms_dbfs);
    let sibilance_db = sibilance(&read, sr, speech_rms_dbfs, noise_floor_dbfs);

    // --- Other steps ------------------------------------------------------------
    let typing_seg = segment_mono(test, SegmentKind::Typing, &live);
    let typing_rel_db = typing_seg.as_ref().map(|t| {
        let f = dsp::frames(t, sr, 10.0);
        dsp::percentile(&f.rms_db, 95.0) - speech_rms_dbfs
    });
    let tap_seg = segment_mono(test, SegmentKind::DeskTap, &live);
    let tap_rel_db = tap_seg.as_ref().map(|t| {
        let lf = dsp::filtered(t, &[Biquad::lowpass(sr, 150.0, 0.707), Biquad::lowpass(sr, 150.0, 0.707)]);
        dsp::db_amp(dsp::peak_abs(&lf)) - speech_peak_dbfs
    });
    let vibration = vibration::measure(&vibration::Inputs {
        sr,
        speech_peak_dbfs,
        speech_mid_db: vibration::speech_mid_db(&read, sr, active_thresh),
        tap: tap_seg.as_deref(),
        typing: typing_seg.as_deref(),
        silence: silence.as_deref(),
        read: &read,
    });
    let (dropouts, crackles) = match segment_mono(test, SegmentKind::CableWiggle, &live) {
        Some(w) => glitches(&w, sr),
        None => (0, 0),
    };
    let read_spread = if active_rms.len() >= 10 { dsp::percentile(&active_rms, 90.0) - dsp::percentile(&active_rms, 10.0) } else { 0.0 };
    let (moving_drop_db, moving_spread_db) = match segment_mono(test, SegmentKind::Moving, &live) {
        Some(m) => {
            let f = dsp::frames(&m, sr, 20.0);
            let act: Vec<f64> = f.rms_db.iter().copied().filter(|l| *l > active_thresh).collect();
            if act.len() < 10 {
                (Some(speech_rms_dbfs - dsp::median(&f.rms_db)), None)
            } else {
                (
                    Some(speech_rms_dbfs - dsp::median(&act)),
                    Some(dsp::percentile(&act, 90.0) - dsp::percentile(&act, 10.0) - read_spread),
                )
            }
        }
        None => (None, None),
    };
    let pause_floor_rise_db = silence.as_ref().map(|_| dsp::percentile(&read_frames.rms_db, 3.0) - noise_floor_dbfs);

    AudioMetrics {
        sample_rate: sr,
        channels: test.audio.channels.len(),
        no_signal,
        channel_levels_db,
        dead_channel,
        stereo_correlation,
        dc_offset,
        noise_floor_dbfs,
        noise_character,
        speech_rms_dbfs,
        speech_peak_dbfs,
        quiet_speech_dbfs,
        speech_lufs,
        snr_db,
        loud_peak_dbfs,
        clipped_fraction,
        f0_hz,
        band_levels_db,
        band_deviation_db,
        tone,
        upper_bandwidth_hz,
        hum,
        reverb_decay_s,
        reflection,
        plosive_events,
        plosive_worst_db,
        sibilance_db,
        typing_rel_db,
        tap_rel_db,
        vibration,
        dropouts,
        crackles,
        moving_drop_db,
        moving_spread_db,
        loud_vs_read_db,
        pause_floor_rise_db,
    }
}

/// Mains hum: tonal peaks at 50 or 60 Hz and harmonics, above local noise.
fn detect_hum(spec: &Spectrum) -> Option<HumInfo> {
    let mut best: Option<HumInfo> = None;
    for mains in [50.0, 60.0] {
        let mut strongest = f64::NEG_INFINITY;
        let mut level = f64::NEG_INFINITY;
        for h in 1..=5 {
            let f = mains * h as f64;
            let k = (f / spec.bin_hz).round() as usize;
            if k + 12 >= spec.power.len() || k < 12 {
                continue;
            }
            let tone: f64 = spec.power[k - 1..=k + 1].iter().sum();
            let mut neigh: Vec<f64> = spec.power[k - 10..k - 3].iter().chain(&spec.power[k + 4..k + 11]).copied().collect();
            neigh.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let floor = neigh[neigh.len() / 2] * 3.0; // 3 bins' worth
            let prom = dsp::db_pow(tone) - dsp::db_pow(floor);
            if prom > strongest {
                strongest = prom;
                level = dsp::db_pow(tone);
            }
        }
        if strongest > 12.0 && level > -100.0 {
            let info = HumInfo { mains_hz: mains, prominence_db: strongest, level_dbfs: level };
            if best.map(|b| info.prominence_db > b.prominence_db).unwrap_or(true) {
                best = Some(info);
            }
        }
    }
    best
}

/// Decay rate of syllable endings that lead into pauses (EDT-like, from −5 to
/// −30 dB below the syllable peak), converted to a 60 dB decay time.
fn estimate_decay(x: &[f32], sr: u32, floor: f64, speech_rms: f64) -> Option<f64> {
    let f = dsp::frames(x, sr, 10.0);
    let n = f.rms_db.len();
    // Light smoothing (30 ms) so noisy reverb tails still read as one decay.
    let smoothed: Vec<f64> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(1);
            let b = (i + 2).min(n);
            f.rms_db[a..b].iter().sum::<f64>() / (b - a) as f64
        })
        .collect();
    let e = &smoothed;
    // A pause is >= 150 ms well below speech; reverb tails can keep pauses
    // above the true noise floor, so the threshold follows the speech level.
    let gap = (floor + 10.0).max(speech_rms - 30.0);
    let fit_floor = gap + 3.0;
    let mut estimates = Vec::new();
    let mut i = 0;
    while i < n {
        if e[i] < gap {
            let start = i;
            while i < n && e[i] < gap {
                i += 1;
            }
            if i - start >= 15 && start >= 10 {
                // Walk back from the pause over the final, monotonic decay only
                // (a quieter consonant after the loudest syllable is a plateau,
                // not room decay).
                let back = start.saturating_sub(40);
                let mut pk = start;
                while pk > back && e[pk - 1] >= e[pk] - 1.5 {
                    pk -= 1;
                }
                let pv = e[pk];
                if pv > fit_floor + 18.0 {
                    let pts: Vec<(f64, f64)> = (pk..start + 5)
                        .filter(|&j| j < n && e[j] <= pv - 5.0 && e[j] >= (pv - 30.0).max(fit_floor))
                        .map(|j| (j as f64 * f.frame_s, e[j]))
                        .collect();
                    if pts.len() >= 3 {
                        let m = pts.len() as f64;
                        let mx = pts.iter().map(|p| p.0).sum::<f64>() / m;
                        let my = pts.iter().map(|p| p.1).sum::<f64>() / m;
                        let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
                        let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
                        if sxx > 0.0 {
                            let slope = sxy / sxx; // dB per second
                            if slope < -20.0 {
                                estimates.push(-60.0 / slope);
                            }
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    // The room sets a lower bound on how fast any syllable can die away, so the
    // faster decays describe the room best.
    if estimates.len() < 2 { None } else { Some(dsp::percentile(&estimates, 25.0)) }
}

/// Comb filtering from an early reflection (desk, monitor): a periodic ripple in
/// the long-term spectrum. Found via the cepstrum of the detrended log spectrum.
fn detect_reflection(spec: &Spectrum, f0: Option<f64>) -> Option<ReflectionInfo> {
    let lo = (200.0 / spec.bin_hz) as usize;
    let hi = ((8000.0 / spec.bin_hz) as usize).min(spec.power.len() - 1);
    if hi <= lo + 64 {
        return None;
    }
    let logp: Vec<f64> = spec.power[lo..=hi].iter().map(|p| dsp::db_pow(*p)).collect();
    // Detrend with a wide moving average (≈1.5 kHz) to remove the voice envelope.
    let half = ((750.0 / spec.bin_hz) as usize).max(4);
    let m = logp.len();
    let mut prefix = vec![0.0; m + 1];
    for i in 0..m {
        prefix[i + 1] = prefix[i] + logp[i];
    }
    let resid: Vec<f64> = (0..m)
        .map(|i| {
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(m);
            logp[i] - (prefix[b] - prefix[a]) / (b - a) as f64
        })
        .collect();
    let nfft = (m * 4).next_power_of_two();
    let mut buf: Vec<rustfft::num_complex::Complex<f64>> = (0..nfft)
        .map(|i| {
            if i < m {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (m - 1) as f64).cos();
                rustfft::num_complex::Complex::new(resid[i] * w, 0.0)
            } else {
                rustfft::num_complex::Complex::new(0.0, 0.0)
            }
        })
        .collect();
    let mut planner = rustfft::FftPlanner::<f64>::new();
    planner.plan_fft_forward(nfft).process(&mut buf);
    let wsum: f64 = (0..m).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (m - 1) as f64).cos()).sum();
    // Quefrency of index q: q / (nfft * bin_hz) seconds.
    let q_of = |idx: usize| idx as f64 / (nfft as f64 * spec.bin_hz);
    let mut best = (0usize, 0.0f64);
    let mut mags = Vec::new();
    for idx in 1..nfft / 2 {
        let q = q_of(idx);
        if !(0.0005..=0.003).contains(&q) {
            continue;
        }
        if let Some(f0) = f0 {
            let p = 1.0 / f0;
            if (q - p).abs() < 0.1 * p || (q - p / 2.0).abs() < 0.1 * p / 2.0 {
                continue;
            }
        }
        let mag = buf[idx].norm();
        mags.push(mag);
        if mag > best.1 {
            best = (idx, mag);
        }
    }
    if mags.len() < 8 {
        return None;
    }
    let med = dsp::median(&mags);
    // Cosine amplitude of the ripple in dB → peak-to-peak.
    let amp = 2.0 * best.1 / wsum;
    let ripple_db = 2.0 * amp;
    if best.1 > 4.0 * med && ripple_db > 3.0 {
        let delay_ms = q_of(best.0) * 1000.0;
        Some(ReflectionInfo { delay_ms, path_cm: delay_ms * 34.3, ripple_db })
    } else {
        None
    }
}

/// Low-frequency bursts that dominate the voice band: "p" and "b" pops.
fn plosives(x: &[f32], sr: u32, speech_rms: f64) -> (usize, Option<f64>) {
    let lf = dsp::filtered(x, &[Biquad::lowpass(sr, 150.0, 0.707), Biquad::lowpass(sr, 150.0, 0.707)]);
    let mid = dsp::filtered(x, &[Biquad::highpass(sr, 300.0, 0.707), Biquad::lowpass(sr, 3000.0, 0.707)]);
    let lf_f = dsp::frames(&lf, sr, 20.0);
    let mid_f = dsp::frames(&mid, sr, 20.0);
    let mut events = 0;
    let mut worst: Option<f64> = None;
    let mut run = 0usize;
    let mut run_worst = f64::NEG_INFINITY;
    let close = |run: usize, rw: f64, events: &mut usize, worst: &mut Option<f64>| {
        // Plosives are short bursts (<= 60 ms); longer LF dominance is boom, not pops.
        if run > 0 && run <= 3 {
            *events += 1;
            *worst = Some(worst.map_or(rw, |w: f64| w.max(rw)));
        }
    };
    for (l, m) in lf_f.rms_db.iter().zip(&mid_f.rms_db) {
        let excess = l - speech_rms;
        if *l > m + 6.0 && excess > -6.0 {
            run += 1;
            run_worst = run_worst.max(excess);
        } else {
            close(run, run_worst, &mut events, &mut worst);
            run = 0;
            run_worst = f64::NEG_INFINITY;
        }
    }
    close(run, run_worst, &mut events, &mut worst);
    (events, worst)
}

/// Loudest sibilant energy (5–9 kHz) relative to the speech level.
fn sibilance(x: &[f32], sr: u32, speech_rms: f64, floor: f64) -> Option<f64> {
    if sr < 22050 {
        return None;
    }
    let hf = dsp::filtered(
        x,
        &[Biquad::highpass(sr, 5000.0, 0.707), Biquad::highpass(sr, 5000.0, 0.707), Biquad::lowpass(sr, 9000.0, 0.707)],
    );
    let f = dsp::frames(&hf, sr, 10.0);
    let vals: Vec<f64> = f.rms_db.iter().copied().filter(|l| *l > floor + 10.0).collect();
    if vals.len() < 5 { None } else { Some(dsp::percentile(&vals, 98.0) - speech_rms) }
}

/// Dropouts (runs of exact zeros ≥ 2 ms) and crackles (sudden jumps).
fn glitches(x: &[f32], sr: u32) -> (usize, usize) {
    let min_run = (sr as f64 * 0.002) as usize;
    let mut dropouts = 0;
    let mut run = 0;
    for v in x {
        if *v == 0.0 {
            run += 1;
            if run == min_run {
                dropouts += 1;
            }
        } else {
            run = 0;
        }
    }
    let typical = dsp::percentile(&x.windows(2).map(|w| (w[1] - w[0]).abs() as f64).collect::<Vec<_>>(), 99.0);
    let mut crackles = 0;
    let mut last = 0usize;
    for (i, w) in x.windows(2).enumerate() {
        let d = (w[1] - w[0]).abs() as f64;
        if d > 0.3 && d > 8.0 * typical && i > last + (sr as usize / 50) {
            crackles += 1;
            last = i;
        }
    }
    (dropouts, crackles)
}
