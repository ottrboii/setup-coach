//! Synthetic guided-test recordings with controllable faults, for tests, demos
//! and analyser calibration. The voice is a simple source-filter model whose
//! long-term spectrum is matched to the reference speech curve, so a "clean"
//! scenario sits near the target and each injected fault is measurable.

use super::analyze::reference_for_f0;
use super::dsp::{self, BAND_CENTERS, Biquad, Rng, from_db};
use super::{AudioBuffer, MicTest, Segment, SegmentKind};

/// Knobs for one synthetic test. Levels are dBFS unless noted.
#[derive(Debug, Clone)]
pub struct Scenario {
    pub seed: u64,
    pub sample_rate: u32,
    pub f0_hz: f64,
    /// Active speech RMS at the output.
    pub speech_level_dbfs: f64,
    /// Laugh/shout boost over normal speech (dB).
    pub loud_boost_db: f64,
    /// "s" level relative to vowels (dB).
    pub sibilance_db: f64,
    /// "p/b" low-frequency burst level relative to vowels (dB).
    pub plosive_db: f64,
    /// Mic/proximity colouration as peaking filters (freq, gain dB, Q).
    pub tone: Vec<(f64, f64, f64)>,
    /// Room decay (0 = dry) and wet level relative to dry.
    pub rt60_s: f64,
    pub reverb_wet_db: f64,
    /// Early reflection (delay ms, gain 0..1).
    pub reflection: Option<(f64, f64)>,
    /// Fan/AC rumble level.
    pub fan_dbfs: Option<f64>,
    /// Electrical white hiss level.
    pub hiss_dbfs: f64,
    /// Mains hum (Hz, level).
    pub hum: Option<(f64, f64)>,
    /// Interface input 1 arriving as left-only stereo.
    pub left_only: bool,
    /// Low-pass (Bluetooth hands-free, low sample rate).
    pub bandlimit_hz: Option<f64>,
    /// Typing level relative to speech (dB).
    pub typing_db: f64,
    /// Desk tap peak relative to speech peaks (dB).
    pub tap_db: f64,
    /// How long the tap thud rings (decay time constant, s). A mic arm that
    /// touches the desk rings on; a decoupled one stops quickly.
    pub tap_ring_s: f64,
    /// Desk coupling: key presses and a few bumps in the quiet arrive as low
    /// thuds this far from the speech peaks (dB). `None` = decoupled mic.
    pub desk_coupling_db: Option<f64>,
    pub dropouts: bool,
    /// Average level drop while moving (dB).
    pub moving_drop_db: f64,
    /// Mic auto-level squashes the laugh.
    pub auto_level: bool,
    /// Completely silent recording.
    pub silent: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            seed: 7,
            sample_rate: 48000,
            f0_hz: 120.0,
            speech_level_dbfs: -26.0,
            loud_boost_db: 8.0,
            sibilance_db: -16.0,
            plosive_db: -40.0,
            tone: Vec::new(),
            rt60_s: 0.0,
            reverb_wet_db: -12.0,
            reflection: None,
            fan_dbfs: None,
            hiss_dbfs: -82.0,
            hum: None,
            left_only: false,
            bandlimit_hz: None,
            typing_db: -40.0,
            tap_db: -35.0,
            tap_ring_s: 0.02,
            desk_coupling_db: None,
            dropouts: false,
            moving_drop_db: 1.0,
            auto_level: false,
            silent: false,
        }
    }
}

const VOWELS: [[f64; 3]; 5] = [
    [730.0, 1090.0, 2440.0],
    [270.0, 2290.0, 3010.0],
    [300.0, 870.0, 2240.0],
    [530.0, 1840.0, 2480.0],
    [570.0, 840.0, 2410.0],
];

/// Generate `secs` of speech-like signal with vowel RMS ≈ 1.
fn voice(rng: &mut Rng, sr: u32, secs: f64, f0: f64, sib_db: f64, plos_db: f64) -> Vec<f64> {
    let n = (secs * sr as f64) as usize;
    let mut out = vec![0.0f64; n + sr as usize];
    let srf = sr as f64;
    let mut t = 0.15;
    while t < secs - 0.5 {
        let syllables = 5 + (rng.uniform() * 5.0) as usize;
        for s in 0..syllables {
            if t > secs - 0.4 {
                break;
            }
            let start = (t * srf) as usize;
            // Plosive onset.
            if rng.uniform() < 0.35 {
                let len = (0.04 * srf) as usize;
                let amp = from_db(plos_db) * 2.0;
                for i in 0..len {
                    let tt = i as f64 / srf;
                    let v = amp * (2.0 * std::f64::consts::PI * 70.0 * tt).sin() * (-tt / 0.012).exp();
                    if start + i < out.len() {
                        out[start + i] += v;
                    }
                }
                t += 0.02;
            }
            // Vowel.
            let dur = rng.range(0.12, 0.22);
            let len = (dur * srf) as usize;
            let formants = VOWELS[(rng.uniform() * VOWELS.len() as f64) as usize % VOWELS.len()];
            let mut exc = vec![0.0f64; len + (0.04 * srf) as usize];
            let base = f0 * rng.range(0.9, 1.12);
            let mut phase = 0.0;
            for i in 0..len {
                let tt = i as f64 / srf;
                let inst = base * (1.0 - 0.08 * tt / dur) * (1.0 + 0.01 * rng.bipolar());
                phase += inst / srf;
                if phase >= 1.0 {
                    phase -= 1.0;
                    exc[i] = 1.0;
                }
            }
            let mut src = Biquad::lowpass(sr, 800.0, 0.5);
            let mut f1 = Biquad::bandpass(sr, formants[0], 6.0);
            let mut f2 = Biquad::bandpass(sr, formants[1], 8.0);
            let mut f3 = Biquad::bandpass(sr, formants[2], 10.0);
            let mut seg: Vec<f64> = exc
                .iter()
                .map(|e| {
                    let s0 = src.process(*e);
                    f1.process(s0) + 0.6 * f2.process(s0) + 0.35 * f3.process(s0) + 0.1 * s0
                })
                .collect();
            // Envelope: 15 ms attack, exponential release over the last 40 ms.
            let att = (0.015 * srf) as usize;
            let rel_start = len.saturating_sub((0.04 * srf) as usize);
            for (i, v) in seg.iter_mut().enumerate() {
                let g = if i < att {
                    i as f64 / att as f64
                } else if i < rel_start {
                    1.0
                } else {
                    (-((i - rel_start) as f64 / srf) / 0.012).exp()
                };
                *v *= g;
            }
            let rms = (seg[..len].iter().map(|v| v * v).sum::<f64>() / len as f64).sqrt().max(1e-9);
            for (i, v) in seg.iter().enumerate() {
                if start + i < out.len() {
                    out[start + i] += v / rms;
                }
            }
            t += dur;
            // Sibilant coda.
            if rng.uniform() < 0.35 {
                let sdur = rng.range(0.08, 0.12);
                let slen = (sdur * srf) as usize;
                let s0 = (t * srf) as usize;
                let mut hp1 = Biquad::highpass(sr, 4500.0, 0.707);
                let mut hp2 = Biquad::highpass(sr, 4500.0, 0.707);
                let mut lp = Biquad::lowpass(sr, 9000.0, 0.707);
                let noise: Vec<f64> = (0..slen).map(|_| lp.process(hp2.process(hp1.process(rng.bipolar())))).collect();
                let nrms = (noise.iter().map(|v| v * v).sum::<f64>() / slen as f64).sqrt().max(1e-9);
                let ramp = (0.02 * srf) as usize;
                for (i, v) in noise.iter().enumerate() {
                    let g = (i.min(slen - i) as f64 / ramp as f64).min(1.0);
                    if s0 + i < out.len() {
                        out[s0 + i] += v / nrms * from_db(sib_db) * g;
                    }
                }
                t += sdur;
            }
            t += rng.range(0.03, 0.07);
            if s % 3 == 2 {
                t += 0.12;
            }
        }
        t += rng.range(0.45, 0.7);
    }
    out.truncate(n);
    out
}

fn apply(filters: &mut [Biquad], x: &mut [f64]) {
    for f in filters.iter_mut() {
        for v in x.iter_mut() {
            *v = f.process(*v);
        }
    }
}

/// Match the long-term spectrum of `x` to the reference curve (2 passes of
/// third-octave peaking corrections).
fn match_reference(x: &mut [f64], sr: u32, f0: f64) -> Vec<Biquad> {
    let reference = reference_for_f0(Some(f0));
    let mut all = Vec::new();
    for _ in 0..2 {
        let xf: Vec<f32> = x.iter().map(|v| *v as f32).collect();
        let levels = dsp::third_octave_db(&dsp::welch(&xf, sr, 8192));
        let dev: Vec<f64> = levels.iter().zip(&reference).map(|(l, r)| l - r).collect();
        let norm = dev[8..15].iter().sum::<f64>() / 7.0;
        let mut pass: Vec<Biquad> = BAND_CENTERS
            .iter()
            .zip(&dev)
            .map(|(fc, d)| Biquad::peaking(sr, *fc, 4.3, (-(d - norm)).clamp(-18.0, 18.0)))
            .collect();
        apply(&mut pass, x);
        all.extend(pass);
    }
    all
}

fn rms_active(x: &[f64], sr: u32) -> f64 {
    let n = (0.02 * sr as f64) as usize;
    let mut levels: Vec<f64> = x.chunks(n).map(|c| (c.iter().map(|v| v * v).sum::<f64>() / c.len() as f64).sqrt()).collect();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let top: Vec<f64> = levels[levels.len() / 2..].to_vec();
    dsp::median(&top)
}

/// Build a full guided test for a scenario.
pub fn build(s: &Scenario) -> MicTest {
    let sr = s.sample_rate;
    let srf = sr as f64;
    let mut rng = Rng::new(s.seed);
    let plan: [(SegmentKind, f64); 7] = [
        (SegmentKind::Silence, 3.0),
        (SegmentKind::Read, 9.0),
        (SegmentKind::Loud, 2.5),
        (SegmentKind::Typing, 2.0),
        (SegmentKind::DeskTap, 1.5),
        (SegmentKind::CableWiggle, 2.0),
        (SegmentKind::Moving, 4.0),
    ];
    let total: f64 = plan.iter().map(|p| p.1).sum();
    let n = (total * srf) as usize;
    let mut segments = Vec::new();
    let mut acoustic = vec![0.0f64; n];

    // Voice for each speaking segment, spectrally matched once on the read.
    let mut t0 = 0.0;
    let mut correction: Option<Vec<Biquad>> = None;
    let speech_amp = from_db(s.speech_level_dbfs);
    for (kind, dur) in plan {
        let a = (t0 * srf) as usize;
        let speaking = matches!(kind, SegmentKind::Read | SegmentKind::Loud | SegmentKind::CableWiggle | SegmentKind::Moving);
        if speaking {
            if correction.is_none() {
                // Derive the spectral match from a neutral voice (same seed and
                // pitch, default consonants) so injected faults survive.
                let base = Scenario::default();
                let mut crng = Rng::new(s.seed ^ 0xC0FFEE);
                let mut cal = voice(&mut crng, sr, 9.0, s.f0_hz, base.sibilance_db, base.plosive_db);
                correction = Some(match_reference(&mut cal, sr, s.f0_hz));
            }
            let mut v = voice(&mut rng, sr, dur, s.f0_hz, s.sibilance_db, s.plosive_db);
            let mut c = correction.clone().unwrap();
            apply(&mut c, &mut v);
            let r = rms_active(&v, sr).max(1e-9);
            let mut g = speech_amp / r;
            if kind == SegmentKind::Loud {
                g *= from_db(if s.auto_level { 0.5 } else { s.loud_boost_db });
            }
            for (i, x) in v.iter().enumerate() {
                let mut gi = g;
                if kind == SegmentKind::Moving {
                    let tt = i as f64 / srf;
                    gi *= from_db(-s.moving_drop_db * (1.0 - (2.0 * std::f64::consts::PI * tt / 2.0).cos()));
                }
                if a + i < n {
                    acoustic[a + i] += x * gi;
                }
            }
        }
        if kind == SegmentKind::Typing {
            let click_amp = from_db(s.speech_level_dbfs + s.typing_db) * (10.0f64 / 4.0).sqrt();
            let mut hp = Biquad::highpass(sr, 1000.0, 0.707);
            let mut tt = 0.1;
            while tt < dur - 0.1 {
                let c0 = a + (tt * srf) as usize;
                let len = (0.004 * srf) as usize;
                for i in 0..len {
                    let env = (-(i as f64 / srf) / 0.0015).exp();
                    if c0 + i < n {
                        acoustic[c0 + i] += hp.process(rng.bipolar() * 1.7) * env * click_amp * 1.8;
                    }
                }
                tt += rng.range(0.1, 0.16);
            }
        }
        if kind == SegmentKind::DeskTap {
            let peak = from_db(s.speech_level_dbfs + 10.0 + s.tap_db);
            for tap in [0.4, 0.9] {
                let c0 = a + (tap * srf) as usize;
                for i in 0..(0.08 * srf) as usize {
                    let tt = i as f64 / srf;
                    if c0 + i < n {
                        acoustic[c0 + i] += peak * (2.0 * std::f64::consts::PI * 80.0 * tt).sin() * (-tt / s.tap_ring_s).exp();
                    }
                }
            }
        }
        if let Some(c) = s.desk_coupling_db {
            // Thuds from the desk: one per key press while typing, a few bumps in the quiet.
            let thud = from_db(s.speech_level_dbfs + 10.0 + c);
            let mut times: Vec<f64> = Vec::new();
            if kind == SegmentKind::Typing {
                let mut tt = 0.1;
                while tt < dur - 0.1 {
                    times.push(tt);
                    tt += 0.25;
                }
            } else if kind == SegmentKind::Silence {
                times.extend([0.7, 1.6, 2.4]);
            }
            for t in times {
                let c0 = a + (t * srf) as usize;
                for i in 0..(0.3 * srf) as usize {
                    let tt = i as f64 / srf;
                    if c0 + i < n {
                        acoustic[c0 + i] += thud * (2.0 * std::f64::consts::PI * 70.0 * tt).sin() * (-tt / 0.04).exp();
                    }
                }
            }
        }
        segments.push(Segment { kind, start_s: t0, end_s: t0 + dur });
        t0 += dur;
    }

    // Mic colouration (proximity, capsule character, wrong side…).
    let mut tone: Vec<Biquad> = s.tone.iter().map(|(f, g, q)| Biquad::peaking(sr, *f, *q, *g)).collect();
    apply(&mut tone, &mut acoustic);

    // Room: early reflection + reverb tail.
    if let Some((ms, g)) = s.reflection {
        let d = (ms / 1000.0 * srf) as usize;
        let dry = acoustic.clone();
        for i in d..n {
            acoustic[i] += g * dry[i - d];
        }
    }
    if s.rt60_s > 0.0 {
        let wet = schroeder(&acoustic, sr, s.rt60_s);
        let dry_rms = (acoustic.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        let wet_rms = (wet.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt().max(1e-12);
        let k = dry_rms / wet_rms * from_db(s.reverb_wet_db);
        for (a, w) in acoustic.iter_mut().zip(&wet) {
            *a += w * k;
        }
    }
    // Fan rumble (acoustic).
    if let Some(level) = s.fan_dbfs {
        let mut lp1 = Biquad::lowpass(sr, 400.0, 0.707);
        let mut lp2 = Biquad::lowpass(sr, 400.0, 0.707);
        let mut fan: Vec<f64> = (0..n).map(|_| lp2.process(lp1.process(rng.bipolar()))).collect();
        let r = (fan.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt().max(1e-12);
        for v in fan.iter_mut() {
            *v *= from_db(level) / r;
        }
        for (a, f) in acoustic.iter_mut().zip(&fan) {
            *a += f;
        }
    }
    // Electrical: hiss and hum.
    let hiss = from_db(s.hiss_dbfs) * 3f64.sqrt();
    for v in acoustic.iter_mut() {
        *v += rng.bipolar() * hiss;
    }
    if let Some((hz, level)) = s.hum {
        let amp = from_db(level) * 2f64.sqrt();
        for (i, v) in acoustic.iter_mut().enumerate() {
            let tt = i as f64 / srf;
            let w = 2.0 * std::f64::consts::PI * hz * tt;
            *v += amp * (w.sin() + 0.5 * (2.0 * w).sin() + 0.35 * (3.0 * w).sin());
        }
    }
    // Bandwidth limit (codec).
    if let Some(hz) = s.bandlimit_hz {
        let mut lps: Vec<Biquad> = (0..4).map(|_| Biquad::lowpass(sr, hz, 0.707)).collect();
        apply(&mut lps, &mut acoustic);
    }
    if s.silent {
        acoustic.iter_mut().for_each(|v| *v = 0.0);
    }
    // ADC: clamp to full scale.
    let mut mono: Vec<f32> = acoustic.iter().map(|v| v.clamp(-1.0, 1.0) as f32).collect();
    if s.dropouts {
        let w = segments.iter().find(|g| g.kind == SegmentKind::CableWiggle).unwrap();
        for (k, off) in [0.3, 0.9, 1.5].iter().enumerate() {
            let a = ((w.start_s + off) * srf) as usize;
            for v in mono[a..a + (0.006 * srf) as usize].iter_mut() {
                *v = 0.0;
            }
            let c = a + (0.2 * srf) as usize + k * 100;
            mono[c] = if mono[c] > 0.0 { -0.6 } else { 0.6 };
        }
    }
    let channels = if s.left_only { vec![mono.clone(), vec![0.0; mono.len()]] } else { vec![mono] };
    MicTest { audio: AudioBuffer { sample_rate: sr, channels }, segments }
}

/// Simple Schroeder reverb (4 combs + 2 all-passes) with decay `rt60`.
fn schroeder(x: &[f64], sr: u32, rt60: f64) -> Vec<f64> {
    let srf = sr as f64;
    let combs = [0.0297, 0.0371, 0.0411, 0.0437];
    let mut out = vec![0.0; x.len()];
    for d in combs {
        let len = (d * srf) as usize;
        let g = 10f64.powf(-3.0 * d / rt60);
        let mut buf = vec![0.0; len];
        let mut idx = 0;
        for (i, v) in x.iter().enumerate() {
            let y = buf[idx];
            buf[idx] = v + g * y;
            idx = (idx + 1) % len;
            out[i] += y * 0.25;
        }
    }
    for (d, g) in [(0.005, 0.7), (0.0017, 0.7)] {
        let len = (d * srf) as usize;
        let mut buf = vec![0.0; len];
        let mut idx = 0;
        for v in out.iter_mut() {
            let b = buf[idx];
            let y = -g * *v + b;
            buf[idx] = *v + g * y;
            idx = (idx + 1) % len;
            *v = y;
        }
    }
    out
}
