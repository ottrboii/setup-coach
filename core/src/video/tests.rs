//! Tests on synthetic guided-test captures: a face-like ellipse with skin
//! tone and features, a textured wall, a shirt, a white paper during the
//! paper step, sensor noise, and per-test faults.

use super::*;
use crate::constraints::Constraint;
use crate::finding::{FixKind, Severity};

const W: usize = 160;
const H: usize = 120;

fn face_rect() -> Rect {
    Rect::new(60, 15, 40, 50)
}
fn paper_rect() -> Rect {
    Rect::new(112, 72, 34, 34)
}

struct Lcg(u64);
impl Lcg {
    fn uniform(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn gauss(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-12);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

#[derive(Clone)]
struct Scene {
    wall: [f64; 3],
    skin: [f64; 3],
    paper: [f64; 3],
    shirt: [f64; 3],
    noise: f64,
    face_detail: f64,
    wall_detail: f64,
    wall_period: f64,
    /// Alternate sharp/soft face every other frame (focus hunting).
    hunt: bool,
    /// Draw sharp-edged eyes and mouth (a defocused face has none).
    features: bool,
    face: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            wall: [140.0, 135.0, 130.0],
            skin: [220.0, 170.0, 145.0],
            paper: [225.0, 225.0, 225.0],
            shirt: [60.0, 60.0, 70.0],
            noise: 1.5,
            face_detail: 0.05,
            wall_detail: 0.08,
            wall_period: 9.0,
            hunt: false,
            features: true,
            face: true,
        }
    }
}

const STEPS: [(TestStep, usize); 4] =
    [(TestStep::Neutral, 12), (TestStep::TurnHead, 6), (TestStep::HoldPaper, 8), (TestStep::LeanInOut, 6)];

type Mod<'a> = &'a dyn Fn(usize, usize, usize, &mut [f64; 3]);

fn base_pixel(sc: &Scene, x: usize, y: usize, idx: usize, paper_on: bool) -> [f64; 3] {
    let fr = face_rect();
    if paper_on && paper_rect().contains(x, y) {
        return sc.paper;
    }
    if sc.face && fr.in_ellipse(x, y, 1.0) {
        let soft = sc.hunt && idx % 2 == 1;
        let detail = if soft { 0.0 } else { sc.face_detail };
        let features = sc.features && !soft;
        let fx = (x as f64 - fr.x as f64) / fr.w as f64;
        let fy = (y as f64 - fr.y as f64) / fr.h as f64;
        let mut k = 1.0 + detail * (2.0 * std::f64::consts::PI * (x + y) as f64 / 5.0).sin();
        let eye = |cx: f64| ((fx - cx) / 0.08).powi(2) + ((fy - 0.37) / 0.05).powi(2) <= 1.0;
        if features && (eye(0.33) || eye(0.67)) {
            k *= 0.35;
        }
        if features && ((fx - 0.5) / 0.15).powi(2) + ((fy - 0.75) / 0.04).powi(2) <= 1.0 {
            k *= 0.6;
        }
        return [sc.skin[0] * k, sc.skin[1] * k, sc.skin[2] * k];
    }
    if sc.face && y >= fr.bottom() && (45..115).contains(&x) {
        return sc.shirt;
    }
    let k = 1.0 + sc.wall_detail * (2.0 * std::f64::consts::PI * x as f64 / sc.wall_period).sin();
    [sc.wall[0] * k, sc.wall[1] * k, sc.wall[2] * k]
}

fn render(sc: &Scene, step: TestStep, idx: usize, t: f64, rng: &mut Lcg, m: Mod) -> TaggedFrame {
    let paper_on = step == TestStep::HoldPaper;
    let mut rgb = Vec::with_capacity(W * H * 3);
    for y in 0..H {
        for x in 0..W {
            let mut p = base_pixel(sc, x, y, idx, paper_on);
            m(x, y, idx, &mut p);
            for c in p {
                let v = c + sc.noise * rng.gauss();
                rgb.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    let mut tf = TaggedFrame::new(step, Frame { width: W, height: H, rgb, timestamp_s: t });
    if sc.face {
        tf = tf.with_face(face_rect());
    }
    if paper_on {
        tf = tf.with_paper(paper_rect());
    }
    tf
}

fn capture_with(sc: &Scene, fps: f64, m: Mod, timestamps: Option<&dyn Fn(usize) -> f64>) -> VideoTestCapture {
    let mut rng = Lcg(42);
    let mut frames = Vec::new();
    let mut idx = 0;
    for (step, n) in STEPS {
        for _ in 0..n {
            let t = match timestamps {
                Some(f) => f(idx),
                None => idx as f64 / fps,
            };
            frames.push(render(sc, step, idx, t, &mut rng, m));
            idx += 1;
        }
    }
    VideoTestCapture { frames }
}

fn capture(sc: &Scene, m: Mod) -> VideoTestCapture {
    capture_with(sc, 30.0, m, None)
}

fn no_mod(_: usize, _: usize, _: usize, _: &mut [f64; 3]) {}

/// A webcam already locked to sensible manual settings for 50 Hz mains.
fn locked_state() -> CameraState {
    CameraState {
        exposure_log2: Some((1.0f64 / 50.0).log2()),
        auto_exposure: false,
        ae_priority: false,
        gain: Some(30.0),
        wb_kelvin: Some(5000.0),
        auto_wb: false,
        focus: Some(30.0),
        auto_focus: false,
        sharpness: Some(128.0),
        saturation: Some(128.0),
        contrast: Some(128.0),
        backlight_comp: Some(0.0),
        powerline_hz: Some(50),
        hdr: None,
        requested_fps: 30.0,
        requested_width: 1920,
        requested_height: 1080,
        format: VideoFormat::Mjpeg,
        exposure_range: None,
        gain_range: Some(ControlRange::new(0.0, 255.0, 1.0, 0.0)),
        wb_range: Some(ControlRange::new(2000.0, 6500.0, 10.0, 4000.0)),
        focus_range: Some(ControlRange::new(0.0, 250.0, 5.0, 0.0)),
        sharpness_range: Some(ControlRange::new(0.0, 255.0, 1.0, 128.0)),
        saturation_range: Some(ControlRange::new(0.0, 255.0, 1.0, 128.0)),
        backlight_range: Some(ControlRange::new(0.0, 1.0, 1.0, 0.0)),
    }
}

fn run(sc: &Scene, m: Mod, st: &CameraState) -> (VideoMetrics, Vec<crate::finding::Finding>) {
    analyze(&capture(sc, m), st, CameraClass::FullUvc, MainsHz::Hz50)
}

fn ids(f: &[crate::finding::Finding]) -> Vec<String> {
    f.iter().map(|x| x.id.clone()).collect()
}

fn has(f: &[crate::finding::Finding], id: &str) -> bool {
    f.iter().any(|x| x.id == id)
}

// ---------------------------------------------------------------------------
// Good setup: nothing that costs points.

#[test]
fn good_setup_has_no_problems() {
    let (m, f) = run(&Scene::default(), &no_mod, &locked_state());
    let bad: Vec<_> = f.iter().filter(|x| x.severity >= Severity::Minor).map(|x| x.id.clone()).collect();
    assert!(bad.is_empty(), "unexpected findings {bad:?}\nmetrics {m:#?}");
    assert!(m.face_found && m.paper_found);
    assert!((m.paper_luma.unwrap() - 225.0).abs() < 3.0);
    assert!(!m.flicker.detected);
    assert!((m.delivered_fps.unwrap() - 30.0).abs() < 0.5);
}

#[test]
fn out_of_the_box_webcam_flags_auto_settings_and_wrong_antiflicker() {
    let mut st = CameraState { powerline_hz: Some(60), ..CameraState::default() };
    st.gain_range = Some(ControlRange::new(0.0, 255.0, 1.0, 0.0));
    let (_, f) = run(&Scene::default(), &no_mod, &st);
    assert!(has(&f, "camera.auto_exposure_on"), "{:?}", ids(&f));
    assert!(has(&f, "camera.auto_wb_on"));
    let af = f.iter().find(|x| x.id == "camera.anti_flicker_wrong").expect("anti-flicker");
    assert_eq!(af.fix_kind, FixKind::Auto);
}

// ---------------------------------------------------------------------------
// Exposure and skin-tone adaptivity.

#[test]
fn window_behind_is_backlit() {
    let sc = Scene { wall: [240.0, 238.0, 235.0], wall_detail: 0.02, ..Scene::default() };
    let (m, f) = run(&sc, &no_mod, &locked_state());
    assert!(m.background_vs_paper_stops.unwrap() > 0.0);
    assert!(has(&f, "camera.backlit"), "{:?}", ids(&f));
}

#[test]
fn dark_skin_correctly_lit_is_not_flagged() {
    for skin in [[95.0, 62.0, 48.0], [60.0, 38.0, 30.0]] {
        let sc = Scene { skin, ..Scene::default() };
        let (m, f) = run(&sc, &no_mod, &locked_state());
        for id in ["camera.face_needs_light", "camera.backlit", "camera.underexposed", "camera.too_dark_grainy", "camera.face_tint"] {
            assert!(!has(&f, id), "skin {skin:?} flagged {id}; metrics {m:#?}");
        }
    }
}

#[test]
fn face_in_shadow_needs_light() {
    let sc = Scene { skin: [14.0, 10.0, 8.0], ..Scene::default() };
    let (_, f) = run(&sc, &no_mod, &locked_state());
    let x = f.iter().find(|x| x.id == "camera.face_needs_light").expect("face_needs_light");
    assert_eq!(x.fix_kind, FixKind::Manual);
    assert!(x.can_decline);
}

#[test]
fn blown_out_face_is_flagged() {
    let sc = Scene { skin: [255.0, 240.0, 228.0], face_detail: 0.0, ..Scene::default() };
    let (_, f) = run(&sc, &no_mod, &locked_state());
    assert!(has(&f, "camera.face_clipping"), "{:?}", ids(&f));
}

#[test]
fn dark_picture_with_low_gain_is_underexposed_not_light_limited() {
    let sc = Scene { paper: [170.0, 170.0, 170.0], ..Scene::default() };
    let st = CameraState { exposure_log2: Some(-7.0), gain: Some(20.0), ..locked_state() };
    let (_, f) = run(&sc, &no_mod, &st);
    assert!(has(&f, "camera.underexposed"), "{:?}", ids(&f));
    assert!(!has(&f, "camera.too_dark_grainy"));
}

#[test]
fn grainy_dark_room_is_light_limited() {
    let sc = Scene { paper: [170.0, 170.0, 170.0], noise: 9.0, ..Scene::default() };
    let st = CameraState { gain: Some(200.0), ..locked_state() };
    let (m, f) = run(&sc, &no_mod, &st);
    assert!(m.noise_sigma.unwrap() > 6.0, "noise {:?}", m.noise_sigma);
    let x = f.iter().find(|x| x.id == "camera.too_dark_grainy").expect("too dark");
    assert_eq!(x.suggest_product.as_deref(), Some("key_light"));
}

#[test]
fn blown_paper_is_overexposed() {
    let sc = Scene { paper: [253.0, 253.0, 253.0], ..Scene::default() };
    let (_, f) = run(&sc, &no_mod, &locked_state());
    assert!(has(&f, "camera.overexposed"), "{:?}", ids(&f));
}

#[test]
fn auto_exposure_pumping_is_flagged() {
    let pump = |_: usize, _: usize, idx: usize, p: &mut [f64; 3]| {
        let k = if idx >= 26 { 0.55 } else { 1.0 };
        for c in p.iter_mut() {
            *c *= k;
        }
    };
    let st = CameraState { auto_exposure: true, ..locked_state() };
    let (m, f) = run(&Scene::default(), &pump, &st);
    assert!(m.exposure_swing_stops > 0.35, "{}", m.exposure_swing_stops);
    let x = f.iter().find(|x| x.id == "camera.exposure_pumping").expect("pumping");
    assert_eq!(x.fix_kind, FixKind::Auto);
}

// ---------------------------------------------------------------------------
// Colour.

#[test]
fn warm_cast_is_flagged_and_kelvin_goes_down() {
    let warm = |_: usize, _: usize, _: usize, p: &mut [f64; 3]| {
        p[0] *= 1.15;
        p[2] *= 0.8;
    };
    let sc = Scene { paper: [200.0, 200.0, 200.0], ..Scene::default() };
    let (m, f) = run(&sc, &warm, &locked_state());
    assert_eq!(m.paper_cast, Some(ColourCast::Warm));
    assert!(has(&f, "camera.colour_cast"), "{:?}", ids(&f));
    assert!(!m.face_tinted, "white balance error must not be read as a face tint");
    let rec = recommend_settings(&m, &locked_state(), CameraClass::FullUvc, MainsHz::Hz50, &[]);
    let k = rec.uvc.unwrap().wb_kelvin.unwrap();
    assert!(k < 4500.0, "kelvin {k}");
}

#[test]
fn cool_cast_moves_kelvin_up() {
    let cool = |_: usize, _: usize, _: usize, p: &mut [f64; 3]| {
        p[0] *= 0.85;
        p[2] *= 1.1;
    };
    let sc = Scene { paper: [200.0, 200.0, 200.0], ..Scene::default() };
    let st = CameraState { wb_kelvin: Some(4000.0), ..locked_state() };
    let (m, _) = run(&sc, &cool, &st);
    assert_eq!(m.paper_cast, Some(ColourCast::Cool));
    let k = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]).uvc.unwrap().wb_kelvin.unwrap();
    assert!(k > 4000.0, "kelvin {k}");
}

#[test]
fn rgb_light_on_face_is_a_tint() {
    let sc = Scene { skin: [110.0, 175.0, 120.0], ..Scene::default() };
    let (m, f) = run(&sc, &no_mod, &locked_state());
    assert!(m.face_tinted);
    assert_eq!(m.face_tint_name.as_deref(), Some("green"));
    assert!(has(&f, "camera.face_tint"));
}

#[test]
fn auto_wb_drift_is_flagged() {
    let drift = |_: usize, _: usize, idx: usize, p: &mut [f64; 3]| {
        let k = 1.0 + 0.15 * (idx as f64 / 4.0).sin();
        p[0] *= k;
        p[2] /= k;
    };
    let st = CameraState { auto_wb: true, ..locked_state() };
    let (m, f) = run(&Scene::default(), &drift, &st);
    assert!(m.colour_drift_pct > 8.0, "{}", m.colour_drift_pct);
    assert!(has(&f, "camera.colour_drift"), "{:?}", ids(&f));
}

// ---------------------------------------------------------------------------
// Flicker.

#[test]
fn rolling_bands_are_detected() {
    let bands = |_: usize, y: usize, idx: usize, p: &mut [f64; 3]| {
        let k = 1.0 + 0.15 * (2.0 * std::f64::consts::PI * (3.0 * y as f64 / H as f64 + idx as f64 * 0.37)).sin();
        for c in p.iter_mut() {
            *c *= k;
        }
    };
    let (m, f) = run(&Scene::default(), &bands, &locked_state());
    assert!(m.flicker.detected, "{:?}", m.flicker);
    assert!((m.flicker.bands_per_frame.unwrap() - 3.0).abs() <= 1.0);
    assert!(has(&f, "camera.flicker"));
}

#[test]
fn flicker_safe_shutters_follow_mains() {
    let s50 = flicker_safe_shutters(MainsHz::Hz50, 30.0);
    let s60 = flicker_safe_shutters(MainsHz::Hz60, 30.0);
    for t in &s50 {
        assert!(((t * 100.0) - (t * 100.0).round()).abs() < 1e-9 && *t <= 1.0 / 30.0 + 1e-9);
    }
    for t in &s60 {
        assert!(((t * 120.0) - (t * 120.0).round()).abs() < 1e-9 && *t <= 1.0 / 30.0 + 1e-9);
    }
    assert!(s50.contains(&0.02) && s50.contains(&0.01));
    assert!(s60.iter().any(|t| (t - 1.0 / 60.0).abs() < 1e-12));
    // 60 fps under 50 Hz lights: only 1/100 fits in a frame.
    assert_eq!(flicker_safe_shutters(MainsHz::Hz50, 60.0), vec![0.01]);
}

// ---------------------------------------------------------------------------
// Focus and sharpness.

#[test]
fn background_in_focus_face_soft() {
    let sc = Scene { face_detail: 0.0, features: false, wall_detail: 0.3, wall_period: 4.0, ..Scene::default() };
    let (m, f) = run(&sc, &no_mod, &locked_state());
    assert!(has(&f, "camera.soft_focus"), "{:?} face {:?} bg {:?}", ids(&f), m.face_sharpness, m.background_sharpness);
}

#[test]
fn focus_hunting_with_autofocus() {
    let sc = Scene { hunt: true, face_detail: 0.12, ..Scene::default() };
    let st = CameraState { auto_focus: true, ..locked_state() };
    let (m, f) = run(&sc, &no_mod, &st);
    assert!(m.sharpness_cv.unwrap() > 0.35, "{:?}", m.sharpness_cv);
    assert!(has(&f, "camera.focus_hunting"));
}

#[test]
fn best_focus_finds_peak() {
    let samples: Vec<(f64, f64)> = (0..=10).map(|i| {
        let x = i as f64 * 10.0;
        (x, 100.0 - (x - 42.0).powi(2) * 0.05)
    }).collect();
    let b = best_focus(&samples).unwrap();
    assert!((b - 42.0).abs() < 1.0, "{b}");
}

// ---------------------------------------------------------------------------
// Timing.

#[test]
fn half_frame_rate_is_choppy() {
    let st = CameraState { ae_priority: true, ..locked_state() };
    let ts = |i: usize| i as f64 / 15.0;
    let c = capture_with(&Scene::default(), 30.0, &no_mod, Some(&ts));
    let (m, f) = analyze(&c, &st, CameraClass::FullUvc, MainsHz::Hz50);
    assert!((m.delivered_fps.unwrap() - 15.0).abs() < 0.5);
    let x = f.iter().find(|x| x.id == "camera.low_fps").expect("low fps");
    assert!(x.why.contains("Low light compensation"));
}

#[test]
fn freezes_are_dropouts() {
    let ts = |i: usize| i as f64 / 30.0 + if i > 5 { 0.2 } else { 0.0 } + if i > 15 { 0.2 } else { 0.0 } + if i > 25 { 0.2 } else { 0.0 };
    let c = capture_with(&Scene::default(), 30.0, &no_mod, Some(&ts));
    let (m, f) = analyze(&c, &locked_state(), CameraClass::FullUvc, MainsHz::Hz50);
    assert_eq!(m.dropouts, 3);
    assert!(has(&f, "camera.dropouts"));
}

#[test]
fn yuy2_at_1080p_is_flagged_and_switched_to_mjpeg() {
    let st = CameraState { format: VideoFormat::Yuy2, ..locked_state() };
    let (m, f) = run(&Scene::default(), &no_mod, &st);
    assert!(has(&f, "camera.yuy2_bandwidth"));
    let rec = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]);
    assert_eq!(rec.format.format, VideoFormat::Mjpeg);
}

// ---------------------------------------------------------------------------
// Lighting shape, glare, range, framing.

#[test]
fn one_sided_light_is_harsh() {
    let side = |x: usize, y: usize, _: usize, p: &mut [f64; 3]| {
        let fr = face_rect();
        if fr.contains(x, y) && x < fr.x + fr.w / 2 {
            for c in p.iter_mut() {
                *c *= 0.4;
            }
        }
    };
    let (m, f) = run(&Scene::default(), &side, &locked_state());
    assert!(m.key_fill_ratio.unwrap() > 4.0, "{:?}", m.key_fill_ratio);
    assert!(has(&f, "camera.harsh_shadows"));
}

#[test]
fn overhead_light_gives_raccoon_eyes() {
    let eyes = |x: usize, y: usize, _: usize, p: &mut [f64; 3]| {
        let fr = face_rect();
        let band = fr.frac(0.0, 0.26, 1.0, 0.47);
        if band.contains(x, y) {
            for c in p.iter_mut() {
                *c *= 0.35;
            }
        }
    };
    let (m, f) = run(&Scene::default(), &eyes, &locked_state());
    assert!(m.eye_to_cheek_stops.unwrap() < -1.2);
    let x = f.iter().find(|x| x.id == "camera.harsh_shadows").expect("harsh");
    assert_eq!(x.title, "Your eyes are in dark shadows");
}

#[test]
fn ring_light_glare_in_eyes() {
    let glare = |x: usize, y: usize, _: usize, p: &mut [f64; 3]| {
        let fr = face_rect();
        for cx in [0.33, 0.67] {
            let gx = (fr.x as f64 + fr.w as f64 * cx) as usize;
            let gy = (fr.y as f64 + fr.h as f64 * 0.37) as usize;
            if (gx..gx + 3).contains(&x) && (gy..gy + 2).contains(&y) {
                *p = [255.0, 255.0, 255.0];
            }
        }
    };
    let (m, f) = run(&Scene::default(), &glare, &locked_state());
    assert!(m.eye_glare_pct.unwrap() > 0.5);
    assert!(has(&f, "camera.glare"));
    assert!(!has(&f, "camera.face_clipping"));
}

#[test]
fn limited_range_shown_as_full_is_washed_out() {
    let squash = |x: usize, y: usize, _: usize, p: &mut [f64; 3]| {
        if x < 20 && y >= 100 {
            *p = [0.0, 0.0, 0.0];
        } else if (20..40).contains(&x) && y >= 100 {
            *p = [255.0, 255.0, 255.0];
        }
        for c in p.iter_mut() {
            *c = 16.0 + *c * 219.0 / 255.0;
        }
    };
    let sc = Scene { noise: 1.0, ..Scene::default() };
    let (m, f) = run(&sc, &squash, &locked_state());
    assert_eq!(m.range_hint, Some(RangeHint::WashedOut), "floor {} ceil {}", m.black_floor_luma, m.white_ceiling_luma);
    assert!(has(&f, "camera.range_washed_out"));
}

#[test]
fn camera_below_eye_level_from_pitch() {
    let mut c = capture(&Scene::default(), &no_mod);
    for t in &mut c.frames {
        t.face_pitch_deg = Some(20.0);
    }
    let (_, f) = analyze(&c, &locked_state(), CameraClass::FullUvc, MainsHz::Hz50);
    assert!(has(&f, "camera.camera_low"));
}

#[test]
fn no_frames_is_critical() {
    let (_, f) = analyze(&VideoTestCapture::default(), &locked_state(), CameraClass::FullUvc, MainsHz::Hz50);
    assert_eq!(f[0].id, "camera.no_video");
    assert_eq!(f[0].severity, Severity::Critical);
}

// ---------------------------------------------------------------------------
// Recommender.

fn metrics_with_paper(paper: f64) -> VideoMetrics {
    let sc = Scene { paper: [paper, paper, paper], ..Scene::default() };
    measure(&capture(&sc, &no_mod), 30.0)
}

#[test]
fn recommends_flicker_safe_shutter_per_mains() {
    let m = metrics_with_paper(150.0);
    let st = CameraState { exposure_log2: Some((0.01f64).log2()), gain: Some(0.0), ..locked_state() };
    let r50 = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]);
    let u50 = r50.uvc.unwrap();
    let t50 = u50.target_shutter_s.unwrap();
    assert!((t50 - 0.02).abs() < 1e-9, "{t50}");
    assert!(!r50.light_limited);
    assert!(u50.gain.unwrap() < 40.0);
    assert!(!u50.auto_exposure && !u50.ae_priority && !u50.auto_focus && !u50.auto_wb);
    assert_eq!(u50.powerline_hz, Some(50));

    let r60 = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz60, &[]);
    let u60 = r60.uvc.unwrap();
    let t60 = u60.target_shutter_s.unwrap();
    assert!(((t60 * 120.0) - (t60 * 120.0).round()).abs() < 1e-9, "{t60}");
    assert!(t60 <= 1.0 / 30.0 + 1e-9);
    assert_eq!(u60.powerline_hz, Some(60));
}

#[test]
fn light_limited_60fps_drops_to_30() {
    let m = metrics_with_paper(90.0);
    let st = CameraState { exposure_log2: Some((0.01f64).log2()), gain: Some(0.0), requested_fps: 60.0, ..locked_state() };
    let r = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]);
    assert_eq!(r.format.fps, 30.0);
    assert!(r.notes.iter().any(|n| n.contains("30 fps")));
    let t = r.uvc.unwrap().target_shutter_s.unwrap();
    assert!(t <= 1.0 / 30.0 + 1e-9 && t > 1.0 / 60.0);
}

#[test]
fn cant_add_light_allows_more_gain() {
    let m = metrics_with_paper(40.0);
    let st = CameraState { exposure_log2: Some((0.01f64).log2()), gain: Some(0.0), ..locked_state() };
    let normal = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]);
    let constrained = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[Constraint::CantAddLight]);
    assert!(normal.light_limited && constrained.light_limited);
    let (g1, g2) = (normal.uvc.unwrap().gain.unwrap(), constrained.uvc.as_ref().unwrap().gain.unwrap());
    assert!(g2 > g1, "{g1} vs {g2}");
    assert!(normal.coach_steps.iter().any(|s| s.contains("Add light")));
    assert!(constrained.notes.iter().any(|s| s.contains("can't add light")));
    // Slowest flicker-safe shutter for 30 fps at 50 Hz.
    assert!((constrained.uvc.unwrap().target_shutter_s.unwrap() - 0.03).abs() < 1e-9);
}

#[test]
fn cant_move_backlight_exposes_for_face_with_backlight_comp() {
    let sc = Scene { wall: [240.0, 238.0, 235.0], wall_detail: 0.02, ..Scene::default() };
    let m = measure(&capture(&sc, &no_mod), 30.0);
    let st = CameraState { backlight_range: Some(ControlRange::new(0.0, 2.0, 1.0, 0.0)), ..locked_state() };
    let r = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[Constraint::CantMoveBacklight]);
    assert_eq!(r.uvc.unwrap().backlight_comp, Some(1.0));
    assert!(r.notes.iter().any(|n| n.contains("exposure is set for your face")));
}

#[test]
fn sharpness_and_saturation_never_go_above_default() {
    let m = metrics_with_paper(225.0);
    let st = CameraState { sharpness: Some(220.0), saturation: Some(100.0), ..locked_state() };
    let u = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]).uvc.unwrap();
    assert_eq!(u.sharpness, Some(128.0));
    assert_eq!(u.saturation, Some(100.0));
    let (_, f) = run(&Scene::default(), &no_mod, &st);
    assert!(has(&f, "camera.oversharpened"));
}

#[test]
fn integer_exposure_steps_flag_inexact_shutter() {
    let m = metrics_with_paper(150.0);
    let st = CameraState {
        exposure_log2: Some(-7.0),
        gain: Some(0.0),
        exposure_range: Some(ControlRange::new(-13.0, -1.0, 1.0, -5.0)),
        ..locked_state()
    };
    let u = recommend_settings(&m, &st, CameraClass::FullUvc, MainsHz::Hz50, &[]).uvc.unwrap();
    let v = u.exposure_log2.unwrap();
    assert_eq!(v, v.round());
    assert!(v.exp2() <= 1.0 / 30.0 + 1e-9);
    assert!(!u.shutter_exact);
}

#[test]
fn capture_card_is_coach_only() {
    let m = metrics_with_paper(225.0);
    let st = CameraState::default();
    let r50 = recommend_settings(&m, &st, CameraClass::CaptureCard, MainsHz::Hz50, &[]);
    assert!(r50.uvc.is_none());
    assert!(r50.coach_steps.iter().any(|s| s.contains("1/50")));
    assert!(r50.coach_steps.iter().any(|s| s.contains("HDMI info display")));
    assert!(r50.coach_steps.iter().any(|s| s.contains("dummy battery")));
    let r60 = recommend_settings(&m, &st, CameraClass::CaptureCard, MainsHz::Hz60, &[]);
    assert!(r60.coach_steps.iter().any(|s| s.contains("1/60")));
    // Measured problems still show up, as camera-menu (manual) fixes; no UVC state findings.
    let sc = Scene { paper: [253.0, 253.0, 253.0], ..Scene::default() };
    let (_, f) = analyze(&capture(&sc, &no_mod), &st, CameraClass::CaptureCard, MainsHz::Hz50);
    let x = f.iter().find(|x| x.id == "camera.overexposed").expect("overexposed");
    assert_eq!(x.fix_kind, FixKind::Manual);
    assert!(x.fix.contains("camera menu"));
    assert!(!has(&f, "camera.auto_exposure_on"));
}

#[test]
fn settings_drift_detects_reset() {
    let m = metrics_with_paper(225.0);
    let applied = recommend_settings(&m, &locked_state(), CameraClass::FullUvc, MainsHz::Hz50, &[]).uvc.unwrap();
    let mut live = locked_state();
    live.exposure_log2 = applied.exposure_log2;
    live.gain = applied.gain;
    live.wb_kelvin = applied.wb_kelvin;
    live.focus = applied.focus;
    assert!(settings_drift(&live, &applied).is_none());
    live.auto_exposure = true;
    live.powerline_hz = Some(60);
    let f = settings_drift(&live, &applied).expect("drift");
    assert_eq!(f.id, "camera.settings_reset");
    assert!(f.why.contains("auto_exposure") && f.why.contains("anti_flicker"));
}
