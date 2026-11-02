//! Turn the guided-test frames into camera measurements.

use super::image::{
    LumaImage, Periodic, dominant_periodic, hue_sat, lin_luminance, laplacian_variance, linear_to_srgb8,
    luma_percentiles, noise_sigma, region_stats, srgb_to_linear, stops,
};
use super::types::{Rect, TaggedFrame, TestStep, VideoTestCapture};
use serde::{Deserialize, Serialize};

/// Overall colour cast seen on the white paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourCast {
    Neutral,
    /// Orange: the light is warmer than the white balance setting.
    Warm,
    /// Blue: the light is cooler than the white balance setting.
    Cool,
    Green,
    Magenta,
}

/// Hint that the colour range is mismatched somewhere in the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeHint {
    /// Blacks sit at ~16 and whites at ~235: limited range shown as full.
    WashedOut,
    /// Lots of near-black with nothing above true black: contrast too high or range squashed.
    Crushed,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FlickerInfo {
    pub detected: bool,
    /// Dark/bright band pairs visible per frame (rolling-shutter banding).
    pub bands_per_frame: Option<f64>,
    /// Band strength relative to the average brightness, in percent.
    pub amplitude_pct: f64,
    /// Whole-frame brightness pulsing from frame to frame.
    pub temporal: bool,
}

/// Everything measured from one guided test.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoMetrics {
    pub frames: usize,
    pub face_found: bool,
    pub paper_found: bool,
    pub frame_width: usize,
    pub frame_height: usize,

    // Exposure
    pub face_mean_luma: Option<f64>,
    pub face_median_luma: Option<f64>,
    /// Percent of face pixels blown to white.
    pub face_clipped_pct: Option<f64>,
    /// Percent of face pixels so dark that detail is lost.
    pub face_crushed_pct: Option<f64>,
    pub background_mean_luma: Option<f64>,
    /// Positive = face brighter than the background.
    pub face_vs_background_stops: Option<f64>,
    /// Background relative to the white paper (incident light at the face).
    /// Above 0 means the background is lit more strongly than you are.
    pub background_vs_paper_stops: Option<f64>,
    /// Percent of all pixels so dark that detail is lost (luma <= 12).
    pub shadow_crushed_pct: f64,

    // Paper (exposure + white balance reference)
    pub paper_luma: Option<f64>,
    pub paper_clipped_pct: Option<f64>,
    /// Linear R/G and B/G of the paper (1.0 = neutral).
    pub paper_rg: Option<f64>,
    pub paper_bg: Option<f64>,
    /// Largest deviation from neutral, percent.
    pub paper_cast_pct: Option<f64>,
    pub paper_cast: Option<ColourCast>,
    /// ln(R/B) of the paper; > 0 warm, < 0 cool. Drives the Kelvin correction.
    pub paper_ln_rb: Option<f64>,

    // Face colour (after correcting by the paper when available)
    pub face_hue_deg: Option<f64>,
    pub face_saturation: Option<f64>,
    /// Face lit by a coloured source (RGB strip, monitor glow).
    pub face_tinted: bool,
    pub face_tint_name: Option<String>,

    // Noise / sharpness
    pub noise_sigma: Option<f64>,
    pub face_sharpness: Option<f64>,
    pub background_sharpness: Option<f64>,
    /// Coefficient of variation of face sharpness while still (focus hunting).
    pub sharpness_cv: Option<f64>,

    // Stability
    /// Largest brightness swing of the (static) background, in stops.
    pub exposure_swing_stops: f64,
    /// Largest colour swing of the background, percent of R/B.
    pub colour_drift_pct: f64,
    pub flicker: FlickerInfo,

    // Timing
    pub delivered_fps: Option<f64>,
    /// Gaps longer than 2.5 frame intervals (freezes / USB dropouts).
    pub dropouts: usize,

    // Lighting shape
    /// Brighter cheek / darker cheek, linear light.
    pub key_fill_ratio: Option<f64>,
    /// Percent of eye-band pixels that are specular (ring light, glasses glare).
    pub eye_glare_pct: Option<f64>,
    /// Eye band vs cheeks in stops (very negative = "raccoon eyes").
    pub eye_to_cheek_stops: Option<f64>,

    // Framing (fractions of the frame)
    pub face_center_x: Option<f64>,
    pub face_height_frac: Option<f64>,
    /// Eye line height from the top, 0..1 (target ~1/3).
    pub eye_line_y: Option<f64>,
    pub face_pitch_deg: Option<f64>,

    // Range
    /// 0.1th percentile of luma.
    pub black_floor_luma: f64,
    /// 99.9th percentile of luma.
    pub white_ceiling_luma: f64,
    pub range_hint: Option<RangeHint>,
}

fn mean(v: &[f64]) -> Option<f64> {
    if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) }
}

fn cv(v: &[f64]) -> Option<f64> {
    let m = mean(v)?;
    if v.len() < 3 || m <= 0.0 {
        return None;
    }
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64;
    Some(var.sqrt() / m)
}

/// Background mask: above the face's bottom edge and outside a widened face
/// box (the wall behind your head), never the paper. Without a face: the
/// whole frame except the paper.
fn background_pred(t: &TaggedFrame) -> impl Fn(usize, usize) -> bool + '_ {
    let f = &t.frame;
    let face_x = t.face.map(|r| r.expand(0.3, 0.15, f.width, f.height));
    let limit_y = t.face.map(|r| r.bottom()).unwrap_or(f.height);
    move |x, y| {
        if y >= limit_y {
            return false;
        }
        if let Some(fr) = face_x {
            if fr.contains(x, y) {
                return false;
            }
        }
        if let Some(p) = t.paper {
            if p.expand(0.1, 0.1, f.width, f.height).contains(x, y) {
                return false;
            }
        }
        true
    }
}

/// Frames used for the steady "how do you look" measurements.
fn steady_frames(c: &VideoTestCapture) -> Vec<&TaggedFrame> {
    let neutral: Vec<&TaggedFrame> =
        c.frames.iter().filter(|t| t.step == TestStep::Neutral && t.frame.is_valid()).collect();
    if !neutral.is_empty() {
        return neutral;
    }
    c.frames.iter().filter(|t| t.frame.is_valid()).collect()
}

/// Measure the guided test.
pub fn measure(capture: &VideoTestCapture, requested_fps: f64) -> VideoMetrics {
    let mut m = VideoMetrics::default();
    let valid: Vec<&TaggedFrame> = capture.frames.iter().filter(|t| t.frame.is_valid()).collect();
    m.frames = valid.len();
    if valid.is_empty() {
        return m;
    }
    m.frame_width = valid[0].frame.width;
    m.frame_height = valid[0].frame.height;
    let steady = steady_frames(capture);

    measure_face_and_background(&steady, &mut m);
    measure_paper(&valid, &mut m);
    measure_face_colour(&steady, &mut m);
    measure_sharpness_and_noise(&valid, &steady, &mut m);
    measure_stability(&valid, &mut m);
    measure_flicker(&valid, &mut m);
    measure_timing(&valid, requested_fps, &mut m);
    measure_range(&steady, &mut m);
    m
}

fn measure_face_and_background(steady: &[&TaggedFrame], m: &mut VideoMetrics) {
    let (mut fmean, mut fmed, mut fclip, mut fcrush, mut flin) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut bmean, mut blin, mut crushed_all) = (vec![], vec![], vec![]);
    let (mut kf, mut glare, mut eyes, mut cx, mut fh, mut eyey, mut pitch) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for t in steady {
        let f = &t.frame;
        let all = region_stats(f, f.full_rect(), |_, _| true);
        crushed_all.push(all.frac_crushed);
        let bg = region_stats(f, f.full_rect(), background_pred(t));
        if bg.n > 0 {
            bmean.push(bg.mean_luma8);
            blin.push(bg.mean_lin);
        }
        let Some(face) = t.face.map(|r| r.clamp_to(f.width, f.height)) else { continue };
        if face.w < 4 || face.h < 4 {
            continue;
        }
        let fs = region_stats(f, face, |x, y| face.in_ellipse(x, y, 0.8));
        if fs.n == 0 {
            continue;
        }
        fmean.push(fs.mean_luma8);
        fmed.push(fs.median_luma8);
        fclip.push(fs.frac_clipped * 100.0);
        fcrush.push(fs.frac_crushed * 100.0);
        flin.push(fs.mean_lin);

        // Cheeks: image-left and image-right lower face.
        let lc = region_stats(f, face.frac(0.15, 0.50, 0.40, 0.72), |_, _| true);
        let rc = region_stats(f, face.frac(0.60, 0.50, 0.85, 0.72), |_, _| true);
        if lc.n > 0 && rc.n > 0 {
            let (hi, lo) = if lc.mean_lin > rc.mean_lin { (lc.mean_lin, rc.mean_lin) } else { (rc.mean_lin, lc.mean_lin) };
            kf.push(hi / lo.max(1e-4));
            let eb = region_stats(f, face.frac(0.15, 0.28, 0.85, 0.45), |_, _| true);
            if eb.n > 0 {
                glare.push(eb.frac_bright * 100.0);
                eyes.push(stops(eb.mean_lin, (lc.mean_lin + rc.mean_lin) / 2.0));
            }
        }
        cx.push((face.x as f64 + face.w as f64 / 2.0) / f.width as f64);
        fh.push(face.h as f64 / f.height as f64);
        eyey.push((face.y as f64 + face.h as f64 * 0.4) / f.height as f64);
        if let Some(p) = t.face_pitch_deg {
            pitch.push(p);
        }
    }
    m.shadow_crushed_pct = mean(&crushed_all).unwrap_or(0.0) * 100.0;
    m.background_mean_luma = mean(&bmean);
    m.face_found = !fmean.is_empty();
    m.face_mean_luma = mean(&fmean);
    m.face_median_luma = mean(&fmed);
    m.face_clipped_pct = mean(&fclip);
    m.face_crushed_pct = mean(&fcrush);
    if let (Some(fl), Some(bl)) = (mean(&flin), mean(&blin)) {
        m.face_vs_background_stops = Some(stops(fl, bl));
    }
    m.key_fill_ratio = mean(&kf);
    m.eye_glare_pct = mean(&glare);
    m.eye_to_cheek_stops = mean(&eyes);
    m.face_center_x = mean(&cx);
    m.face_height_frac = mean(&fh);
    m.eye_line_y = mean(&eyey);
    m.face_pitch_deg = mean(&pitch);
}

fn measure_paper(valid: &[&TaggedFrame], m: &mut VideoMetrics) {
    let (mut lum, mut clip, mut r, mut g, mut b, mut lin, mut bg_rel) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for t in valid.iter().filter(|t| t.paper.is_some()) {
        let f = &t.frame;
        let p = t.paper.unwrap().clamp_to(f.width, f.height);
        // Inner 70% avoids edges and fingers.
        let inner = p.frac(0.15, 0.15, 0.85, 0.85);
        let s = region_stats(f, inner, |_, _| true);
        if s.n == 0 {
            continue;
        }
        lum.push(s.median_luma8);
        clip.push(s.frac_clipped * 100.0);
        r.push(s.mean_lin_rgb[0]);
        g.push(s.mean_lin_rgb[1]);
        b.push(s.mean_lin_rgb[2]);
        lin.push(s.mean_lin);
        let bg = region_stats(f, f.full_rect(), background_pred(t));
        if bg.n > 0 {
            bg_rel.push(stops(bg.mean_lin, s.mean_lin));
        }
    }
    if lum.is_empty() {
        return;
    }
    m.paper_found = true;
    m.paper_luma = mean(&lum);
    m.paper_clipped_pct = mean(&clip);
    let (r, g, b) = (mean(&r).unwrap(), mean(&g).unwrap().max(1e-6), mean(&b).unwrap());
    let rg = r / g;
    let bgr = b / g;
    m.paper_rg = Some(rg);
    m.paper_bg = Some(bgr);
    m.paper_cast_pct = Some(((rg - 1.0).abs().max((bgr - 1.0).abs())) * 100.0);
    m.paper_ln_rb = Some((r.max(1e-6) / b.max(1e-6)).ln());
    m.background_vs_paper_stops = mean(&bg_rel);
    // Name the cast: warm/cool along the red-blue axis, green/magenta otherwise.
    let rb = rg - bgr; // > 0 warm
    let gm = 1.0 - (rg + bgr) / 2.0; // > 0 green (G above R and B)
    m.paper_cast = Some(if m.paper_cast_pct.unwrap() < 4.0 {
        ColourCast::Neutral
    } else if rb.abs() >= gm.abs() {
        if rb > 0.0 { ColourCast::Warm } else { ColourCast::Cool }
    } else if gm > 0.0 {
        ColourCast::Green
    } else {
        ColourCast::Magenta
    });
}

fn measure_face_colour(steady: &[&TaggedFrame], m: &mut VideoMetrics) {
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for t in steady {
        let f = &t.frame;
        let Some(face) = t.face.map(|r| r.clamp_to(f.width, f.height)) else { continue };
        let s = region_stats(f, face, |x, y| face.in_ellipse(x, y, 0.8));
        if s.n == 0 {
            continue;
        }
        for i in 0..3 {
            sum[i] += s.mean_lin_rgb[i];
        }
        n += 1.0;
    }
    if n == 0.0 {
        return;
    }
    let mut lin = [sum[0] / n, sum[1] / n, sum[2] / n];
    // Correct by the paper so a wrong white balance isn't mistaken for a tint.
    if let (Some(rg), Some(bg)) = (m.paper_rg, m.paper_bg) {
        lin[0] /= rg.max(1e-3);
        lin[2] /= bg.max(1e-3);
    }
    let enc = [linear_to_srgb8(lin[0]), linear_to_srgb8(lin[1]), linear_to_srgb8(lin[2])];
    let (h, s) = hue_sat(enc);
    m.face_hue_deg = Some(h);
    m.face_saturation = Some(s);
    // Skin of every tone sits around 5-45 degrees (orange). Outside that, with
    // real saturation, a coloured light is on the face.
    let skin = h <= 50.0 || h >= 345.0;
    if !skin && s > 0.10 {
        m.face_tinted = true;
        m.face_tint_name = Some(
            if h < 170.0 { "green" } else if h < 260.0 { "blue" } else { "purple" }.to_string(),
        );
    }
}

fn measure_sharpness_and_noise(valid: &[&TaggedFrame], steady: &[&TaggedFrame], m: &mut VideoMetrics) {
    let (mut fsh, mut bsh, mut noise) = (vec![], vec![], vec![]);
    for t in steady {
        let img = LumaImage::from_frame(&t.frame);
        if let Some(face) = t.face {
            let inner = face.clamp_to(img.w, img.h).frac(0.2, 0.2, 0.8, 0.8);
            fsh.push(laplacian_variance(&img, inner));
        }
        let bgp = background_pred(t);
        if let Some(n) = noise_sigma(&img, &bgp) {
            noise.push(n);
        }
        // Background sharpness over the background columns above the face.
        let r = match t.face {
            Some(face) => Rect::new(0, 0, face.x.saturating_sub(face.w / 3).max(4), face.bottom()),
            None => t.frame.full_rect(),
        };
        if r.w >= 4 && r.h >= 4 {
            bsh.push(laplacian_variance(&img, r));
        }
    }
    m.face_sharpness = mean(&fsh);
    m.background_sharpness = mean(&bsh);
    m.noise_sigma = mean(&noise);
    // Focus hunting: face sharpness wobbling across frames where you sit still.
    let still: Vec<f64> = valid
        .iter()
        .filter(|t| matches!(t.step, TestStep::Neutral | TestStep::HoldPaper))
        .filter_map(|t| {
            let face = t.face?;
            let img = LumaImage::from_frame(&t.frame);
            Some(laplacian_variance(&img, face.clamp_to(img.w, img.h).frac(0.2, 0.2, 0.8, 0.8)))
        })
        .collect();
    m.sharpness_cv = cv(&still);
}

fn measure_stability(valid: &[&TaggedFrame], m: &mut VideoMetrics) {
    let mut ts: Vec<&&TaggedFrame> = valid.iter().collect();
    ts.sort_by(|a, b| a.frame.timestamp_s.partial_cmp(&b.frame.timestamp_s).unwrap());
    let (mut lums, mut rbs) = (vec![], vec![]);
    for t in ts {
        let s = region_stats(&t.frame, t.frame.full_rect(), background_pred(t));
        if s.n == 0 {
            continue;
        }
        lums.push(s.mean_lin);
        rbs.push(s.mean_lin_rgb[0] / s.mean_lin_rgb[2].max(1e-6));
    }
    // 3-frame moving average so fast flicker doesn't count as pumping.
    let smooth = |v: &[f64]| -> Vec<f64> {
        if v.len() < 3 {
            return v.to_vec();
        }
        v.windows(3).map(|w| (w[0] + w[1] + w[2]) / 3.0).collect()
    };
    let sl = smooth(&lums);
    if let (Some(mx), Some(mn)) = (
        sl.iter().cloned().reduce(f64::max),
        sl.iter().cloned().reduce(f64::min),
    ) {
        m.exposure_swing_stops = stops(mx, mn);
    }
    let sr = smooth(&rbs);
    if let (Some(mx), Some(mn), Some(avg)) =
        (sr.iter().cloned().reduce(f64::max), sr.iter().cloned().reduce(f64::min), mean(&sr))
    {
        m.colour_drift_pct = (mx - mn) / avg.max(1e-6) * 100.0;
    }
}

/// Rolling-shutter bands show as a sinusoid in the row means of the static
/// background; whole-frame pulsing shows in the frame-mean series.
fn measure_flicker(valid: &[&TaggedFrame], m: &mut VideoMetrics) {
    let mut hits: Vec<Periodic> = vec![];
    for t in valid {
        let f = &t.frame;
        let face_cols = t.face.map(|r| r.expand(0.3, 0.0, f.width, f.height));
        let mut rows = Vec::with_capacity(f.height);
        for y in 0..f.height {
            let (mut s, mut n) = (0.0, 0.0);
            for x in 0..f.width {
                if let Some(fc) = face_cols {
                    if x >= fc.x && x < fc.right() {
                        continue;
                    }
                }
                if let Some(p) = t.paper {
                    if p.contains(x, y) {
                        continue;
                    }
                }
                s += lin_luminance(f.px(x, y));
                n += 1.0;
            }
            if n > 0.0 {
                rows.push(s / n);
            }
        }
        if let Some(p) = dominant_periodic(&rows, 2) {
            if p.rel_amplitude > 0.04 && p.concentration > 0.35 {
                hits.push(p);
            }
        }
    }
    let frac = hits.len() as f64 / valid.len().max(1) as f64;
    let mut info = FlickerInfo::default();
    if frac >= 0.6 && !hits.is_empty() {
        let mut cycles: Vec<f64> = hits.iter().map(|p| p.cycles).collect();
        cycles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = cycles[cycles.len() / 2];
        let consistent = cycles.iter().filter(|c| (*c - med).abs() <= 1.0).count() as f64
            / cycles.len() as f64;
        if consistent >= 0.6 {
            info.detected = true;
            info.bands_per_frame = Some(med);
            info.amplitude_pct =
                hits.iter().map(|p| p.rel_amplitude).sum::<f64>() / hits.len() as f64 * 100.0;
        }
    }
    // Temporal: whole-frame brightness pulsing.
    let mut ts: Vec<&&TaggedFrame> = valid.iter().collect();
    ts.sort_by(|a, b| a.frame.timestamp_s.partial_cmp(&b.frame.timestamp_s).unwrap());
    let means: Vec<f64> = ts
        .iter()
        .map(|t| region_stats(&t.frame, t.frame.full_rect(), background_pred(t)).mean_lin)
        .collect();
    if means.len() >= 12 {
        if let Some(p) = dominant_periodic(&means, 2) {
            if p.rel_amplitude > 0.04 && p.concentration > 0.5 {
                info.temporal = true;
                info.detected = true;
                info.amplitude_pct = info.amplitude_pct.max(p.rel_amplitude * 100.0);
            }
        }
    }
    m.flicker = info;
}

fn measure_timing(valid: &[&TaggedFrame], requested_fps: f64, m: &mut VideoMetrics) {
    let mut ts: Vec<f64> = valid.iter().map(|t| t.frame.timestamp_s).collect();
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut iv: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0 && *d < 0.5).collect();
    if iv.len() < 3 {
        return;
    }
    let all = iv.clone();
    iv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = iv[iv.len() / 2];
    m.delivered_fps = Some(1.0 / med);
    let expected = if requested_fps > 0.0 { (1.0 / requested_fps).max(med) } else { med };
    m.dropouts = all.iter().filter(|d| **d > 2.5 * expected).count();
}

fn measure_range(steady: &[&TaggedFrame], m: &mut VideoMetrics) {
    let (mut lo, mut hi) = (vec![], vec![]);
    for t in steady {
        let img = LumaImage::from_frame(&t.frame);
        let p = luma_percentiles(&img, &[0.001, 0.999]);
        lo.push(p[0]);
        hi.push(p[1]);
    }
    m.black_floor_luma = mean(&lo).unwrap_or(0.0);
    m.white_ceiling_luma = mean(&hi).unwrap_or(255.0);
    // Limited-range video shown as full range: nothing below ~16, nothing above ~235.
    if (13.0..=22.0).contains(&m.black_floor_luma) && (220.0..=240.0).contains(&m.white_ceiling_luma) {
        m.range_hint = Some(RangeHint::WashedOut);
    } else if m.shadow_crushed_pct > 8.0 && m.black_floor_luma <= 1.0 {
        m.range_hint = Some(RangeHint::Crushed);
    }
}

/// Convenience used by the recommender: linear value of an 8-bit luma.
pub fn luma_to_linear(l8: f64) -> f64 {
    srgb_to_linear(l8)
}
