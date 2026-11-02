//! Small DSP toolkit: levels, spectra, filters, pitch and loudness.
//! Everything is plain, deterministic signal processing (no ML).

use rustfft::{FftPlanner, num_complex::Complex};
use std::f64::consts::PI;

/// Amplitude to dB (20·log10).
pub fn db_amp(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Power / mean-square to dB (10·log10).
pub fn db_pow(p: f64) -> f64 {
    10.0 * p.max(1e-24).log10()
}

/// dB to amplitude.
pub fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

pub fn mean_square(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64
}

/// RMS level in dBFS (a full-scale sine reads −3 dBFS).
pub fn level_db(x: &[f32]) -> f64 {
    db_pow(mean_square(x))
}

pub fn peak_abs(x: &[f32]) -> f64 {
    x.iter().fold(0.0f64, |m, v| m.max(v.abs() as f64))
}

/// Linear-interpolated percentile (p in 0..=100) ignoring non-finite values.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = (p / 100.0).clamp(0.0, 1.0) * (v.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    v[lo] + (v[hi] - v[lo]) * (rank - lo as f64)
}

pub fn median(values: &[f64]) -> f64 {
    percentile(values, 50.0)
}

/// Per-frame RMS and peak levels in dBFS.
#[derive(Debug, Clone)]
pub struct Frames {
    pub rms_db: Vec<f64>,
    pub peak_db: Vec<f64>,
    /// Frame length in seconds.
    pub frame_s: f64,
}

pub fn frames(x: &[f32], sr: u32, frame_ms: f64) -> Frames {
    let n = ((sr as f64) * frame_ms / 1000.0).round().max(1.0) as usize;
    let mut rms_db = Vec::with_capacity(x.len() / n + 1);
    let mut peak_db = Vec::with_capacity(x.len() / n + 1);
    for chunk in x.chunks(n) {
        if chunk.len() < n / 2 {
            break;
        }
        rms_db.push(level_db(chunk));
        peak_db.push(db_amp(peak_abs(chunk)));
    }
    Frames { rms_db, peak_db, frame_s: n as f64 / sr as f64 }
}

/// One-sided power spectrum, scaled so the sum over bins equals the signal's
/// mean square (so band levels are directly comparable to [`level_db`]).
#[derive(Debug, Clone)]
pub struct Spectrum {
    pub power: Vec<f64>,
    pub bin_hz: f64,
}

impl Spectrum {
    pub fn band_power(&self, lo_hz: f64, hi_hz: f64) -> f64 {
        let lo = (lo_hz / self.bin_hz).ceil().max(0.0) as usize;
        let hi = ((hi_hz / self.bin_hz).floor() as usize).min(self.power.len().saturating_sub(1));
        if hi < lo {
            // Band narrower than a bin: take the nearest bin.
            let k = ((lo_hz + hi_hz) / 2.0 / self.bin_hz).round() as usize;
            return self.power.get(k).copied().unwrap_or(0.0);
        }
        self.power[lo..=hi].iter().sum()
    }

    pub fn band_db(&self, lo_hz: f64, hi_hz: f64) -> f64 {
        db_pow(self.band_power(lo_hz, hi_hz))
    }

    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.bin_hz
    }
}

/// Welch-averaged power spectrum with a Hann window and 50 % overlap.
pub fn welch(x: &[f32], sr: u32, n: usize) -> Spectrum {
    let n = n.max(16);
    let hop = n / 2;
    let window: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()).collect();
    let wsum2: f64 = window.iter().map(|w| w * w).sum();
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let mut acc = vec![0.0; n / 2 + 1];
    let mut count = 0usize;
    let mut buf = vec![Complex::new(0.0, 0.0); n];
    let mut start = 0usize;
    loop {
        for i in 0..n {
            let s = x.get(start + i).copied().unwrap_or(0.0) as f64;
            buf[i] = Complex::new(s * window[i], 0.0);
        }
        fft.process(&mut buf);
        for k in 0..=n / 2 {
            acc[k] += buf[k].norm_sqr();
        }
        count += 1;
        if start + n >= x.len() {
            break;
        }
        start += hop;
    }
    let norm = 1.0 / (count as f64 * n as f64 * wsum2);
    let power = acc
        .iter()
        .enumerate()
        .map(|(k, p)| p * norm * if k == 0 || k == n / 2 { 1.0 } else { 2.0 })
        .collect();
    Spectrum { power, bin_hz: sr as f64 / n as f64 }
}

/// Third-octave band centres used for tonal analysis (80 Hz – 10 kHz).
pub const BAND_CENTERS: [f64; 22] = [
    80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0,
    1600.0, 2000.0, 2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0,
];

pub fn band_edges(fc: f64) -> (f64, f64) {
    let k = 2f64.powf(1.0 / 6.0);
    (fc / k, fc * k)
}

/// Third-octave band levels in dB for [`BAND_CENTERS`].
pub fn third_octave_db(spec: &Spectrum) -> Vec<f64> {
    BAND_CENTERS
        .iter()
        .map(|&fc| {
            let (lo, hi) = band_edges(fc);
            spec.band_db(lo, hi)
        })
        .collect()
}

/// Biquad filter (RBJ cookbook), transposed direct form II, f64 state.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn from_raw(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    fn w0(sr: u32, f: f64) -> (f64, f64) {
        let f = f.clamp(1.0, sr as f64 * 0.49);
        let w = 2.0 * PI * f / sr as f64;
        (w.cos(), w.sin())
    }

    pub fn highpass(sr: u32, f: f64, q: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let alpha = s / (2.0 * q);
        Self::from_raw((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn lowpass(sr: u32, f: f64, q: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let alpha = s / (2.0 * q);
        Self::from_raw((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Band-pass with 0 dB peak gain.
    pub fn bandpass(sr: u32, f: f64, q: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let alpha = s / (2.0 * q);
        Self::from_raw(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn peaking(sr: u32, f: f64, q: f64, gain_db: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let a = 10f64.powf(gain_db / 40.0);
        let alpha = s / (2.0 * q);
        Self::from_raw(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(sr: u32, f: f64, gain_db: f64) -> Self {
        Self::low_shelf_q(sr, f, gain_db, std::f64::consts::FRAC_1_SQRT_2)
    }

    pub fn low_shelf_q(sr: u32, f: f64, gain_db: f64, q: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let a = 10f64.powf(gain_db / 40.0);
        let alpha = s / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::from_raw(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    pub fn high_shelf(sr: u32, f: f64, gain_db: f64) -> Self {
        Self::high_shelf_q(sr, f, gain_db, std::f64::consts::FRAC_1_SQRT_2)
    }

    pub fn high_shelf_q(sr: u32, f: f64, gain_db: f64, q: f64) -> Self {
        let (c, s) = Self::w0(sr, f);
        let a = 10f64.powf(gain_db / 40.0);
        let alpha = s / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        Self::from_raw(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn run(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process(*s as f64) as f32;
        }
    }

    /// Magnitude response in dB at frequency `f` (for tests / UI curves).
    pub fn response_db(&self, sr: u32, f: f64) -> f64 {
        let w = 2.0 * PI * f / sr as f64;
        let z1 = Complex::new(w.cos(), -w.sin());
        let z2 = z1 * z1;
        let num = Complex::new(self.b0, 0.0) + z1 * self.b1 + z2 * self.b2;
        let den = Complex::new(1.0, 0.0) + z1 * self.a1 + z2 * self.a2;
        db_amp((num / den).norm())
    }
}

/// Filter a copy of `x` through a chain of biquads.
pub fn filtered(x: &[f32], filters: &[Biquad]) -> Vec<f32> {
    let mut out = x.to_vec();
    for f in filters {
        let mut f = *f;
        f.run(&mut out);
    }
    out
}

/// Median fundamental frequency of voiced frames (60–400 Hz), by normalised
/// autocorrelation on a 4× decimated signal. `None` if too little voicing.
pub fn estimate_f0(x: &[f32], sr: u32) -> Option<f64> {
    // Low-pass then decimate by 4.
    let lp = filtered(x, &[Biquad::lowpass(sr, 1000.0, 0.707), Biquad::lowpass(sr, 1000.0, 0.707)]);
    let dec: Vec<f64> = lp.chunks(4).map(|c| c.iter().map(|v| *v as f64).sum::<f64>() / c.len() as f64).collect();
    let dsr = sr as f64 / 4.0;
    let frame = (dsr * 0.04) as usize;
    let min_lag = (dsr / 400.0).floor() as usize;
    let max_lag = (dsr / 60.0).ceil() as usize;
    if dec.len() < frame + max_lag + 1 {
        return None;
    }
    let energies: Vec<f64> = dec.chunks(frame).map(|c| c.iter().map(|v| v * v).sum::<f64>()).collect();
    let e_thresh = percentile(&energies, 60.0);
    let mut f0s = Vec::new();
    let mut start = 0;
    while start + frame + max_lag < dec.len() {
        let seg = &dec[start..start + frame];
        let e0: f64 = seg.iter().map(|v| v * v).sum();
        if e0 >= e_thresh && e0 > 1e-10 {
            let mut corr = vec![0.0; max_lag + 1];
            for lag in min_lag..=max_lag {
                let other = &dec[start + lag..start + lag + frame];
                let num: f64 = seg.iter().zip(other).map(|(a, b)| a * b).sum();
                let e1: f64 = other.iter().map(|v| v * v).sum();
                corr[lag] = num / (e0 * e1).sqrt().max(1e-12);
            }
            let best = (min_lag..=max_lag).max_by(|a, b| corr[*a].partial_cmp(&corr[*b]).unwrap()).unwrap();
            if corr[best] > 0.5 {
                // Avoid sub-octave errors: take the first local peak close to the best.
                let mut chosen = best;
                for lag in min_lag + 1..best {
                    if corr[lag] >= 0.9 * corr[best] && corr[lag] >= corr[lag - 1] && corr[lag] >= corr[lag + 1] {
                        chosen = lag;
                        break;
                    }
                }
                f0s.push(dsr / chosen as f64);
            }
        }
        start += frame;
    }
    if f0s.len() < 5 { None } else { Some(median(&f0s)) }
}

/// Integrated loudness in LUFS (ITU-R BS.1770 K-weighting, 400 ms blocks,
/// absolute −70 LUFS and relative −10 LU gating). Mono input.
pub fn integrated_lufs(x: &[f32], sr: u32) -> f64 {
    let mut shelf = Biquad::high_shelf_q(sr, 1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let mut hp = Biquad::highpass(sr, 38.13547087602444, 0.5003270373238773);
    let y: Vec<f64> = x.iter().map(|&s| hp.process(shelf.process(s as f64))).collect();
    let block = (0.4 * sr as f64) as usize;
    let hop = (block / 4).max(1);
    let mut zs = Vec::new();
    let mut i = 0;
    while i + block <= y.len() {
        zs.push(y[i..i + block].iter().map(|v| v * v).sum::<f64>() / block as f64);
        i += hop;
    }
    if zs.is_empty() && !y.is_empty() {
        zs.push(y.iter().map(|v| v * v).sum::<f64>() / y.len() as f64);
    }
    let l = |ms: f64| -0.691 + 10.0 * ms.max(1e-24).log10();
    let abs: Vec<f64> = zs.iter().copied().filter(|z| l(*z) > -70.0).collect();
    if abs.is_empty() {
        return -70.0;
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let rel = l(mean(&abs)) - 10.0;
    let gated: Vec<f64> = abs.iter().copied().filter(|z| l(*z) > rel).collect();
    if gated.is_empty() { l(mean(&abs)) } else { l(mean(&gated)) }
}

/// Small deterministic PRNG (xorshift64*) so analysis tests are reproducible.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f64 {
        self.uniform() * 2.0 - 1.0
    }
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f64, secs: f64, sr: u32) -> Vec<f32> {
        (0..(secs * sr as f64) as usize)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / sr as f64).sin()) as f32)
            .collect()
    }

    #[test]
    fn levels_of_a_sine() {
        let x = sine(1000.0, 0.5, 1.0, 48000);
        assert!((level_db(&x) - (db_amp(0.5) - 3.0103)).abs() < 0.05);
        assert!((db_amp(peak_abs(&x)) - db_amp(0.5)).abs() < 0.01);
    }

    #[test]
    fn welch_power_matches_mean_square() {
        let x = sine(1000.0, 0.25, 2.0, 48000);
        let s = welch(&x, 48000, 4096);
        let total: f64 = s.power.iter().sum();
        assert!((db_pow(total) - level_db(&x)).abs() < 0.2);
        // energy sits in the 1 kHz third-octave band
        let (lo, hi) = band_edges(1000.0);
        assert!((s.band_db(lo, hi) - level_db(&x)).abs() < 0.3);
    }

    #[test]
    fn lufs_of_reference_sine() {
        // 997 Hz sine at amplitude 0.1 (-20 dBFS peak) ≈ -23.0 LUFS.
        let x = sine(997.0, 0.1, 5.0, 48000);
        let l = integrated_lufs(&x, 48000);
        assert!((l - (-23.01)).abs() < 0.3, "lufs {l}");
    }

    #[test]
    fn biquad_responses() {
        let sr = 48000;
        let hp = Biquad::highpass(sr, 100.0, 0.707);
        assert!(hp.response_db(sr, 1000.0).abs() < 0.2);
        assert!((hp.response_db(sr, 100.0) + 3.0).abs() < 0.3);
        assert!(hp.response_db(sr, 25.0) < -20.0);
        let pk = Biquad::peaking(sr, 300.0, 1.0, -4.0);
        assert!((pk.response_db(sr, 300.0) + 4.0).abs() < 0.05);
        let ls = Biquad::low_shelf(sr, 150.0, 6.0);
        assert!((ls.response_db(sr, 20.0) - 6.0).abs() < 0.5);
        assert!(ls.response_db(sr, 5000.0).abs() < 0.2);
    }

    #[test]
    fn pitch_of_pulse_train() {
        let sr = 48000;
        let f0 = 120.0;
        let period = sr as f64 / f0;
        let mut x = vec![0.0f32; sr as usize * 2];
        let mut t = 0.0;
        while (t as usize) < x.len() {
            x[t as usize] = 0.8;
            t += period;
        }
        let mut lp = Biquad::lowpass(sr, 2000.0, 0.707);
        lp.run(&mut x);
        let est = estimate_f0(&x, sr).unwrap();
        assert!((est - f0).abs() < 4.0, "f0 {est}");
    }
}
