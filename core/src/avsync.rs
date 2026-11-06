//! Lip-sync check: how far the mic is from the picture.
//!
//! The user claps three or four times at uneven intervals in front of the
//! camera. The mic recording gives sound onsets, the camera frames give
//! motion onsets (hands meeting). Cross-correlating the two onset tracks over
//! a few hundred milliseconds gives the offset. Nothing is changed or
//! processed; this only measures and then says which side to delay.
//!
//! Both clocks must start together: audio sample 0 and frame timestamps are
//! seconds from the same moment (the app starts both captures at once).
//! Camera and capture-card latency is inside the frame timestamps' pipeline,
//! so the number includes it, which is exactly what viewers see.

use crate::finding::{Area, Finding, FixKind, Severity};
use crate::video::Frame;

pub const AV_OFFSET: &str = "sync.av_offset";

/// Analysis grid step.
const HOP_MS: f64 = 5.0;
/// Search range either way.
const MAX_LAG_MS: f64 = 600.0;
/// Onset tracks are smoothed this wide (frame interval at 30 fps is 33 ms).
const SMOOTH_MS: f64 = 40.0;
/// Fewest claps (sound onsets) needed.
const MIN_EVENTS: usize = 3;
/// Peak must be this much of a perfect match, and this much better than any
/// peak further than 100 ms away (regular clapping fits many offsets).
const MIN_SCORE: f64 = 0.35;
const MIN_UNIQUENESS: f64 = 1.5;

/// ITU-R BT.1359-1 detectability limits, the usual reference for lip sync:
/// sound ahead of the picture is noticed from about 45 ms, sound behind it
/// from about 125 ms. Annoying ("unacceptable") at about 90 and 185 ms.
pub const NOTICED_AUDIO_EARLY_MS: f64 = 45.0;
pub const NOTICED_AUDIO_LATE_MS: f64 = 125.0;
pub const ANNOYING_AUDIO_EARLY_MS: f64 = 90.0;
pub const ANNOYING_AUDIO_LATE_MS: f64 = 185.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncMeasurement {
    /// Positive: the sound arrives after the picture (mic late).
    /// Negative: the sound arrives before the picture (picture late).
    pub offset_ms: f64,
    /// 0..1 how clear the match was.
    pub score: f64,
    /// Sound onsets found.
    pub events: usize,
}

fn smooth(x: &[f64], half: usize) -> Vec<f64> {
    let n = x.len();
    let mut out = vec![0.0; n];
    for (i, &v) in x.iter().enumerate() {
        if v == 0.0 {
            continue;
        }
        for j in i.saturating_sub(half)..=(i + half).min(n.saturating_sub(1)) {
            let w = 1.0 - (j as f64 - i as f64).abs() / (half as f64 + 1.0);
            out[j] += v * w;
        }
    }
    out
}

/// Positive jumps of an envelope, one value per grid step.
fn onsets(env: &[f64]) -> Vec<f64> {
    let mut o = vec![0.0; env.len()];
    for i in 1..env.len() {
        o[i] = (env[i] - env[i - 1]).max(0.0);
    }
    o
}

fn audio_track(mic: &[f32], rate: u32, bins: usize) -> Vec<f64> {
    let hop = (f64::from(rate) * HOP_MS / 1000.0).round().max(1.0) as usize;
    let mut env = vec![0.0; bins];
    for (b, e) in env.iter_mut().enumerate() {
        let s = b * hop;
        if s >= mic.len() {
            break;
        }
        let end = (s + hop).min(mic.len());
        let ms: f64 = mic[s..end].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (end - s) as f64;
        *e = ms.sqrt();
    }
    onsets(&env)
}

fn motion(a: &Frame, b: &Frame) -> f64 {
    if !a.is_valid() || !b.is_valid() || a.width != b.width || a.height != b.height {
        return 0.0;
    }
    let step = ((a.width * a.height) / 4000).max(1);
    let (mut sum, mut n) = (0.0, 0usize);
    let mut i = 0;
    while i < a.width * a.height {
        let (pa, pb) = (&a.rgb[i * 3..i * 3 + 3], &b.rgb[i * 3..i * 3 + 3]);
        let la = f64::from(pa[0]) * 0.299 + f64::from(pa[1]) * 0.587 + f64::from(pa[2]) * 0.114;
        let lb = f64::from(pb[0]) * 0.299 + f64::from(pb[1]) * 0.587 + f64::from(pb[2]) * 0.114;
        sum += (la - lb).abs();
        n += 1;
        i += step;
    }
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn video_track(frames: &[Frame], bins: usize) -> Vec<f64> {
    // Motion energy per frame, placed on the grid at the frame's timestamp,
    // then its positive jumps (a clap starts moving, it does not just move).
    let mut m = vec![0.0f64; bins];
    for w in frames.windows(2) {
        let bin = (w[1].timestamp_s * 1000.0 / HOP_MS).round();
        if bin >= 0.0 && (bin as usize) < bins {
            m[bin as usize] = m[bin as usize].max(motion(&w[0], &w[1]));
        }
    }
    let mut held = vec![0.0f64; bins];
    let mut last = 0.0;
    for i in 0..bins {
        if m[i] > 0.0 {
            last = m[i];
        }
        held[i] = last;
    }
    onsets(&held)
}

/// Offset between sound and picture, or `None` when the clap test did not
/// give a clear answer (too few claps, no motion, regular clapping).
pub fn measure_offset(mic: &[f32], rate: u32, frames: &[Frame]) -> Option<SyncMeasurement> {
    if rate == 0 || mic.is_empty() || frames.len() < 4 {
        return None;
    }
    let dur_ms = (mic.len() as f64 / f64::from(rate) * 1000.0).max(frames.last()?.timestamp_s * 1000.0);
    let bins = (dur_ms / HOP_MS).ceil() as usize + 1;
    let half = (SMOOTH_MS / HOP_MS / 2.0).round() as usize;
    let a_raw = audio_track(mic, rate, bins);
    let amax = a_raw.iter().cloned().fold(0.0, f64::max);
    if amax <= 0.0 {
        return None;
    }
    // Count claps: separate bursts above half the strongest onset.
    let gap = (200.0 / HOP_MS) as usize;
    let (mut events, mut last) = (0usize, None::<usize>);
    for (i, v) in a_raw.iter().enumerate() {
        if *v >= amax * 0.5 && last.is_none_or(|l| i - l > gap) {
            events += 1;
        }
        if *v >= amax * 0.5 {
            last = Some(i);
        }
    }
    if events < MIN_EVENTS {
        return None;
    }
    let a = smooth(&a_raw, half);
    let v = smooth(&video_track(frames, bins), half);
    let (na, nv) = (a.iter().map(|x| x * x).sum::<f64>().sqrt(), v.iter().map(|x| x * x).sum::<f64>().sqrt());
    if na <= 0.0 || nv <= 0.0 {
        return None;
    }
    let max_lag = (MAX_LAG_MS / HOP_MS) as i64;
    // corr[l] = sum a[t] * v[t - l]: sound at t belongs to the picture event l earlier.
    let corr: Vec<f64> = (-max_lag..=max_lag)
        .map(|l| {
            (0..bins as i64)
                .filter_map(|t| {
                    let u = t - l;
                    (u >= 0 && u < bins as i64).then(|| a[t as usize] * v[u as usize])
                })
                .sum::<f64>()
                / (na * nv)
        })
        .collect();
    let (pk, &best) = corr.iter().enumerate().max_by(|x, y| x.1.partial_cmp(y.1).unwrap())?;
    if best < MIN_SCORE {
        return None;
    }
    let far = (100.0 / HOP_MS) as usize;
    let other = corr.iter().enumerate().filter(|(i, _)| i.abs_diff(pk) > far).map(|(_, c)| *c).fold(0.0, f64::max);
    if other > 0.0 && best / other < MIN_UNIQUENESS {
        return None;
    }
    // Parabolic refinement keeps the answer finer than the grid.
    let frac = if pk > 0 && pk + 1 < corr.len() {
        let (l, c, r) = (corr[pk - 1], corr[pk], corr[pk + 1]);
        let d = l - 2.0 * c + r;
        if d.abs() > 1e-12 { 0.5 * (l - r) / d } else { 0.0 }
    } else {
        0.0
    };
    let offset_ms = ((pk as i64 - max_lag) as f64 + frac) * HOP_MS;
    Some(SyncMeasurement { offset_ms, score: best.min(1.0), events })
}

/// A finding if the offset is something viewers would notice. `capture_card`
/// adds the usual cause. OBS' sync offset can only delay the sound, so a late
/// mic is fixed by delaying the picture instead.
pub fn finding(m: &SyncMeasurement, capture_card: bool) -> Option<Finding> {
    let o = m.offset_ms;
    let (noticed, annoying) = if o >= 0.0 {
        (NOTICED_AUDIO_LATE_MS, ANNOYING_AUDIO_LATE_MS)
    } else {
        (NOTICED_AUDIO_EARLY_MS, ANNOYING_AUDIO_EARLY_MS)
    };
    if o.abs() < noticed {
        return None;
    }
    let ms = (o.abs() / 10.0).round() * 10.0;
    let cause = if capture_card { " A capture card adds a short delay to the picture, which is normal." } else { "" };
    let (title, why, fix) = if o >= 0.0 {
        (
            "Your voice arrives after your picture",
            format!("Viewers hear you a moment after they see your mouth move.{cause}"),
            format!("In OBS, right-click your camera source, choose Filters, add a Render Delay filter and set it to {ms:.0} ms. (OBS's own Sync Offset only delays sound, so it can't fix this direction.) Then run the clap test again."),
        )
    } else {
        (
            "Your picture arrives after your voice",
            format!("Viewers hear you before they see your mouth move.{cause}"),
            format!("In OBS open Audio Mixer > the gear icon > Advanced Audio Properties, and set the Sync Offset for your mic to {ms:.0} ms. Then run the clap test again."),
        )
    };
    let mut f = Finding::new(AV_OFFSET, Area::Mic, if o.abs() >= annoying { Severity::Major } else { Severity::Minor }, title, &why, &fix, FixKind::Guided);
    f.evidence.insert("offset_ms".into(), o.round());
    f.evidence.insert("score".into(), (m.score * 100.0).round() / 100.0);
    f.evidence.insert("claps".into(), m.events as f64);
    Some(f)
}

// ---------------------------------------------------------------------------
// Drift check: is the offset fixed, or does it creep?
//
// The clap test is repeated on one continuous recording (the mic stays open
// between claps, so its own sample clock is the audio timeline). A fixed
// offset gives the same number every time; two clocks that run at slightly
// different rates give a number that changes steadily. Only a fixed offset
// can be fixed with OBS' Sync Offset.
// ---------------------------------------------------------------------------

pub const AV_DRIFT: &str = "sync.av_drift";

/// Starting value (OPINION, docs/field-complaints/dual-pc-audio-sync.md):
/// more than 10 ms per 10 minutes is noticeable within an hour of streaming.
pub const DRIFT_LIMIT_MS_PER_10MIN: f64 = 10.0;
/// The offset has to move at least this much before we call it drift: one
/// clap measurement is good to about +-10 ms (5 ms grid, frame timing).
pub const DRIFT_MIN_CHANGE_MS: f64 = 20.0;
/// Scatter around the line (three or more checks) above this: the clap
/// checks disagree too much to say anything.
pub const DRIFT_MAX_SPREAD_MS: f64 = 30.0;
/// Shorter than this, a "steady" answer is marked short (a longer check is surer).
pub const DRIFT_SURE_SPAN_MIN: f64 = 20.0;
/// Major once the drift would pass the annoying limit (~90 ms) within an hour.
pub const DRIFT_MAJOR_MS_PER_10MIN: f64 = 15.0;

/// One clap check: when (seconds from the start of the drift check) and the
/// offset it measured (ms, + = sound after picture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetSample {
    pub t_s: f64,
    pub offset_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftVerdict {
    /// The offset holds: a fixed Sync Offset / Render Delay is a valid fix.
    Steady,
    /// The offset moves: a fixed offset will not hold; fix the clocks.
    Drifting,
    /// Can't tell yet.
    Unclear(DriftUnclear),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftUnclear {
    /// Fewer than two checks, or no time between them.
    TooFewPoints,
    /// The checks disagree with each other more than with any straight line.
    Noisy,
    /// It moved a little, but not more than one check can be off: wait longer.
    TooShort,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftEstimate {
    pub verdict: DriftVerdict,
    /// Least-squares slope, ms of offset change per 10 minutes (+ = sound falls further behind).
    pub slope_ms_per_10min: f64,
    /// Change over the whole check along the line (ms).
    pub change_ms: f64,
    /// Time from first to last check (minutes).
    pub span_min: f64,
    pub points: usize,
    /// Scatter of the checks around the line (ms); `None` with two points.
    pub spread_ms: Option<f64>,
    /// "Steady" over less than `DRIFT_SURE_SPAN_MIN`: say a longer check is surer.
    pub short: bool,
}

/// Fit a straight line through the clap checks and decide whether the offset
/// holds. Thresholds are starting values (see the constants).
pub fn estimate_drift(samples: &[OffsetSample]) -> DriftEstimate {
    let pts: Vec<OffsetSample> = samples.iter().copied().filter(|s| s.t_s.is_finite() && s.offset_ms.is_finite()).collect();
    let n = pts.len();
    let span_s = if n > 0 {
        let lo = pts.iter().map(|s| s.t_s).fold(f64::INFINITY, f64::min);
        let hi = pts.iter().map(|s| s.t_s).fold(f64::NEG_INFINITY, f64::max);
        hi - lo
    } else {
        0.0
    };
    let mut e = DriftEstimate {
        verdict: DriftVerdict::Unclear(DriftUnclear::TooFewPoints),
        slope_ms_per_10min: 0.0,
        change_ms: 0.0,
        span_min: span_s / 60.0,
        points: n,
        spread_ms: None,
        short: false,
    };
    if n < 2 || span_s <= 0.0 {
        return e;
    }
    let mt = pts.iter().map(|s| s.t_s).sum::<f64>() / n as f64;
    let mo = pts.iter().map(|s| s.offset_ms).sum::<f64>() / n as f64;
    let sxx: f64 = pts.iter().map(|s| (s.t_s - mt).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|s| (s.t_s - mt) * (s.offset_ms - mo)).sum();
    let slope_per_s = sxy / sxx;
    e.slope_ms_per_10min = slope_per_s * 600.0;
    e.change_ms = slope_per_s * span_s;
    if n >= 3 {
        let rss: f64 = pts.iter().map(|s| (s.offset_ms - (mo + slope_per_s * (s.t_s - mt))).powi(2)).sum();
        let spread = (rss / (n - 2) as f64).sqrt();
        e.spread_ms = Some(spread);
        if spread > DRIFT_MAX_SPREAD_MS {
            e.verdict = DriftVerdict::Unclear(DriftUnclear::Noisy);
            return e;
        }
    }
    let fast = e.slope_ms_per_10min.abs() >= DRIFT_LIMIT_MS_PER_10MIN;
    e.verdict = if !fast {
        e.short = e.span_min < DRIFT_SURE_SPAN_MIN;
        DriftVerdict::Steady
    } else if e.change_ms.abs() >= DRIFT_MIN_CHANGE_MS {
        DriftVerdict::Drifting
    } else {
        DriftVerdict::Unclear(DriftUnclear::TooShort)
    };
    e
}

/// A finding when the offset drifts. `capture_card` adds the capture-card
/// audio cause. No finding for steady or unclear (the clap finding covers a
/// fixed offset).
pub fn drift_finding(e: &DriftEstimate, capture_card: bool) -> Option<Finding> {
    if e.verdict != DriftVerdict::Drifting {
        return None;
    }
    let card = if capture_card {
        " If your game sound comes in through the capture card and also through another cable or app, turn one of them off in OBS so there is only one way in."
    } else {
        ""
    };
    let why = "Your sound and your picture run on different clocks that tick at slightly different speeds, so the gap between them keeps growing during the stream. A fixed Sync Offset would be right now and wrong again later.";
    let fix = format!(
        "Bring your mic and your game sound into one device (a mixer or audio interface), so they share one clock. Set every audio device and OBS to 48 kHz (Windows Sound settings > device > Advanced, and OBS Settings > Audio).{card} Then run the drift check again."
    );
    let sev = if e.slope_ms_per_10min.abs() >= DRIFT_MAJOR_MS_PER_10MIN { Severity::Major } else { Severity::Minor };
    let mut f = Finding::new(AV_DRIFT, Area::Mic, sev, "Your sound and picture slowly drift apart", why, &fix, FixKind::Guided);
    f.evidence.insert("slope_ms_per_10min".into(), (e.slope_ms_per_10min * 10.0).round() / 10.0);
    f.evidence.insert("change_ms".into(), e.change_ms.round());
    f.evidence.insert("span_min".into(), (e.span_min * 10.0).round() / 10.0);
    f.evidence.insert("checks".into(), e.points as f64);
    if let Some(s) = e.spread_ms {
        f.evidence.insert("spread_ms".into(), s.round());
    }
    Some(f)
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    fn series(points: &[(f64, f64)]) -> Vec<OffsetSample> {
        points.iter().map(|&(min, o)| OffsetSample { t_s: min * 60.0, offset_ms: o }).collect()
    }

    #[test]
    fn zero_slope_is_steady() {
        let e = estimate_drift(&series(&[(0.0, -120.0), (30.0, -118.0), (60.0, -121.0)]));
        assert_eq!(e.verdict, DriftVerdict::Steady, "{e:?}");
        assert!(e.slope_ms_per_10min.abs() < 1.0 && !e.short);
        assert!(drift_finding(&e, true).is_none());
        // Two checks ten minutes apart: steady, but marked short.
        let e = estimate_drift(&series(&[(0.0, 50.0), (10.0, 54.0)]));
        assert_eq!(e.verdict, DriftVerdict::Steady);
        assert!(e.short);
    }

    #[test]
    fn steady_drift_is_found_and_explained() {
        // 20 ms per 10 min, over an hour: 120 ms.
        let pts: Vec<(f64, f64)> = (0..=6).map(|k| (k as f64 * 10.0, -100.0 + k as f64 * 20.0)).collect();
        let e = estimate_drift(&series(&pts));
        assert_eq!(e.verdict, DriftVerdict::Drifting, "{e:?}");
        assert!((e.slope_ms_per_10min - 20.0).abs() < 0.01);
        assert!((e.change_ms - 120.0).abs() < 0.1);
        let f = drift_finding(&e, true).unwrap();
        assert_eq!(f.id, AV_DRIFT);
        assert_eq!(f.severity, Severity::Major);
        assert!(f.fix.contains("one device") && f.fix.contains("48 kHz") && f.fix.contains("capture card"));
        assert!(!f.why.contains("ms"), "numbers only in evidence: {}", f.why);
        // Drift the other way (sound gets ahead), slower: still drift, minor.
        let e = estimate_drift(&series(&[(0.0, 0.0), (30.0, -36.0)]));
        assert_eq!(e.verdict, DriftVerdict::Drifting);
        assert_eq!(drift_finding(&e, false).unwrap().severity, Severity::Minor);
    }

    #[test]
    fn noisy_checks_are_unclear() {
        let e = estimate_drift(&series(&[(0.0, 0.0), (10.0, 90.0), (20.0, -60.0), (30.0, 40.0)]));
        assert_eq!(e.verdict, DriftVerdict::Unclear(DriftUnclear::Noisy), "{e:?}");
        assert!(drift_finding(&e, false).is_none());
        // Small noise on a real drift still reads as drift.
        let e = estimate_drift(&series(&[(0.0, 0.0), (10.0, 18.0), (20.0, 33.0), (30.0, 64.0)]));
        assert_eq!(e.verdict, DriftVerdict::Drifting, "{e:?}");
    }

    #[test]
    fn too_few_points_or_too_short_is_unclear() {
        assert_eq!(estimate_drift(&[]).verdict, DriftVerdict::Unclear(DriftUnclear::TooFewPoints));
        assert_eq!(estimate_drift(&series(&[(0.0, 10.0)])).verdict, DriftVerdict::Unclear(DriftUnclear::TooFewPoints));
        // Same moment twice.
        assert_eq!(estimate_drift(&series(&[(5.0, 10.0), (5.0, 40.0)])).verdict, DriftVerdict::Unclear(DriftUnclear::TooFewPoints));
        // Non-finite values are dropped.
        assert_eq!(estimate_drift(&series(&[(0.0, f64::NAN), (10.0, 1.0)])).points, 1);
        // Fast slope but only 15 ms moved (inside one check's error): wait longer.
        let e = estimate_drift(&series(&[(0.0, 0.0), (5.0, 15.0)]));
        assert_eq!(e.verdict, DriftVerdict::Unclear(DriftUnclear::TooShort), "{e:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const CLAPS: [f64; 5] = [1.0, 1.9, 3.2, 4.1, 5.6];

    /// Mic: quiet noise with a 40 ms burst at each time.
    fn mic(times: &[f64], secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(RATE)) as usize;
        let mut v: Vec<f32> = (0..n)
            .map(|i| {
                let h = (i as u32).wrapping_mul(2654435761).rotate_left(11) ^ 0x1234_5678;
                ((h % 2001) as f32 / 1000.0 - 1.0) * 0.003
            })
            .collect();
        for t in times {
            let s = (t * f64::from(RATE)) as usize;
            for k in 0..(0.04 * f64::from(RATE)) as usize {
                if s + k < n {
                    v[s + k] += ((k as f32) * 0.37).sin() * 0.5 * (1.0 - k as f32 / 1920.0);
                }
            }
        }
        v
    }

    /// Camera at 30 fps: a patch is "hands" for 150 ms from each time.
    fn video(times: &[f64], secs: f64) -> Vec<Frame> {
        (0..(secs * 30.0) as usize)
            .map(|i| {
                let t = i as f64 / 30.0;
                let hands = times.iter().any(|c| t >= *c && t < *c + 0.15);
                let mut f = Frame::solid(64, 36, [100, 110, 120], t);
                if hands {
                    for y in 10..26 {
                        for x in 20..44 {
                            let k = (y * 64 + x) * 3;
                            f.rgb[k..k + 3].copy_from_slice(&[230, 200, 180]);
                        }
                    }
                }
                f
            })
            .collect()
    }

    fn measure_with(sound_after_picture_s: f64) -> Option<SyncMeasurement> {
        let sound: Vec<f64> = CLAPS.iter().map(|c| c + sound_after_picture_s).collect();
        measure_offset(&mic(&sound, 8.0), RATE, &video(&CLAPS, 8.0))
    }

    #[test]
    fn finds_a_late_mic() {
        let m = measure_with(0.2).unwrap();
        assert!((m.offset_ms - 200.0).abs() < 30.0, "{m:?}");
        assert_eq!(m.events, 5);
    }

    #[test]
    fn finds_a_late_picture() {
        let m = measure_with(-0.12).unwrap();
        assert!((m.offset_ms + 120.0).abs() < 30.0, "{m:?}");
    }

    #[test]
    fn in_sync_gives_no_finding() {
        let m = measure_with(0.0).unwrap();
        assert!(m.offset_ms.abs() < 30.0, "{m:?}");
        assert!(finding(&m, false).is_none());
    }

    #[test]
    fn says_which_side_to_delay() {
        let late = finding(&SyncMeasurement { offset_ms: 200.0, score: 0.8, events: 5 }, true).unwrap();
        assert!(late.fix.contains("Render Delay") && late.fix.contains("200 ms") && late.why.contains("capture card"));
        assert_eq!(late.severity, Severity::Major);
        let early = finding(&SyncMeasurement { offset_ms: -60.0, score: 0.8, events: 5 }, false).unwrap();
        assert!(early.fix.contains("Sync Offset") && early.fix.contains("60 ms"));
        assert_eq!(early.severity, Severity::Minor);
        // 100 ms late is under the 125 ms noticed limit; 100 ms early is over the 45 ms one.
        assert!(finding(&SyncMeasurement { offset_ms: 100.0, score: 0.8, events: 5 }, false).is_none());
        assert!(finding(&SyncMeasurement { offset_ms: -100.0, score: 0.8, events: 5 }, false).is_some());
    }

    #[test]
    fn too_few_claps_or_no_motion_gives_no_answer() {
        let two: Vec<f64> = CLAPS[..2].to_vec();
        assert!(measure_offset(&mic(&two, 8.0), RATE, &video(&two, 8.0)).is_none());
        // Sound but a still picture.
        let still: Vec<Frame> = (0..240).map(|i| Frame::solid(64, 36, [100, 110, 120], i as f64 / 30.0)).collect();
        assert!(measure_offset(&mic(&CLAPS, 8.0), RATE, &still).is_none());
        // Silence.
        assert!(measure_offset(&vec![0.0; 8 * RATE as usize], RATE, &video(&CLAPS, 8.0)).is_none());
        assert!(measure_offset(&[], RATE, &[]).is_none());
    }
}
