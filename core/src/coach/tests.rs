use super::*;
use crate::audio::dsp::{Biquad, Rng, from_db};
use crate::audio::findings::MicContext;
use crate::audio::synth::{self, Scenario};
use crate::audio::SegmentKind;
use crate::video::{CameraClass, CameraState, Frame, MainsHz, Rect, TaggedFrame, TestStep};

const SR: u32 = 48_000;

/// Speech-like source (the synthetic read), normalised so its talking peaks
/// sit near 0 dBFS; scale with `from_db(peak_db)`.
fn speech(secs: f64) -> Vec<f32> {
    static SRC: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    let src = SRC.get_or_init(|| {
        let t = synth::build(&Scenario { speech_level_dbfs: -26.0, ..Default::default() });
        let seg = t.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
        let src: Vec<f32> = t.audio.channels[0][t.range(seg)].to_vec();
        let peak = src.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        src.iter().map(|v| v / peak * 0.5).collect() // ~ -6 dBFS peaks
    });
    let n = (secs * SR as f64) as usize;
    (0..n).map(|i| src[i % src.len()]).collect()
}

fn scaled(x: &[f32], db: f64) -> Vec<f32> {
    let g = from_db(db) as f32;
    x.iter().map(|v| v * g).collect()
}

fn noise(secs: f64, level_dbfs: f64, seed: u64, lowpass: Option<f64>) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let n = (secs * SR as f64) as usize;
    let mut f = lowpass.map(|hz| Biquad::lowpass(SR, hz, 0.707));
    let mut v: Vec<f64> = (0..n).map(|_| {
        let x = rng.bipolar();
        match f.as_mut() { Some(f) => f.process(x), None => x }
    }).collect();
    let rms = (v.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt().max(1e-12);
    let k = from_db(level_dbfs) / rms;
    v.iter_mut().for_each(|x| *x *= k);
    v.into_iter().map(|x| x as f32).collect()
}

fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// Feed in 20 ms chunks, collecting every update.
fn feed(c: &mut LiveMicCoach, x: &[f32]) -> Vec<CoachUpdate> {
    x.chunks((SR / 50) as usize).filter_map(|ch| c.push(ch)).collect()
}

fn last(u: &[CoachUpdate]) -> &CoachUpdate {
    u.last().expect("updates")
}

#[test]
fn set_gain_points_up_when_quiet_and_down_when_hot() {
    let ctx = MicContext::default();
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &ctx, None);
    let u = feed(&mut c, &scaled(&speech(3.0), -36.0));
    let l = last(&u);
    assert_eq!(l.hint.kind, HintKind::TurnUp, "{l:?}");
    assert!(l.value.unwrap() < -18.0);
    assert!(l.headline.contains("Turn the gain up"));
    assert!(matches!(l.phase, Phase::Working | Phase::Almost));

    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &ctx, None);
    let u = feed(&mut c, &scaled(&speech(3.0), -1.0));
    assert_eq!(last(&u).hint.kind, HintKind::TurnDown, "{:?}", last(&u));
}

#[test]
fn set_gain_holds_then_finishes() {
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &MicContext::default(), None);
    let u = feed(&mut c, &scaled(&speech(8.0), -9.0));
    assert!(u.iter().any(|x| x.phase == Phase::Hold), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
    let done = u.iter().position(|x| x.phase == Phase::Done).expect("done");
    let first_hold = u.iter().position(|x| x.phase == Phase::Hold).unwrap();
    assert!(first_hold < done);
    assert!(u[done].t_s - u[first_hold].t_s >= 2.8);
    assert_eq!(u[done].progress, 1.0);
    assert!(c.is_done());
}

#[test]
fn set_gain_clipping_is_urgent() {
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &MicContext::default(), None);
    let hot: Vec<f32> = speech(2.0).iter().map(|v| (v * 4.0).clamp(-1.0, 1.0)).collect();
    let u = feed(&mut c, &hot);
    assert_eq!(last(&u).hint.kind, HintKind::TurnDown);
    assert!(last(&u).headline.contains("Too loud"));
}

#[test]
fn silence_waits_then_gets_stuck_with_an_alternative() {
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &MicContext::default(), None).with_stuck_after(10.0);
    let u = feed(&mut c, &noise(12.0, -75.0, 1, None));
    assert_eq!(u[5].phase, Phase::Waiting);
    assert_eq!(u[5].hint.kind, HintKind::KeepTalking);
    let l = last(&u);
    assert_eq!(l.phase, Phase::Stuck);
    assert!(l.stuck_alternative.as_deref().unwrap().contains("closer"));
}

#[test]
fn auto_gain_converges_without_overshoot() {
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &MicContext::default(), None).with_auto_gain(true);
    let src = speech(60.0);
    let chunk = (SR / 10) as usize;
    let mut gain = 0.0f64; // software gain
    let offset = -40.0; // at gain 0 the voice peaks around -46 dBFS
    let mut gains = vec![gain];
    let mut values = Vec::new();
    for ch in src.chunks(chunk) {
        if let Some(u) = feed(&mut c, &scaled(ch, offset + gain)).last() {
            assert!(u.auto_adjusting || matches!(u.phase, Phase::Hold | Phase::Done | Phase::Waiting));
            if let Some(v) = u.value {
                values.push(v);
            }
        }
        if let Some(next) = c.auto_gain_step(gain, -10.0, 60.0) {
            assert!(next > gain, "approaching from below must only go up: {gains:?} -> {next}");
            gain = next;
            gains.push(gain);
        }
        if c.is_done() {
            break;
        }
    }
    assert!(c.is_done(), "gains {gains:?} values {values:?}");
    assert!(gains.len() <= 6, "too many steps: {gains:?}");
    assert!(values.iter().all(|v| *v <= -10.0), "overshoot: {values:?}");
}

#[test]
fn auto_gain_reports_maxed_gain() {
    let mut c = LiveMicCoach::new(CoachTaskKind::SetGain, SR, &MicContext::default(), None).with_auto_gain(true);
    let src = scaled(&speech(5.0), -40.0);
    let mut updates = Vec::new();
    for ch in src.chunks((SR / 10) as usize) {
        updates.extend(feed(&mut c, ch));
        assert!(c.auto_gain_step(20.0, 0.0, 20.0).is_none(), "gain is already at max");
    }
    let l = last(&updates);
    assert_eq!(l.hint.kind, HintKind::Closer, "{l:?}");
    assert!(l.headline.contains("maximum"));
}

#[test]
fn distance_closer_then_hold() {
    let mut c = LiveMicCoach::new(CoachTaskKind::FindDistance, SR, &MicContext::default(), None).with_goal(DistanceGoal::Closer);
    let s = speech(14.0);
    let (a, b) = s.split_at(6 * SR as usize);
    let u = feed(&mut c, &scaled(a, -20.0));
    assert!(u.iter().take(5).all(|x| x.phase == Phase::Waiting));
    assert_eq!(last(&u).hint.kind, HintKind::Closer, "{:?}", last(&u));
    // Closer: louder and bassier (proximity effect).
    let mut shelf = Biquad::low_shelf(SR, 200.0, 4.0);
    let mut b: Vec<f32> = scaled(b, -12.0);
    shelf.run(&mut b);
    let u = feed(&mut c, &b);
    assert!(u.iter().any(|x| x.phase == Phase::Done), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn distance_too_close_says_back_off() {
    let mut c = LiveMicCoach::new(CoachTaskKind::FindDistance, SR, &MicContext::default(), None).with_goal(DistanceGoal::Confirm);
    let s = speech(8.0);
    let (a, b) = s.split_at(4 * SR as usize);
    feed(&mut c, &scaled(a, -20.0));
    let u = feed(&mut c, &scaled(b, -10.0));
    assert_eq!(last(&u).hint.kind, HintKind::Farther, "{:?}", last(&u));
}

/// "pop" words: 400 ms pause, then a 160 ms voiced syllable, optionally with
/// a low-frequency burst at its onset.
fn pops(n: usize, popped: bool, seed: u64) -> Vec<f32> {
    let mut out = Vec::new();
    let mut rng = Rng::new(seed);
    for _ in 0..n {
        out.extend(noise(0.4, -80.0, rng.next_u64(), None));
        let len = (0.16 * SR as f64) as usize;
        for i in 0..len {
            let t = i as f64 / SR as f64;
            let env = (t / 0.01).min(1.0) * (-(t - 0.12).max(0.0) / 0.01).exp();
            let mut v = 0.0;
            for k in 1..12 {
                v += (2.0 * std::f64::consts::PI * 180.0 * k as f64 * t).sin() / k as f64;
            }
            v *= 0.08 * env;
            if popped && t < 0.04 {
                v += 0.5 * (2.0 * std::f64::consts::PI * 70.0 * t).sin() * (-t / 0.015).exp();
            }
            out.push(v as f32);
        }
    }
    out
}

#[test]
fn pops_need_three_clean_in_a_row() {
    let mut c = LiveMicCoach::new(CoachTaskKind::AngleOffAxis, SR, &MicContext::default(), None);
    let u = feed(&mut c, &noise(0.6, -80.0, 9, None));
    assert_eq!(last(&u).hint.kind, HintKind::SayPops);
    let u = feed(&mut c, &pops(3, true, 1));
    assert!(u.iter().any(|x| x.hint.kind == HintKind::AngleAway), "{:?}", u.iter().map(|x| (&x.headline, x.value)).collect::<Vec<_>>());
    assert!(!c.is_done());
    let u = feed(&mut c, &pops(2, false, 2));
    assert!(u.iter().any(|x| x.headline.starts_with("Clean!")));
    assert!(!c.is_done());
    let u = feed(&mut c, &pops(2, false, 3));
    assert!(c.is_done(), "{:?}", u.iter().map(|x| (&x.headline, x.value)).collect::<Vec<_>>());
}

#[test]
fn quiet_room_rumble_then_quiet() {
    let mut c = LiveMicCoach::new(CoachTaskKind::QuietRoom, SR, &MicContext::default(), None);
    let u = feed(&mut c, &noise(3.0, -45.0, 4, Some(300.0)));
    let l = last(&u);
    assert!(l.headline.contains("rumble"), "{l:?}");
    assert!((l.value.unwrap() + 45.0).abs() < 3.0);
    // Talking in the middle: "be quiet".
    let u = feed(&mut c, &mix(&scaled(&speech(1.0), -10.0), &noise(1.0, -45.0, 5, Some(300.0))));
    assert_eq!(last(&u).hint.kind, HintKind::BeQuiet);
    // Fan off.
    let u = feed(&mut c, &noise(4.0, -72.0, 6, None));
    assert!(u.iter().any(|x| x.phase == Phase::Done), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

fn typing(secs: f64, click_db: f64, seed: u64) -> Vec<f32> {
    let mut out = noise(secs, -78.0, seed, None);
    let mut rng = Rng::new(seed ^ 77);
    let mut t = 0.05;
    let amp = from_db(click_db);
    while t < secs - 0.02 {
        let s0 = (t * SR as f64) as usize;
        for i in 0..(0.004 * SR as f64) as usize {
            let env = (-(i as f64 / SR as f64) / 0.0015).exp();
            out[s0 + i] += (rng.bipolar() * amp * 2.5 * env) as f32;
        }
        t += rng.range(0.1, 0.16);
    }
    out
}

#[test]
fn keyboard_null_tracks_typing_level() {
    let mut c = LiveMicCoach::new(CoachTaskKind::KeyboardNull, SR, &MicContext::default(), None);
    let u = feed(&mut c, &noise(1.0, -78.0, 3, None));
    assert_eq!(last(&u).hint.kind, HintKind::TypeNow);
    let u = feed(&mut c, &typing(2.5, -30.0, 4));
    let l = last(&u);
    assert_eq!(l.hint.kind, HintKind::AngleAway, "{l:?}");
    let u = feed(&mut c, &typing(5.0, -55.0, 5));
    assert!(u.iter().any(|x| x.phase == Phase::Done), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn talk_into_logo_needs_clarity_rise() {
    let mut c = LiveMicCoach::new(CoachTaskKind::TalkIntoLogo, SR, &MicContext::default(), None);
    let s = speech(10.0);
    let (a, b) = s.split_at(5 * SR as usize);
    let mut dull = scaled(a, -20.0);
    for mut f in [Biquad::high_shelf(SR, 2500.0, -12.0)] {
        f.run(&mut dull);
    }
    let u = feed(&mut c, &dull);
    assert_eq!(last(&u).hint.kind, HintKind::FaceLogo, "{:?}", last(&u));
    let u = feed(&mut c, &scaled(b, -20.0));
    assert!(u.iter().any(|x| x.phase == Phase::Done), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn updates_serialize_snake_case() {
    let mut c = LiveMicCoach::new(CoachTaskKind::FindDistance, SR, &MicContext::default(), None);
    let u = feed(&mut c, &scaled(&speech(1.0), -20.0));
    let j = serde_json::to_value(last(&u)).unwrap();
    assert_eq!(j["task"], "find_distance");
    assert_eq!(j["hint"]["kind"], "keep_talking");
    assert_eq!(j["phase"], "waiting");
    let back: CoachUpdate = serde_json::from_value(j).unwrap();
    assert_eq!(back.task, CoachTaskKind::FindDistance);
}

// ---------------------------------------------------------------- camera

const FACE: Rect = Rect { x: 120, y: 50, w: 80, h: 110 };
const PAPER: Rect = Rect { x: 230, y: 90, w: 70, h: 90 };

struct Scene {
    face: f64,
    left_factor: f64,
    bg: f64,
    paper: Option<[f64; 3]>,
    face_rect: Rect,
    /// Rolling-shutter band amplitude (0 = none).
    bands: f64,
}

impl Default for Scene {
    fn default() -> Self {
        Scene { face: 170.0, left_factor: 1.0, bg: 90.0, paper: None, face_rect: FACE, bands: 0.0 }
    }
}

fn frame(s: &Scene, t: f64) -> TaggedFrame {
    let (w, h) = (320usize, 240usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    let r = s.face_rect;
    let fcx = r.x as f64 + r.w as f64 / 2.0;
    for y in 0..h {
        for x in 0..w {
            let px = if let Some(p) = s.paper.filter(|_| PAPER.contains(x, y)) {
                p
            } else if r.in_ellipse(x, y, 1.0) {
                let k = if (x as f64) < fcx { s.left_factor } else { 1.0 };
                [s.face * k, s.face * k * 0.78, s.face * k * 0.66]
            } else {
                [s.bg, s.bg, s.bg * 1.02]
            };
            let n = ((x * 31 + y * 17) % 5) as f64 - 2.0;
            let band = 1.0 + s.bands * (2.0 * std::f64::consts::PI * (y as f64 + t * 37.0) / 40.0).sin();
            for v in px {
                rgb.push((v * band + n).clamp(0.0, 255.0) as u8);
            }
        }
    }
    let mut tf = TaggedFrame::new(TestStep::Neutral, Frame { width: w, height: h, rgb, timestamp_s: t }).with_face(r);
    if s.paper.is_some() {
        tf = tf.with_paper(PAPER);
    }
    tf
}

fn run(c: &mut LiveCameraCoach, s: &Scene, from: f64, secs: f64) -> Vec<CoachUpdate> {
    let n = (secs * 8.0) as usize;
    (0..n).map(|i| c.push(&frame(s, from + i as f64 / 8.0))).collect()
}

fn cam(task: CoachTaskKind) -> LiveCameraCoach {
    LiveCameraCoach::new(task, &CameraState::default(), CameraClass::FullUvc, MainsHz::Hz50)
}

#[test]
fn light_face_wants_light_then_finishes() {
    let mut c = cam(CoachTaskKind::LightFace);
    let u = run(&mut c, &Scene { face: 60.0, bg: 120.0, ..Default::default() }, 0.0, 1.0);
    let l = last(&u);
    assert!(matches!(l.hint.kind, HintKind::AddLight | HintKind::DimBackground), "{l:?}");
    assert!(!c.overlay_arrows().is_empty());
    assert!(!c.face_ok());
    let u = run(&mut c, &Scene { face: 190.0, bg: 80.0, ..Default::default() }, 1.0, 3.0);
    assert!(c.face_ok());
    assert!(u.iter().any(|x| x.phase == Phase::Hold));
    assert!(c.is_done(), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn bright_background_says_darken_it() {
    let mut c = cam(CoachTaskKind::LightFace);
    let u = run(&mut c, &Scene { face: 120.0, bg: 230.0, ..Default::default() }, 0.0, 1.0);
    assert_eq!(last(&u).hint.kind, HintKind::DimBackground, "{:?}", last(&u));
}

#[test]
fn light_direction_points_to_darker_side() {
    let mut c = cam(CoachTaskKind::LightDirection);
    let u = run(&mut c, &Scene { left_factor: 0.35, ..Default::default() }, 0.0, 1.0);
    assert_eq!(last(&u).hint.kind, HintKind::MoveLightLeft, "{:?}", last(&u));
    let arrows = c.overlay_arrows();
    assert!(arrows[0].dx < 0.0);
    let u = run(&mut c, &Scene::default(), 1.0, 3.0);
    assert!(c.is_done(), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn white_balance_needs_paper_and_neutral() {
    let mut c = cam(CoachTaskKind::WhiteBalance);
    let u = run(&mut c, &Scene::default(), 0.0, 0.5);
    assert!(last(&u).headline.contains("white sheet"));
    let u = run(&mut c, &Scene { paper: Some([240.0, 200.0, 150.0]), ..Default::default() }, 0.5, 1.0);
    assert!(last(&u).headline.contains("orange"), "{:?}", last(&u));
    assert!(last(&u).auto_adjusting);
    run(&mut c, &Scene { paper: Some([220.0, 220.0, 220.0]), ..Default::default() }, 1.5, 2.5);
    assert!(c.is_done());
}

#[test]
fn framing_moves_you_to_the_centre() {
    let mut c = cam(CoachTaskKind::Framing);
    let off = Scene { face_rect: Rect { x: 20, y: 50, w: 80, h: 110 }, ..Default::default() };
    let u = run(&mut c, &off, 0.0, 1.0);
    assert_eq!(last(&u).hint.kind, HintKind::MoveRight, "{:?}", last(&u));
    run(&mut c, &Scene::default(), 1.0, 3.0);
    assert!(c.is_done());
}

#[test]
fn camera_gets_stuck_without_progress() {
    let mut c = cam(CoachTaskKind::LightFace);
    let u = run(&mut c, &Scene { face: 60.0, bg: 120.0, ..Default::default() }, 0.0, 50.0);
    let l = last(&u);
    assert_eq!(l.phase, Phase::Stuck);
    assert!(l.stuck_alternative.as_deref().unwrap().contains("window"));
}

#[test]
fn tracker_hold_resets_when_leaving_the_zone() {
    let mut t = Tracker::new(2.0);
    t.measure(0.0, true, 0.0);
    t.measure(1.5, true, 0.0);
    assert!(t.hold_s() >= 1.5);
    t.measure(1.6, false, 0.4);
    assert_eq!(t.hold_s(), 0.0);
    let s = t.measure(1.7, true, 0.0);
    assert_eq!(s.phase, Phase::Hold);
    let s = t.measure(3.8, true, 0.0);
    assert_eq!(s.phase, Phase::Done);
}

#[test]
fn flicker_bands_then_steady() {
    let mut c = cam(CoachTaskKind::Flicker);
    let u = run(&mut c, &Scene { bands: 0.25, ..Default::default() }, 0.0, 3.0);
    let l = last(&u);
    assert!(l.headline.contains("light bands"), "{l:?}");
    assert!(l.detail.contains("60 Hz"));
    let u = run(&mut c, &Scene::default(), 3.0, 6.0);
    assert!(c.is_done(), "{:?}", u.iter().map(|x| (x.phase, x.value)).collect::<Vec<_>>());
}

#[test]
fn static_bright_objects_are_not_flicker() {
    let mut c = cam(CoachTaskKind::Flicker);
    run(&mut c, &Scene { paper: Some([250.0, 250.0, 250.0]), ..Default::default() }, 0.0, 6.0);
    assert!(c.is_done());
}
