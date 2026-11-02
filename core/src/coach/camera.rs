//! Live camera coaching: feed frames (with the face / paper boxes the app
//! found), get guidance plus overlay arrows for the preview.

use super::tracker::{Step, Tracker};
use super::{round1, CoachTarget, CoachTaskKind, CoachUpdate, Hint, HintKind, Phase};
use crate::video::image::{dominant_periodic, lin_luminance, region_stats};
use crate::video::{self, CameraClass, CameraState, ColourCast, MainsHz, Rect, TaggedFrame, TestStep, VideoMetrics, VideoTestCapture};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// An arrow to draw over the live preview, in 0..1 frame coordinates
/// (origin top-left, as the preview is shown).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arrow {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

pub struct LiveCameraCoach {
    task: CoachTaskKind,
    state: CameraState,
    class: CameraClass,
    mains: MainsHz,
    window: VecDeque<TaggedFrame>,
    window_len: usize,
    tracker: Tracker,
    t0: Option<f64>,
    last_hint: Hint,
    last_face: Option<Rect>,
    size: (usize, usize),
    /// Per-frame measurements (single-frame `measure`), newest last.
    per_frame: VecDeque<VideoMetrics>,
    /// Multi-frame measurement for flicker, refreshed every few frames.
    flicker_cache: Option<VideoMetrics>,
    frames_seen: usize,
    /// Row-brightness profile of the previous frame (rolling-band check).
    last_profile: Option<Vec<f64>>,
    /// Per-frame "moving bands seen" flags, newest last.
    band_hits: VecDeque<(bool, f64)>,
    face_ok: bool,
}

impl LiveCameraCoach {
    pub fn new(task: CoachTaskKind, state: &CameraState, class: CameraClass, mains: MainsHz) -> Self {
        let (window_len, hold) = match task {
            CoachTaskKind::Flicker => (12, 3.0),
            CoachTaskKind::WhiteBalance => (3, 1.5),
            _ => (4, 2.0),
        };
        LiveCameraCoach {
            task,
            state: state.clone(),
            class,
            mains,
            window: VecDeque::new(),
            window_len,
            tracker: Tracker::new(hold),
            t0: None,
            last_hint: Hint::none(),
            last_face: None,
            size: (1, 1),
            per_frame: VecDeque::new(),
            flicker_cache: None,
            frames_seen: 0,
            last_profile: None,
            band_hits: VecDeque::new(),
            face_ok: false,
        }
    }

    pub fn with_stuck_after(mut self, s: f64) -> Self {
        self.tracker.stuck_after_s = s;
        self
    }

    pub fn task(&self) -> CoachTaskKind {
        self.task
    }

    pub fn is_done(&self) -> bool {
        self.tracker.is_done()
    }

    /// Face exposure is OK right now (brighter than the background, no
    /// blown-out skin) — the UI turns the face box green.
    pub fn face_ok(&self) -> bool {
        self.face_ok
    }

    /// The camera settings changed (we applied new values): drop old frames.
    pub fn settings_changed(&mut self, state: &CameraState) {
        self.state = state.clone();
        self.window.clear();
        self.per_frame.clear();
        self.flicker_cache = None;
        self.last_profile = None;
        self.band_hits.clear();
        self.tracker.reset_hold();
    }

    pub fn push(&mut self, frame: &TaggedFrame) -> CoachUpdate {
        let t0 = *self.t0.get_or_insert(frame.frame.timestamp_s);
        let t = (frame.frame.timestamp_s - t0).max(0.0);
        self.size = (frame.frame.width.max(1), frame.frame.height.max(1));
        let mut tf = frame.clone();
        tf.step = TestStep::Neutral;
        self.window.push_back(tf);
        while self.window.len() > self.window_len {
            self.window.pop_front();
        }
        if frame.face.is_some() {
            self.last_face = frame.face;
        }
        let fps = if self.state.requested_fps > 0.0 { self.state.requested_fps } else { 30.0 };
        self.frames_seen += 1;
        if self.task == CoachTaskKind::Flicker {
            self.track_bands(frame);
        }
        let m = if self.task == CoachTaskKind::Flicker {
            // Flicker needs a frame series; re-measure every 4th frame.
            if self.flicker_cache.is_none() || self.frames_seen.is_multiple_of(4) {
                let capture = VideoTestCapture { frames: self.window.iter().cloned().collect() };
                self.flicker_cache = Some(video::measure(&capture, fps));
            }
            self.flicker_cache.clone().unwrap_or_default()
        } else {
            let mut single = frame.clone();
            single.step = TestStep::Neutral;
            self.per_frame.push_back(video::measure(&VideoTestCapture { frames: vec![single] }, fps));
            while self.per_frame.len() > self.window_len {
                self.per_frame.pop_front();
            }
            self.smoothed()
        };
        let mut u = match self.task {
            CoachTaskKind::LightFace => self.eval_light_face(t, &m),
            CoachTaskKind::LightDirection => self.eval_direction(t),
            CoachTaskKind::WhiteBalance => self.eval_wb(t, &m),
            CoachTaskKind::CameraHeight => self.eval_height(t, &m),
            CoachTaskKind::Framing => self.eval_framing(t, &m),
            CoachTaskKind::Flicker => self.eval_flicker(t, &m),
            other => CoachUpdate::simple(other, Phase::Waiting, "This step isn't a camera step", ""),
        };
        self.face_ok = m.face_found
            && m.face_vs_background_stops.is_some_and(|v| v >= 0.5)
            && m.face_clipped_pct.unwrap_or(0.0) < 2.0;
        if let Some(v) = m.face_vs_background_stops {
            u.readings.insert("face_vs_background_stops".into(), round1(v));
        }
        if let Some(v) = m.face_mean_luma {
            u.readings.insert("face_luma".into(), round1(v));
        }
        if let Some(v) = m.paper_cast_pct {
            u.readings.insert("paper_cast_pct".into(), round1(v));
        }
        u.t_s = round1(t);
        if u.phase == Phase::Stuck {
            u.stuck_alternative = Some(self.stuck_text().to_string());
        }
        self.last_hint = u.hint.clone();
        u
    }

    /// Rolling-shutter bands move from frame to frame while the scene stays
    /// put, so look for a periodic pattern in the *difference* of consecutive
    /// row-brightness profiles (static bright objects cancel out).
    fn track_bands(&mut self, tf: &TaggedFrame) {
        let f = &tf.frame;
        let cols = tf.face.map(|r| r.expand(0.3, 0.0, f.width, f.height));
        let mut prof = Vec::with_capacity(f.height);
        for y in 0..f.height {
            let (mut s, mut n) = (0.0, 0.0);
            for x in (0..f.width).step_by(2) {
                if cols.is_some_and(|c| x >= c.x && x < c.right()) {
                    continue;
                }
                s += lin_luminance(f.px(x, y));
                n += 1.0;
            }
            prof.push(if n > 0.0 { s / n } else { 0.0 });
        }
        let mean = prof.iter().sum::<f64>() / prof.len().max(1) as f64;
        if mean > 1e-6 {
            prof.iter_mut().for_each(|v| *v /= mean);
        }
        if let Some(prev) = self.last_profile.as_ref().filter(|p| p.len() == prof.len()) {
            let diff: Vec<f64> = prof.iter().zip(prev).map(|(a, b)| 1.0 + (a - b)).collect();
            let hit = dominant_periodic(&diff, 2).filter(|p| p.rel_amplitude > 0.03 && p.concentration > 0.3);
            self.band_hits.push_back((hit.is_some(), hit.map(|p| p.rel_amplitude * 50.0).unwrap_or(0.0)));
            while self.band_hits.len() > 8 {
                self.band_hits.pop_front();
            }
        }
        self.last_profile = Some(prof);
    }

    /// (detected, amplitude %) from moving bands or whole-frame pulsing.
    fn flicker_now(&self, m: &VideoMetrics) -> (bool, f64) {
        let hits = self.band_hits.iter().filter(|h| h.0).count();
        let bands = self.band_hits.len() >= 4 && hits * 2 >= self.band_hits.len();
        let amp = self.band_hits.iter().map(|h| h.1).fold(0.0, f64::max);
        let temporal = m.flicker.temporal;
        (bands || temporal, if bands { amp } else if temporal { m.flicker.amplitude_pct } else { 0.0 })
    }

    /// Latest single-frame metrics with the key readings averaged over the
    /// last few frames (steadier guidance, no flicker in the UI).
    fn smoothed(&self) -> VideoMetrics {
        let mut m = self.per_frame.back().cloned().unwrap_or_default();
        macro_rules! avg {
            ($f:ident) => {{
                let v: Vec<f64> = self.per_frame.iter().filter_map(|x| x.$f).collect();
                m.$f = if v.is_empty() { None } else { Some(v.iter().sum::<f64>() / v.len() as f64) };
            }};
        }
        avg!(face_vs_background_stops);
        avg!(face_clipped_pct);
        avg!(background_mean_luma);
        avg!(background_vs_paper_stops);
        avg!(paper_cast_pct);
        avg!(face_center_x);
        avg!(face_height_frac);
        avg!(eye_line_y);
        avg!(face_pitch_deg);
        m
    }

    fn mk(&self, step: Step, value: Option<f64>, target: Option<CoachTarget>, headline: &str, detail: &str, hint: Hint) -> CoachUpdate {
        CoachUpdate {
            task: self.task,
            phase: step.phase,
            progress: (step.progress * 100.0).round() / 100.0,
            value: value.map(|v| (v * 100.0).round() / 100.0),
            target,
            headline: headline.to_string(),
            detail: detail.to_string(),
            hint,
            hold_s: round1(step.hold_s),
            hold_needed_s: self.tracker.hold_needed_s,
            readings: BTreeMap::new(),
            auto_adjusting: false,
            stuck_alternative: None,
            t_s: 0.0,
        }
    }

    fn no_face(&mut self, t: f64) -> CoachUpdate {
        let step = self.tracker.waiting(t);
        self.mk(step, None, None, "Sit where you normally stream", "Face the camera so we can see you — we'll guide you from there.", Hint::new(HintKind::Wait, 0.3, "Sit in frame"))
    }

    fn stuck_text(&self) -> &'static str {
        match self.task {
            CoachTaskKind::LightFace => "No extra light? Sit facing a window, or turn a desk lamp toward a white wall in front of you. If nothing can change, tap \"I can't do this\" and we'll expose for your face.",
            CoachTaskKind::LightDirection => "Try moving the light so it's in front of you and a little to one side, slightly above your eyes — or bounce it off a white wall.",
            CoachTaskKind::WhiteBalance => "Mixed light (window + warm lamp + RGB) can't all look white at once. Switch off one kind, or tap \"I can't do this\" and we'll balance for the light on your face.",
            CoachTaskKind::CameraHeight => "Stack a few books under the camera or laptop so the lens is level with your eyes.",
            CoachTaskKind::Framing => "Sit about an arm's length from the camera with your head and shoulders in the picture.",
            CoachTaskKind::Flicker => "Some cheap LED bulbs and dimmers flicker no matter what. Set dimmable lights to 100 % or swap the bulb, or tap \"I can't do this\".",
            _ => "Tap \"I can't do this\" and we'll work around it.",
        }
    }

    fn eval_light_face(&mut self, t: f64, m: &VideoMetrics) -> CoachUpdate {
        let target = CoachTarget::new(0.5, 3.0, "stops brighter than background");
        let Some(fvb) = m.face_vs_background_stops else { return self.no_face(t) };
        let clip = m.face_clipped_pct.unwrap_or(0.0);
        let in_zone = fvb >= 0.5 && clip < 2.0;
        let raw = 0.78 * ((fvb + 1.5) / 2.0).clamp(0.0, 1.0);
        let step = self.tracker.measure(t, in_zone, if clip >= 2.0 { 0.5 } else { raw });
        let bright_bg = m.background_vs_paper_stops.is_some_and(|b| b > 0.0) || m.background_mean_luma.is_some_and(|b| b > 200.0);
        let (h, d, hint) = if step.phase == Phase::Done {
            ("You're lit!", "Your face stands out from the background.", Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("That's it — hold still", "Your face is brighter than the background now.", Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if clip >= 2.0 {
            ("Your face is too bright", "Move the light a little farther away or turn it down until the shine on your forehead goes.", Hint::new(HintKind::Farther, (clip / 10.0).min(1.0), "Light farther away"))
        } else if bright_bg {
            ("The background is brighter than you", "Close the blinds or turn off the light behind you — watch your face pop.", Hint::new(HintKind::DimBackground, ((0.5 - fvb) / 2.0).clamp(0.2, 1.0), "Darken the background"))
        } else {
            ("Add light in front of you", "Point a lamp or light at your face from in front, slightly above your eyes. A window in front of you works too.", Hint::new(HintKind::AddLight, ((0.5 - fvb) / 2.0).clamp(0.2, 1.0), "Light on your face"))
        };
        let mut u = self.mk(step, Some(fvb), Some(target), h, d, hint);
        u.readings.insert("face_clipped_pct".into(), round1(clip));
        u
    }

    /// Image-left / image-right cheek brightness averaged over the window.
    fn cheeks(&self) -> Option<(f64, f64)> {
        let (mut l, mut r, mut n) = (0.0, 0.0, 0);
        for tf in &self.window {
            let Some(face) = tf.face.map(|f| f.clamp_to(tf.frame.width, tf.frame.height)) else { continue };
            if face.w < 8 || face.h < 8 {
                continue;
            }
            let lc = region_stats(&tf.frame, face.frac(0.15, 0.50, 0.40, 0.72), |_, _| true);
            let rc = region_stats(&tf.frame, face.frac(0.60, 0.50, 0.85, 0.72), |_, _| true);
            if lc.n > 0 && rc.n > 0 {
                l += lc.mean_lin;
                r += rc.mean_lin;
                n += 1;
            }
        }
        (n > 0).then(|| (l / n as f64, r / n as f64))
    }

    fn eval_direction(&mut self, t: f64) -> CoachUpdate {
        let target = CoachTarget::new(1.0, 3.0, "bright side ÷ dark side");
        let Some((l, r)) = self.cheeks() else { return self.no_face(t) };
        let ratio = l.max(r) / l.min(r).max(1e-4);
        let limit = if self.tracker.hold_s() > 0.0 { 3.3 } else { 3.0 };
        let in_zone = ratio <= limit;
        let raw = 0.78 * (1.0 - ((ratio - 3.0) / 5.0).clamp(0.0, 1.0));
        let step = self.tracker.measure(t, in_zone, raw);
        let left_darker = l < r;
        let (h, d, hint) = if step.phase == Phase::Done {
            ("Even, flattering light", "Both sides of your face are lit nicely.", Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("Nice and even — hold it", "Keep the light there while the ring fills.", Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if left_darker {
            ("One side of your face is in shadow", "Move your light toward the darker side — the left side of the preview — or add a white wall/board there to bounce light back.", Hint::new(HintKind::MoveLightLeft, ((ratio - 3.0) / 5.0).clamp(0.2, 1.0), "Light toward the left of the preview"))
        } else {
            ("One side of your face is in shadow", "Move your light toward the darker side — the right side of the preview — or add a white wall/board there to bounce light back.", Hint::new(HintKind::MoveLightRight, ((ratio - 3.0) / 5.0).clamp(0.2, 1.0), "Light toward the right of the preview"))
        };
        let mut u = self.mk(step, Some(ratio), Some(target), h, d, hint);
        u.readings.insert("left_cheek_lin".into(), (l * 1000.0).round() / 1000.0);
        u.readings.insert("right_cheek_lin".into(), (r * 1000.0).round() / 1000.0);
        u
    }

    fn eval_wb(&mut self, t: f64, m: &VideoMetrics) -> CoachUpdate {
        let target = CoachTarget::new(0.0, 4.0, "% colour cast");
        let (Some(cast), true) = (m.paper_cast_pct, m.paper_found) else {
            let step = self.tracker.waiting(t);
            return self.mk(step, None, Some(target), "Hold the white sheet inside the box", "Fill the dotted box on the preview with the paper, facing the camera.", Hint::new(HintKind::Wait, 0.4, "Paper in the box"));
        };
        let in_zone = cast <= target.max;
        let raw = 0.78 * (1.0 - ((cast - 4.0) / 16.0).clamp(0.0, 1.0));
        let step = self.tracker.measure(t, in_zone, raw);
        let auto = self.class.controllable();
        let (h, d) = match (step.phase, m.paper_cast) {
            (Phase::Done, _) => ("White looks white!".to_string(), "Your colours are true now.".to_string()),
            (Phase::Hold, _) => ("White looks white — hold the paper still".to_string(), "Keep it in the box for a moment.".to_string()),
            (_, Some(ColourCast::Warm)) => ("The paper looks orange".to_string(), if auto { "Warm lamps are mixing in. We'll fine-tune the colour for you — switching off warm bulbs near you helps most." } else { "Warm lamps are mixing in: switch off warm bulbs or set your camera's white balance warmer." }.to_string()),
            (_, Some(ColourCast::Cool)) => ("The paper looks blue".to_string(), if auto { "Daylight or a cool screen is mixing in. We'll fine-tune the colour; closing blinds or dimming the monitor helps." } else { "Daylight or a cool screen is mixing in: close blinds or set your camera's white balance cooler." }.to_string()),
            (_, Some(ColourCast::Green)) | (_, Some(ColourCast::Magenta)) => ("The paper has a colour tint".to_string(), "Coloured (RGB) lights are hitting you. Point them at the wall instead of your face.".to_string()),
            _ => ("Checking colours…".to_string(), "Hold the paper still in the box.".to_string()),
        };
        let mut u = self.mk(step, Some(cast), Some(target), &h, &d, Hint::new(HintKind::Wait, (cast / 20.0).min(1.0), "Hold the paper still"));
        u.auto_adjusting = auto && step.phase != Phase::Done && !in_zone;
        u
    }

    fn eval_height(&mut self, t: f64, m: &VideoMetrics) -> CoachUpdate {
        if !m.face_found {
            return self.no_face(t);
        }
        if let Some(p) = m.face_pitch_deg {
            let target = CoachTarget::new(-8.0, 8.0, "degrees");
            let in_zone = target.contains(p);
            let raw = 0.78 * (1.0 - (target.signed_miss(p).abs() / 20.0).min(1.0));
            let step = self.tracker.measure(t, in_zone, raw);
            let (h, d, hint) = if step.phase == Phase::Done {
                ("Eye level — perfect", "Viewers see you straight on.", Hint::new(HintKind::Stop, 0.0, "Done"))
            } else if in_zone {
                ("That's eye level — hold it", "", Hint::new(HintKind::Stop, 0.0, "Hold"))
            } else if p > 8.0 {
                ("The camera looks up at you", "Raise the camera until the lens is level with your eyes — a few books under it works.", Hint::new(HintKind::RaiseCamera, ((p - 8.0) / 20.0).clamp(0.2, 1.0), "Raise the camera"))
            } else {
                ("The camera looks down at you", "Lower the camera a little so the lens is level with your eyes.", Hint::new(HintKind::LowerCamera, ((-8.0 - p) / 20.0).clamp(0.2, 1.0), "Lower the camera"))
            };
            return self.mk(step, Some(p), Some(target), h, d, hint);
        }
        let Some(y) = m.eye_line_y else { return self.no_face(t) };
        let target = CoachTarget::new(0.28, 0.42, "eye line from top");
        let in_zone = target.contains(y);
        let raw = 0.78 * (1.0 - (target.signed_miss(y).abs() / 0.25).min(1.0));
        let step = self.tracker.measure(t, in_zone, raw);
        let (h, d, hint) = if step.phase == Phase::Done {
            ("Eyes in the right spot", "Your eyes sit about a third from the top — classic framing.", Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("Good height — hold it", "", Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if y > 0.42 {
            ("Your eyes sit low in the picture", "Aim the camera a little lower (or raise it) so your eyes are about a third from the top.", Hint::new(HintKind::LowerCamera, ((y - 0.42) / 0.2).clamp(0.2, 1.0), "Aim lower"))
        } else {
            ("Your head is close to the top", "Aim the camera a little higher so there's a little space above your head.", Hint::new(HintKind::RaiseCamera, ((0.28 - y) / 0.2).clamp(0.2, 1.0), "Aim higher"))
        };
        self.mk(step, Some(y), Some(target), h, d, hint)
    }

    fn eval_framing(&mut self, t: f64, m: &VideoMetrics) -> CoachUpdate {
        let (Some(cx), Some(fh)) = (m.face_center_x, m.face_height_frac) else { return self.no_face(t) };
        let eye = m.eye_line_y.unwrap_or(0.35);
        let cxt = CoachTarget::new(0.4, 0.6, "");
        let fht = CoachTarget::new(0.25, 0.5, "");
        let eyt = CoachTarget::new(0.28, 0.42, "");
        let misses = [cxt.signed_miss(cx) / 0.1, fht.signed_miss(fh) / 0.1, eyt.signed_miss(eye) / 0.07];
        let worst = misses.iter().cloned().fold(0.0f64, |a, b| if b.abs() > a.abs() { b } else { a });
        let score = worst.abs();
        let in_zone = score == 0.0;
        let raw = 0.78 * (1.0 - (score / 3.0).min(1.0));
        let step = self.tracker.measure(t, in_zone, raw);
        let amount = (score / 3.0).clamp(0.2, 1.0);
        let which = misses.iter().position(|v| *v == worst).unwrap_or(0);
        let (h, d, hint) = if step.phase == Phase::Done {
            ("Framed like a pro", "Head and shoulders, centred, eyes a third from the top.", Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("Great framing — hold it", "", Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else {
            match (which, worst > 0.0) {
                (0, true) => ("Move a little to the left of the screen", "Centre yourself in the picture.", Hint::new(HintKind::MoveLeft, amount, "Move left")),
                (0, false) => ("Move a little to the right of the screen", "Centre yourself in the picture.", Hint::new(HintKind::MoveRight, amount, "Move right")),
                (1, true) => ("You're very close to the camera", "Sit back a little — head and shoulders should fit, and wide lenses distort up close.", Hint::new(HintKind::MoveBack, amount, "Sit back")),
                (1, false) => ("You're far from the camera", "Move closer, or move the camera closer, so your head and shoulders fill the frame.", Hint::new(HintKind::MoveForward, amount, "Come closer")),
                (_, true) => ("Your eyes sit low in the picture", "Aim the camera a little lower so your eyes are about a third from the top.", Hint::new(HintKind::LowerCamera, amount, "Aim lower")),
                (_, false) => ("Your head is close to the top", "Aim the camera a little higher.", Hint::new(HintKind::RaiseCamera, amount, "Aim higher")),
            }
        };
        let mut u = self.mk(step, Some(score), Some(CoachTarget::new(0.0, 0.0, "off target")), h, d, hint);
        u.readings.insert("face_center_x".into(), (cx * 100.0).round() / 100.0);
        u.readings.insert("face_height_frac".into(), (fh * 100.0).round() / 100.0);
        u.readings.insert("eye_line_y".into(), (eye * 100.0).round() / 100.0);
        u
    }

    fn eval_flicker(&mut self, t: f64, m: &VideoMetrics) -> CoachUpdate {
        let target = CoachTarget::new(0.0, 0.0, "% flicker");
        if self.window.len() < 8 {
            let step = self.tracker.waiting(t);
            return self.mk(step, None, Some(target), "Checking for flicker…", "Sit still for a few seconds.", Hint::new(HintKind::Wait, 0.3, "Sit still"));
        }
        let (detected, amp) = self.flicker_now(m);
        let amp = if detected { amp.max(0.1) } else { 0.0 };
        let in_zone = !detected;
        let step = self.tracker.measure(t, in_zone, 0.5);
        let other = if self.mains == MainsHz::Hz50 { 60 } else { 50 };
        let (h, d) = if step.phase == Phase::Done {
            ("No more flicker".to_string(), "The light bands are gone.".to_string())
        } else if in_zone {
            ("Looking steady — hold still".to_string(), "Making sure the bands are gone.".to_string())
        } else {
            (
                "We still see light bands".to_string(),
                format!("If a light is on a dimmer, set it to 100 %. Still there? Try the {other} Hz anti-flicker setting — some lights are odd."),
            )
        };
        self.mk(step, Some(amp), Some(target), &h, &d, Hint::new(HintKind::Wait, (amp / 10.0).min(1.0), "Hold still"))
    }

    /// Arrows for the current hint, in 0..1 preview coordinates.
    pub fn overlay_arrows(&self) -> Vec<Arrow> {
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let (fx, fy, fw, fh) = match self.last_face {
            Some(f) => (f.x as f64 / w, f.y as f64 / h, f.w as f64 / w, f.h as f64 / h),
            None => (0.4, 0.25, 0.2, 0.4),
        };
        let (cx, cy) = (fx + fw / 2.0, fy + fh / 2.0);
        let a = 0.08 + 0.12 * self.last_hint.amount;
        match self.last_hint.kind {
            HintKind::MoveLightLeft => vec![Arrow { x: fx - 0.02, y: cy, dx: -a, dy: 0.0 }],
            HintKind::MoveLightRight => vec![Arrow { x: fx + fw + 0.02, y: cy, dx: a, dy: 0.0 }],
            HintKind::MoveLightUp => vec![Arrow { x: cx, y: fy - 0.02, dx: 0.0, dy: -a }],
            HintKind::AddLight => vec![Arrow { x: cx, y: (fy - 0.18).max(0.02), dx: 0.0, dy: a }],
            HintKind::Farther => vec![Arrow { x: cx, y: (fy - 0.05).max(0.02), dx: 0.0, dy: -a }],
            HintKind::DimBackground => vec![Arrow { x: 0.12, y: 0.15, dx: -0.08, dy: -0.08 }, Arrow { x: 0.88, y: 0.15, dx: 0.08, dy: -0.08 }],
            HintKind::RaiseCamera => vec![Arrow { x: 0.5, y: 0.9, dx: 0.0, dy: -a }],
            HintKind::LowerCamera => vec![Arrow { x: 0.5, y: 0.1, dx: 0.0, dy: a }],
            HintKind::MoveLeft => vec![Arrow { x: cx, y: cy, dx: -a, dy: 0.0 }],
            HintKind::MoveRight => vec![Arrow { x: cx, y: cy, dx: a, dy: 0.0 }],
            HintKind::MoveBack => vec![Arrow { x: fx, y: cy, dx: a / 2.0, dy: 0.0 }, Arrow { x: fx + fw, y: cy, dx: -a / 2.0, dy: 0.0 }],
            HintKind::MoveForward => vec![Arrow { x: fx, y: cy, dx: -a / 2.0, dy: 0.0 }, Arrow { x: fx + fw, y: cy, dx: a / 2.0, dy: 0.0 }],
            _ => Vec::new(),
        }
    }
}
