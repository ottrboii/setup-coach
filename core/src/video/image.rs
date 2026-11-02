//! Low-level image measurements: luma, linear light, region statistics,
//! sharpness, noise and periodic-pattern (flicker) detection.

use super::types::{Frame, Rect};

/// sRGB 8-bit value to linear light (0..1).
#[inline]
pub fn srgb_to_linear(v: f64) -> f64 {
    let c = (v / 255.0).clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Linear light (0..1) to an sRGB 8-bit value (0..255, not rounded).
#[inline]
pub fn linear_to_srgb8(l: f64) -> f64 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    c * 255.0
}

/// Rec.709 luma on gamma-encoded 8-bit values (what a waveform shows).
#[inline]
pub fn luma8(p: [u8; 3]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// Relative luminance in linear light.
#[inline]
pub fn lin_luminance(p: [u8; 3]) -> f64 {
    0.2126 * srgb_to_linear(p[0] as f64)
        + 0.7152 * srgb_to_linear(p[1] as f64)
        + 0.0722 * srgb_to_linear(p[2] as f64)
}

/// Stops between two linear values (positive = `a` brighter).
#[inline]
pub fn stops(a: f64, b: f64) -> f64 {
    (a.max(1e-6) / b.max(1e-6)).log2()
}

/// Luma plane of a frame (gamma-encoded, 0..255).
pub struct LumaImage {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f64>,
}

impl LumaImage {
    pub fn from_frame(f: &Frame) -> Self {
        let mut data = Vec::with_capacity(f.width * f.height);
        for y in 0..f.height {
            for x in 0..f.width {
                data.push(luma8(f.px(x, y)));
            }
        }
        LumaImage { w: f.width, h: f.height, data }
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.w + x]
    }
}

/// Statistics of a set of pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RegionStats {
    pub n: usize,
    /// Mean of linear R, G, B.
    pub mean_lin_rgb: [f64; 3],
    /// Mean linear luminance.
    pub mean_lin: f64,
    /// Mean gamma luma (0..255).
    pub mean_luma8: f64,
    pub median_luma8: f64,
    /// Fraction of pixels with any channel >= 250 (blown out).
    pub frac_clipped: f64,
    /// Fraction of pixels with luma <= 12 (shadow detail lost).
    pub frac_crushed: f64,
    /// Fraction of pixels with luma >= 245 (specular / very bright).
    pub frac_bright: f64,
}

/// Measure the pixels of `rect` for which `include(x, y)` is true.
pub fn region_stats<F: Fn(usize, usize) -> bool>(f: &Frame, rect: Rect, include: F) -> RegionStats {
    let r = rect.clamp_to(f.width, f.height);
    let mut hist = [0usize; 256];
    let mut s = RegionStats::default();
    let mut sum_rgb = [0.0f64; 3];
    let (mut sum_lin, mut sum_l8) = (0.0, 0.0);
    let (mut clipped, mut crushed, mut bright) = (0usize, 0usize, 0usize);
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if !include(x, y) {
                continue;
            }
            let p = f.px(x, y);
            let l8 = luma8(p);
            let lr = srgb_to_linear(p[0] as f64);
            let lg = srgb_to_linear(p[1] as f64);
            let lb = srgb_to_linear(p[2] as f64);
            sum_rgb[0] += lr;
            sum_rgb[1] += lg;
            sum_rgb[2] += lb;
            sum_lin += 0.2126 * lr + 0.7152 * lg + 0.0722 * lb;
            sum_l8 += l8;
            hist[l8.round().clamp(0.0, 255.0) as usize] += 1;
            if p.iter().any(|&c| c >= 250) {
                clipped += 1;
            }
            if l8 <= 12.0 {
                crushed += 1;
            }
            if l8 >= 245.0 {
                bright += 1;
            }
            s.n += 1;
        }
    }
    if s.n == 0 {
        return s;
    }
    let n = s.n as f64;
    s.mean_lin_rgb = [sum_rgb[0] / n, sum_rgb[1] / n, sum_rgb[2] / n];
    s.mean_lin = sum_lin / n;
    s.mean_luma8 = sum_l8 / n;
    s.median_luma8 = hist_percentile(&hist, s.n, 0.5);
    s.frac_clipped = clipped as f64 / n;
    s.frac_crushed = crushed as f64 / n;
    s.frac_bright = bright as f64 / n;
    s
}

/// Percentile (0..1) from a 256-bin histogram.
pub fn hist_percentile(hist: &[usize; 256], total: usize, p: f64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let target = (p * total as f64).ceil().max(1.0) as usize;
    let mut acc = 0;
    for (i, c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            return i as f64;
        }
    }
    255.0
}

/// Whole-frame luma percentiles (0..1 each).
pub fn luma_percentiles(img: &LumaImage, ps: &[f64]) -> Vec<f64> {
    let mut hist = [0usize; 256];
    for v in &img.data {
        hist[v.round().clamp(0.0, 255.0) as usize] += 1;
    }
    ps.iter().map(|&p| hist_percentile(&hist, img.data.len(), p)).collect()
}

/// Variance of the 4-neighbour Laplacian inside `rect` (focus measure).
pub fn laplacian_variance(img: &LumaImage, rect: Rect) -> f64 {
    let r = rect.clamp_to(img.w, img.h);
    if r.w < 3 || r.h < 3 {
        return 0.0;
    }
    let (mut sum, mut sum2, mut n) = (0.0, 0.0, 0usize);
    for y in (r.y + 1)..(r.bottom() - 1) {
        for x in (r.x + 1)..(r.right() - 1) {
            let l = img.at(x - 1, y) + img.at(x + 1, y) + img.at(x, y - 1) + img.at(x, y + 1)
                - 4.0 * img.at(x, y);
            sum += l;
            sum2 += l * l;
            n += 1;
        }
    }
    if n == 0 {
        return 0.0;
    }
    let mean = sum / n as f64;
    (sum2 / n as f64 - mean * mean).max(0.0)
}

/// Estimate sensor noise (std dev in 8-bit luma units) from the flattest
/// 8x8 blocks of the pixels selected by `include`.
pub fn noise_sigma<F: Fn(usize, usize) -> bool>(img: &LumaImage, include: F) -> Option<f64> {
    const B: usize = 8;
    let mut block_sigmas = Vec::new();
    let mut by = 1;
    while by + B < img.h {
        let mut bx = 1;
        while bx + B < img.w {
            let mut ok = true;
            'chk: for y in by..by + B {
                for x in bx..bx + B {
                    if !include(x, y) {
                        ok = false;
                        break 'chk;
                    }
                }
            }
            if ok {
                let (mut s, mut s2, mut n) = (0.0, 0.0, 0.0);
                for y in by..by + B {
                    for x in bx..bx + B {
                        let mut m = 0.0;
                        for dy in 0..3 {
                            for dx in 0..3 {
                                m += img.at(x + dx - 1, y + dy - 1);
                            }
                        }
                        let r = img.at(x, y) - m / 9.0;
                        s += r;
                        s2 += r * r;
                        n += 1.0;
                    }
                }
                let mean = s / n;
                block_sigmas.push((s2 / n - mean * mean).max(0.0).sqrt());
            }
            bx += B;
        }
        by += B;
    }
    if block_sigmas.is_empty() {
        return None;
    }
    block_sigmas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = block_sigmas[(block_sigmas.len() as f64 * 0.25) as usize];
    // Residual from a 3x3 mean has std ~0.943 sigma for white noise.
    Some(q / 0.943)
}

/// Dominant periodic component of a 1-D signal after removing its mean and
/// linear trend.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Periodic {
    /// Cycles over the whole signal length.
    pub cycles: f64,
    /// Peak amplitude relative to the signal mean (0.05 = 5%).
    pub rel_amplitude: f64,
    /// Share of the (non-DC) spectral power in the peak bin and its neighbours.
    pub concentration: f64,
    /// Phase of the peak (radians).
    pub phase: f64,
}

/// Find the strongest periodic component with at least `min_cycles` cycles.
pub fn dominant_periodic(signal: &[f64], min_cycles: usize) -> Option<Periodic> {
    let n = signal.len();
    if n < 8 {
        return None;
    }
    let mean = signal.iter().sum::<f64>() / n as f64;
    if mean.abs() < 1e-9 {
        return None;
    }
    // Remove linear trend (least squares).
    let xm = (n as f64 - 1.0) / 2.0;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, v) in signal.iter().enumerate() {
        let dx = i as f64 - xm;
        sxy += dx * (v - mean);
        sxx += dx * dx;
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let detr: Vec<f64> =
        signal.iter().enumerate().map(|(i, v)| v - mean - slope * (i as f64 - xm)).collect();
    let kmax = n / 2;
    let mut power = vec![0.0; kmax + 1];
    let mut phases = vec![0.0; kmax + 1];
    let mut amps = vec![0.0; kmax + 1];
    for k in 1..=kmax {
        let (mut re, mut im) = (0.0, 0.0);
        let w = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
        for (i, v) in detr.iter().enumerate() {
            re += v * (w * i as f64).cos();
            im -= v * (w * i as f64).sin();
        }
        power[k] = re * re + im * im;
        amps[k] = 2.0 * (re * re + im * im).sqrt() / n as f64;
        phases[k] = im.atan2(re);
    }
    let total: f64 = power[1..].iter().sum();
    if total <= 0.0 {
        return None;
    }
    let start = min_cycles.max(1);
    if start > kmax {
        return None;
    }
    let (mut best, mut best_p) = (start, -1.0);
    for k in start..=kmax {
        if power[k] > best_p {
            best_p = power[k];
            best = k;
        }
    }
    let lo = best.saturating_sub(1).max(1);
    let hi = (best + 1).min(kmax);
    let local: f64 = power[lo..=hi].iter().sum();
    Some(Periodic {
        cycles: best as f64,
        rel_amplitude: amps[lo..=hi].iter().cloned().fold(0.0, f64::max) / mean.abs(),
        concentration: local / total,
        phase: phases[best],
    })
}

/// Hue (degrees 0..360) and saturation (0..1) of an RGB triple (any scale).
pub fn hue_sat(rgb: [f64; 3]) -> (f64, f64) {
    let max = rgb.iter().cloned().fold(f64::MIN, f64::max);
    let min = rgb.iter().cloned().fold(f64::MAX, f64::min);
    if max <= 0.0 {
        return (0.0, 0.0);
    }
    let d = max - min;
    let sat = d / max;
    if d <= 1e-12 {
        return (0.0, sat);
    }
    let [r, g, b] = rgb;
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    ((h + 360.0) % 360.0, sat)
}
