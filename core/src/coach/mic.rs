//! Live mic coaching: feed audio chunks, get guidance several times a second.

use super::tracker::{Step, Tracker};
use super::{round1, CoachTarget, CoachTaskKind, CoachUpdate, Hint, HintKind, Phase};
use crate::audio::analyze::AudioMetrics;
use crate::audio::dsp::{Biquad, db_pow, median, percentile};
use crate::audio::findings::{Capsule, MicContext};
use crate::SetupProfile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Which way the distance coach should move the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceGoal {
    Closer,
    Farther,
    /// Just find and hold the sweet spot.
    Confirm,
}

/// Talking-level zone for normal speech peaks (dBFS).
pub const GAIN_ZONE: (f64, f64) = (-18.0, -12.0);
const GAIN_CENTRE: f64 = -15.0;
const FRAME_S: f64 = 0.02;
const UPDATE_EVERY_FRAMES: usize = 5; // 100 ms
const BASELINE_TALK_S: f64 = 2.5;

#[derive(Debug, Clone, Copy)]
struct Fr {
    t: f64,
    rms: f64,
    peak: f64,
    lf: f64,
    mid: f64,
    hi: f64,
    talking: bool,
}

#[derive(Debug, Clone, Copy)]
struct Base {
    level: f64,
    lf_ratio: f64,
    tilt: f64,
    drop: f64,
}

/// Streaming coach for one mic task.
pub struct LiveMicCoach {
    task: CoachTaskKind,
    sr: u32,
    frame_len: usize,
    // streaming filters + frame accumulators
    lf: [Biquad; 2],
    mid: [Biquad; 2],
    hi: [Biquad; 2],
    acc: [f64; 4],
    acc_peak: f64,
    acc_n: usize,
    n_samples: u64,
    hist: VecDeque<Fr>,
    noise: Option<f64>,
    frames_since_update: usize,
    tracker: Tracker,
    // config
    auto: bool,
    speech_rms: f64,
    goal: DistanceGoal,
    distance_text: String,
    /// QuietRoom passes at or under this floor (the profile's noise "met" bar).
    quiet_target_dbfs: f64,
    // gain
    epoch_t: f64,
    at_limit_up: Option<bool>,
    // baseline for distance / logo
    base_talk: Vec<Fr>,
    base: Option<Base>,
    // decay proxy: (rms at offset, frames since)
    offset: Option<(f64, usize)>,
    drops: VecDeque<f64>,
    // pops
    quiet_run: usize,
    attempt: Option<(usize, bool)>,
    streak: usize,
    attempts: usize,
    last_attempt_t: f64,
    last_clean: Option<bool>,
}

impl LiveMicCoach {
    /// `report` is the last mic scan (for the speech level, distance goal…).
    pub fn new(task: CoachTaskKind, sample_rate: u32, ctx: &MicContext, report: Option<&AudioMetrics>) -> Self {
        let sr = sample_rate.max(8000);
        let room_flag = ctx.profile.targets().room_decay.flag;
        let goal = match report {
            Some(m) if m.reverb_decay_s.is_some_and(|r| r > room_flag) || m.speech_peak_dbfs < -22.0 => DistanceGoal::Closer,
            Some(m) if m.tone.boom_db > 5.0 || m.plosive_events > 3 => DistanceGoal::Farther,
            Some(_) => DistanceGoal::Confirm,
            None => DistanceGoal::Closer,
        };
        let distance_text = match (ctx.distance_cm, ctx.capsule) {
            _ if ctx.profile != SetupProfile::Streaming => ctx.profile.distance_text(ctx),
            (Some((a, b)), _) => format!("{a:.0}–{b:.0} cm"),
            (None, Some(Capsule::Dynamic)) => "about a fist away (3–10 cm)".into(),
            (None, Some(_)) => "about a hand-span away (10–20 cm)".into(),
            (None, None) => "about a fist away".into(),
        };
        let hold = match task {
            CoachTaskKind::SetGain | CoachTaskKind::KeyboardNull => 3.0,
            CoachTaskKind::AngleOffAxis => 0.0,
            _ => 2.0,
        };
        let mk2 = |f: &dyn Fn() -> Biquad| [f(), f()];
        LiveMicCoach {
            task,
            sr,
            frame_len: (sr as f64 * FRAME_S).round() as usize,
            lf: mk2(&|| Biquad::lowpass(sr, 150.0, 0.707)),
            mid: [Biquad::highpass(sr, 300.0, 0.707), Biquad::lowpass(sr, 3000.0, 0.707)],
            hi: mk2(&|| Biquad::highpass(sr, 2000.0, 0.707)),
            acc: [0.0; 4],
            acc_peak: 0.0,
            acc_n: 0,
            n_samples: 0,
            hist: VecDeque::new(),
            noise: None,
            frames_since_update: 0,
            tracker: Tracker::new(hold),
            auto: false,
            speech_rms: report.map(|m| m.speech_rms_dbfs).filter(|v| v.is_finite() && *v > -90.0).unwrap_or(-28.0),
            goal,
            distance_text,
            quiet_target_dbfs: ctx.profile.targets().noise_floor.met,
            epoch_t: 0.0,
            at_limit_up: None,
            base_talk: Vec::new(),
            base: None,
            offset: None,
            drops: VecDeque::new(),
            quiet_run: 0,
            attempt: None,
            streak: 0,
            attempts: 0,
            last_attempt_t: 0.0,
            last_clean: None,
        }
    }

    /// The app will move the gain itself (`auto_gain_step`).
    pub fn with_auto_gain(mut self, auto: bool) -> Self {
        self.auto = auto;
        self
    }

    pub fn with_goal(mut self, goal: DistanceGoal) -> Self {
        self.goal = goal;
        self
    }

    /// Override how long the "no progress" timer runs before Stuck.
    pub fn with_stuck_after(mut self, s: f64) -> Self {
        self.tracker.stuck_after_s = s;
        self
    }

    pub fn task(&self) -> CoachTaskKind {
        self.task
    }

    pub fn goal(&self) -> DistanceGoal {
        self.goal
    }

    pub fn is_done(&self) -> bool {
        self.tracker.is_done()
    }

    fn now(&self) -> f64 {
        self.n_samples as f64 / self.sr as f64
    }

    /// Feed mono samples (any chunk size). Returns the newest update produced
    /// by this chunk (at most one per 100 ms of audio).
    pub fn push(&mut self, samples: &[f32]) -> Option<CoachUpdate> {
        let mut out = None;
        for &s in samples {
            let x = s as f64;
            let l0 = self.lf[0].process(x);
            let l = self.lf[1].process(l0);
            let m0 = self.mid[0].process(x);
            let m = self.mid[1].process(m0);
            let h0 = self.hi[0].process(x);
            let h = self.hi[1].process(h0);
            self.acc[0] += x * x;
            self.acc[1] += l * l;
            self.acc[2] += m * m;
            self.acc[3] += h * h;
            self.acc_peak = self.acc_peak.max(x.abs());
            self.acc_n += 1;
            self.n_samples += 1;
            if self.acc_n >= self.frame_len {
                let n = self.acc_n as f64;
                let fr = Fr {
                    t: self.now(),
                    rms: db_pow(self.acc[0] / n),
                    peak: 20.0 * self.acc_peak.max(1e-9).log10(),
                    lf: db_pow(self.acc[1] / n),
                    mid: db_pow(self.acc[2] / n),
                    hi: db_pow(self.acc[3] / n),
                    talking: false,
                };
                self.acc = [0.0; 4];
                self.acc_peak = 0.0;
                self.acc_n = 0;
                if let Some(u) = self.on_frame(fr) {
                    out = Some(u);
                }
            }
        }
        out
    }

    fn on_frame(&mut self, mut fr: Fr) -> Option<CoachUpdate> {
        // Noise floor: fast down, 1 dB/s up.
        let noise = match self.noise {
            None => fr.rms,
            Some(n) if fr.rms < n => fr.rms,
            Some(n) => n + 0.02,
        };
        self.noise = Some(noise);
        fr.talking = fr.rms > noise + 12.0 && fr.rms > -75.0;
        let prev_talking = self.hist.back().map(|p| p.talking).unwrap_or(false);

        // Decay proxy: how fast a word dies away (drier = closer).
        if prev_talking && !fr.talking {
            let start = self.hist.back().map(|p| p.rms).unwrap_or(fr.rms);
            self.offset = Some((start, 0));
        }
        if let Some((start, k)) = self.offset {
            if fr.talking {
                self.offset = None;
            } else if k + 1 >= 3 {
                self.drops.push_back(start - fr.rms);
                if self.drops.len() > 8 {
                    self.drops.pop_front();
                }
                self.offset = None;
            } else {
                self.offset = Some((start, k + 1));
            }
        }

        // Pops: each word onset after a short pause is one "attempt".
        if self.task == CoachTaskKind::AngleOffAxis {
            self.track_pops(&fr);
        }

        // Baseline for distance / logo tasks: everything from the first word
        // until 2.5 s of talking has been heard.
        if self.base.is_none() && (fr.talking || !self.base_talk.is_empty()) && matches!(self.task, CoachTaskKind::FindDistance | CoachTaskKind::TalkIntoLogo) {
            self.base_talk.push(fr);
            if self.base_talk.iter().filter(|f| f.talking).count() as f64 * FRAME_S >= BASELINE_TALK_S {
                let (level, lf_ratio, tilt) = voice_signature(&self.base_talk);
                let drop = if self.drops.is_empty() { 0.0 } else { self.drops.iter().sum::<f64>() / self.drops.len() as f64 };
                self.base = Some(Base { level, lf_ratio, tilt, drop });
            }
        }

        self.hist.push_back(fr);
        while self.hist.front().is_some_and(|f| fr.t - f.t > 10.0) {
            self.hist.pop_front();
        }
        self.frames_since_update += 1;
        if self.frames_since_update >= UPDATE_EVERY_FRAMES {
            self.frames_since_update = 0;
            return Some(self.evaluate());
        }
        None
    }

    fn track_pops(&mut self, fr: &Fr) {
        let burst = fr.lf > fr.mid + 6.0 && fr.lf > fr.rms - 4.0;
        if fr.talking {
            if self.attempt.is_none() && self.quiet_run >= 4 {
                self.attempt = Some((0, false));
            }
            self.quiet_run = 0;
            if let Some((k, popped)) = self.attempt {
                let popped = popped || burst;
                if k + 1 >= 5 {
                    self.finish_attempt(fr.t, popped);
                } else {
                    self.attempt = Some((k + 1, popped));
                }
            }
        } else {
            self.quiet_run += 1;
            if let Some((k, popped)) = self.attempt {
                if k > 0 {
                    self.finish_attempt(fr.t, popped || burst);
                } else {
                    self.attempt = None;
                }
            }
        }
    }

    fn finish_attempt(&mut self, t: f64, popped: bool) {
        self.attempt = None;
        self.attempts += 1;
        self.last_attempt_t = t;
        self.last_clean = Some(!popped);
        self.streak = if popped { 0 } else { self.streak + 1 };
    }

    fn recent(&self, secs: f64, since: f64) -> impl Iterator<Item = &Fr> {
        let now = self.now();
        self.hist.iter().filter(move |f| now - f.t <= secs && f.t >= since)
    }

    /// The talking-level measurement for SetGain: (p95 peak, talk seconds, clipped).
    fn gain_reading(&self) -> (Option<f64>, f64, bool) {
        let frames: Vec<&Fr> = self.recent(1.5, self.epoch_t).collect();
        let clipped = frames.iter().any(|f| f.peak >= -0.3);
        let peaks: Vec<f64> = frames.iter().filter(|f| f.talking).map(|f| f.peak).collect();
        let talk_s = peaks.len() as f64 * FRAME_S;
        let v = if peaks.len() >= 5 { Some(percentile(&peaks, 95.0)) } else { None };
        (v, talk_s, clipped)
    }

    /// Auto mode: given the current software gain (dB) and its range, return
    /// the next gain to apply, or `None` to leave it. Damped (70 % of the
    /// error per step) so it approaches the zone without overshooting; waits
    /// for ≥ 0.8 s of fresh talking after every change.
    pub fn auto_gain_step(&mut self, current_db: f64, min_db: f64, max_db: f64) -> Option<f64> {
        let (v, talk_s, clipped) = self.gain_reading();
        let delta = if clipped && talk_s >= 0.2 {
            -6.0
        } else {
            let v = v?;
            if talk_s < 0.8 || (GAIN_ZONE.0..=GAIN_ZONE.1).contains(&v) {
                return None;
            }
            ((GAIN_CENTRE - v) * 0.7).clamp(-12.0, 12.0)
        };
        let next = (current_db + delta).clamp(min_db, max_db);
        if (next - current_db).abs() < 0.5 {
            self.at_limit_up = Some(delta > 0.0);
            return None;
        }
        self.at_limit_up = None;
        self.epoch_t = self.now();
        self.tracker.reset_hold();
        Some(next)
    }

    /// Tell the coach the gain was changed outside `auto_gain_step` (e.g. the
    /// GoXLR knob): older audio no longer counts.
    pub fn gain_changed(&mut self) {
        self.epoch_t = self.now();
        self.tracker.reset_hold();
    }

    fn evaluate(&mut self) -> CoachUpdate {
        let t = self.now();
        let mut readings = BTreeMap::new();
        if let Some(n) = self.noise {
            readings.insert("noise_dbfs".into(), round1(n));
        }
        if let Some(f) = self.hist.back() {
            readings.insert("level_dbfs".into(), round1(f.rms));
            readings.insert("peak_dbfs".into(), round1(f.peak));
        }
        let mut u = match self.task {
            CoachTaskKind::SetGain => self.eval_gain(t),
            CoachTaskKind::FindDistance => self.eval_distance(t),
            CoachTaskKind::AngleOffAxis => self.eval_pops(t),
            CoachTaskKind::QuietRoom => self.eval_quiet(t),
            CoachTaskKind::KeyboardNull => self.eval_keyboard(t),
            CoachTaskKind::TalkIntoLogo => self.eval_logo(t),
            other => {
                let mut u = CoachUpdate::simple(other, Phase::Waiting, "This step isn't a mic step", "");
                u.t_s = t;
                u
            }
        };
        u.readings.extend(readings);
        u.t_s = round1(t * 10.0) / 10.0;
        if u.phase == Phase::Stuck {
            u.stuck_alternative = Some(self.stuck_text().to_string());
        }
        u
    }

    fn base_update(&self, step: Step, value: Option<f64>, target: Option<CoachTarget>, headline: String, detail: String, hint: Hint) -> CoachUpdate {
        CoachUpdate {
            task: self.task,
            phase: step.phase,
            progress: (step.progress * 100.0).round() / 100.0,
            value: value.map(round1),
            target,
            headline,
            detail,
            hint,
            hold_s: round1(step.hold_s),
            hold_needed_s: self.tracker.hold_needed_s,
            readings: BTreeMap::new(),
            auto_adjusting: false,
            stuck_alternative: None,
            t_s: 0.0,
        }
    }

    fn stuck_text(&self) -> &'static str {
        match self.task {
            CoachTaskKind::SetGain => "Can't get into the green? If the gain knob is already all the way up, move the mic closer instead — about a fist away. On an interface, make sure you're turning the knob for the input your mic is plugged into.",
            CoachTaskKind::FindDistance => "Try this: put your fist between your mouth and the mic, then take your hand away. That's the sweet spot for most streaming mics.",
            CoachTaskKind::AngleOffAxis => "Point the mic at the corner of your mouth instead of straight at it, so you talk just past it — or put a pop filter in front.",
            CoachTaskKind::QuietRoom => "Some noise can't be switched off — that's OK. Point the back of the mic at the noise and tap \"I can't do this\": we'll tune around it.",
            CoachTaskKind::KeyboardNull => "If the mic can't move, put a towel or desk mat under the keyboard, or tap \"I can't do this\" and we'll quiet the gaps between your words.",
            CoachTaskKind::TalkIntoLogo => "Find the side with the brand logo or the grille facing out — that's the front. Talk into that side, not the top.",
            _ => "Tap \"I can't do this\" and we'll work around it.",
        }
    }

    fn knob_words(diff: f64) -> &'static str {
        let d = diff.abs();
        if d < 3.0 {
            "just a touch"
        } else if d < 7.0 {
            "a little — about a quarter turn"
        } else if d < 13.0 {
            "a good amount — about half a turn"
        } else {
            "a lot — about three-quarters of a turn"
        }
    }

    fn eval_gain(&mut self, t: f64) -> CoachUpdate {
        let target = CoachTarget::new(GAIN_ZONE.0, GAIN_ZONE.1, "dBFS");
        let (v, talk_s, clipped) = self.gain_reading();
        let auto = self.auto;
        if clipped {
            let step = self.tracker.measure(t, false, 0.3);
            let (h, d) = if auto {
                ("Too loud — turning it down".to_string(), "You hit the maximum level; we're lowering the gain. Keep talking normally.".to_string())
            } else {
                ("Too loud — turn the gain down".to_string(), "Your voice hit the maximum and crackled. Turn the gain knob down about a quarter turn.".to_string())
            };
            let mut u = self.base_update(step, v, Some(target), h, d, Hint::new(HintKind::TurnDown, 0.6, "Turn the gain down"));
            u.auto_adjusting = auto;
            return u;
        }
        let Some(v) = v.filter(|_| talk_s >= 0.4) else {
            let step = self.tracker.waiting(t);
            let mut u = self.base_update(
                step,
                None,
                Some(target),
                "Keep talking like you do on stream".into(),
                if auto { "We'll set your level while you talk — say anything, read the chat, describe your day.".into() } else { "We need a few seconds of normal talking to read your level.".into() },
                Hint::new(HintKind::KeepTalking, 0.5, "Keep talking"),
            );
            u.auto_adjusting = auto;
            return u;
        };
        // A little hysteresis once holding so small wobbles don't reset the ring.
        let holding = self.tracker_holding();
        let zone_ok = if holding { (GAIN_ZONE.0 - 1.5..=GAIN_ZONE.1 + 1.5).contains(&v) } else { target.contains(v) };
        let miss = target.signed_miss(v);
        let raw = 0.78 * (1.0 - (miss.abs().min(24.0) / 24.0));
        let step = self.tracker.measure(t, zone_ok, raw);
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("Perfect level!".into(), "Your talking sits right in the green zone.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if zone_ok {
            ("Perfect — hold it there…".into(), "Keep talking normally while the ring fills.".into(), Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if miss < 0.0 {
            let words = Self::knob_words(miss);
            if auto && self.at_limit_up == Some(true) {
                (
                    "Your gain is at maximum".into(),
                    format!("We can't turn it up further. Move the mic closer — {} — and keep talking.", self.distance_text),
                    Hint::new(HintKind::Closer, (miss.abs() / 15.0).min(1.0), "Move closer"),
                )
            } else if auto {
                ("Turning you up… keep talking".into(), "We're raising your mic level step by step.".into(), Hint::new(HintKind::TurnUp, (miss.abs() / 15.0).min(1.0), "Raising the level"))
            } else {
                (
                    format!("Turn the gain up {words}"),
                    "Turn slowly while you talk and watch the meter move into the green.".into(),
                    Hint::new(HintKind::TurnUp, (miss.abs() / 15.0).min(1.0), format!("Turn up {words}")),
                )
            }
        } else {
            let words = Self::knob_words(miss);
            if auto {
                ("Turning you down a bit… keep talking".into(), "You're a little hot; we're lowering the level.".into(), Hint::new(HintKind::TurnDown, (miss / 12.0).min(1.0), "Lowering the level"))
            } else {
                (
                    format!("Turn the gain down {words}"),
                    "Leave room for laughs and shouts so they don't crackle.".into(),
                    Hint::new(HintKind::TurnDown, (miss / 12.0).min(1.0), format!("Turn down {words}")),
                )
            }
        };
        let mut u = self.base_update(step, Some(v), Some(target), headline, detail, hint);
        u.auto_adjusting = auto && !zone_ok;
        u.readings.insert("talk_s".into(), round1(talk_s));
        u
    }

    fn tracker_holding(&self) -> bool {
        self.tracker.hold_s() > 0.0
    }

    fn talking_window(&self, secs: f64) -> Vec<Fr> {
        self.recent(secs, 0.0).filter(|f| f.talking).copied().collect()
    }

    fn window(&self, secs: f64) -> Vec<Fr> {
        self.recent(secs, 0.0).copied().collect()
    }

    fn baseline_wait(&mut self, t: f64) -> CoachUpdate {
        let step = self.tracker.waiting(t);
        let have = self.base_talk.len() as f64 * FRAME_S;
        let mut u = self.base_update(
            step,
            None,
            None,
            "Talk normally for a few seconds".into(),
            "Stay where you are — we're listening to how you sound right now.".into(),
            Hint::new(HintKind::KeepTalking, 0.5, "Keep talking"),
        );
        u.readings.insert("baseline_progress".into(), round1((have / BASELINE_TALK_S).min(1.0)));
        u
    }

    fn eval_distance(&mut self, t: f64) -> CoachUpdate {
        let Some(base) = self.base else { return self.baseline_wait(t) };
        let (lo, hi) = match self.goal {
            DistanceGoal::Closer => (5.0, 14.0),
            DistanceGoal::Farther => (-9.0, -3.0),
            DistanceGoal::Confirm => (-2.0, 2.0),
        };
        let target = CoachTarget::new(lo, hi, "dB vs start");
        if (self.talking_window(1.5).len() as f64) * FRAME_S < 0.5 {
            let step = self.tracker.waiting(t);
            return self.base_update(step, None, Some(target), "Keep talking while you move".into(), "We listen while you move the mic.".into(), Hint::new(HintKind::KeepTalking, 0.5, "Keep talking"));
        }
        let (lvl, lfr, _) = voice_signature(&self.window(2.0));
        let drop = if self.drops.is_empty() { base.drop } else { self.drops.iter().rev().take(5).sum::<f64>() / self.drops.len().min(5) as f64 };
        let closeness = 0.75 * (lvl - base.level) + 0.25 * (lfr - base.lf_ratio) + 0.15 * (drop - base.drop);
        let miss = target.signed_miss(closeness);
        let slack = if self.tracker.hold_s() > 0.0 { 1.5 } else { 0.0 };
        let in_zone = miss.abs() <= slack;
        let raw = 0.78 * (1.0 - (miss.abs().min(12.0) / 12.0));
        let step = self.tracker.measure(t, in_zone, raw);
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("That's the spot!".into(), "You sound close and clear. Remember this distance.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("That sounds right — hold that spot".into(), "Keep talking while the ring fills.".into(), Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if miss < 0.0 {
            let h = if closeness > 1.5 { "Sounds closer — keep going" } else { "Bring the mic a little closer" };
            (h.into(), format!("Aim for {} from your mouth. Don't touch the gain while we do this.", self.distance_text), Hint::new(HintKind::Closer, (miss.abs() / 8.0).min(1.0), "Closer"))
        } else {
            let h = if self.goal == DistanceGoal::Farther && closeness < -1.0 { "Sounds less boomy — a little more" } else { "That's a bit too close — back off a touch" };
            (h.into(), "Very close makes your voice boomy and pops your P's.".into(), Hint::new(HintKind::Farther, (miss / 8.0).min(1.0), "Back off"))
        };
        let mut u = self.base_update(step, Some(closeness), Some(target), headline, detail, hint);
        u.readings.insert("level_change_db".into(), round1(lvl - base.level));
        u.readings.insert("bass_change_db".into(), round1(lfr - base.lf_ratio));
        u.readings.insert("dryness_change_db".into(), round1(drop - base.drop));
        u
    }

    fn eval_pops(&mut self, t: f64) -> CoachUpdate {
        if self.streak >= 3 {
            self.tracker.finish();
        }
        let target = CoachTarget::new(3.0, 99.0, "clean pops in a row");
        let idle = self.attempts == 0 || t - self.last_attempt_t > 2.5;
        let step = if self.tracker.is_done() || self.attempts == 0 {
            self.tracker.waiting(t)
        } else {
            self.tracker.measure(t, false, 0.78 * self.streak as f64 / 3.0)
        };
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("No more pops!".into(), "Your P's and B's come through clean. Keep the mic at this angle.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if self.last_clean == Some(false) && !idle {
            ("That one popped".into(), "Turn the mic a little to the side so you talk just past it, then say \"pop\" again.".into(), Hint::new(HintKind::AngleAway, 0.7, "Angle the mic away"))
        } else if self.last_clean == Some(true) && !idle {
            (format!("Clean! {} more", 3 - self.streak.min(3)), "Keep the mic where it is and say \"pop\" again.".into(), Hint::new(HintKind::SayPops, 0.3, "Say \"pop\""))
        } else {
            ("Say \"pop\" … \"pop\" … \"pop\"".into(), "Say it like you normally talk, with a short pause between each one.".into(), Hint::new(HintKind::SayPops, 0.5, "Say \"pop\""))
        };
        let mut u = self.base_update(step, Some(self.streak as f64), Some(target), headline, detail, hint);
        u.readings.insert("attempts".into(), self.attempts as f64);
        u
    }

    fn eval_quiet(&mut self, t: f64) -> CoachUpdate {
        let target = CoachTarget::new(-120.0, self.quiet_target_dbfs, "dBFS");
        let talking_recent = self.recent(0.6, 0.0).any(|f| f.talking);
        let quiet: Vec<Fr> = self.recent(1.5, 0.0).filter(|f| !f.talking).copied().collect();
        if talking_recent || (quiet.len() as f64) * FRAME_S < 0.5 {
            let step = self.tracker.waiting(t);
            return self.base_update(
                step,
                None,
                Some(target),
                "Shh — stay quiet for a moment".into(),
                "We're listening to your room. Don't talk or type while we measure.".into(),
                Hint::new(HintKind::BeQuiet, 0.5, "Stay quiet"),
            );
        }
        let floor = median(&quiet.iter().map(|f| f.rms).collect::<Vec<_>>());
        let lf = median(&quiet.iter().map(|f| f.lf).collect::<Vec<_>>());
        let hi = median(&quiet.iter().map(|f| f.hi).collect::<Vec<_>>());
        let rumble = lf > hi + 10.0;
        let in_zone = floor <= target.max;
        let raw = 0.78 * ((target.max + 20.0 - floor) / 20.0).clamp(0.0, 1.0);
        let step = self.tracker.measure(t, in_zone, raw);
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("Nice and quiet!".into(), "Your room noise is low enough that it won't distract viewers.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("Quiet — hold on…".into(), "Stay silent while the ring fills.".into(), Hint::new(HintKind::BeQuiet, 0.2, "Stay quiet"))
        } else if rumble {
            (
                "There's a rumble or whoosh".into(),
                "That's usually a fan, PC or air conditioner. Turn it off or down, or turn the back of the mic toward it. Watch the number drop.".into(),
                Hint::new(HintKind::AngleAway, ((floor + 60.0) / 20.0).clamp(0.0, 1.0), "Point the mic's back at the noise"),
            )
        } else {
            (
                "There's a hiss in the background".into(),
                format!("Usually the gain is turned up to make up for distance. Move closer ({}) and lower the gain a little.", self.distance_text),
                Hint::new(HintKind::TurnDown, ((floor + 60.0) / 20.0).clamp(0.0, 1.0), "Closer, then less gain"),
            )
        };
        let mut u = self.base_update(step, Some(floor), Some(target), headline, detail, hint);
        u.readings.insert("noise_lf_dbfs".into(), round1(lf));
        u.readings.insert("noise_hf_dbfs".into(), round1(hi));
        u
    }

    fn eval_keyboard(&mut self, t: f64) -> CoachUpdate {
        let target = CoachTarget::new(-80.0, -20.0, "dB vs your voice");
        let noise = self.noise.unwrap_or(-90.0);
        let frames: Vec<Fr> = self.recent(2.0, 0.0).copied().collect();
        let active: Vec<&Fr> = frames.iter().filter(|f| f.rms > noise + 8.0).collect();
        // Long runs of activity = talking, not typing.
        let mut run = 0usize;
        let mut longest = 0usize;
        for f in &frames {
            if f.rms > noise + 8.0 {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        if active.len() < 3 {
            let step = self.tracker.waiting(t);
            return self.base_update(step, None, Some(target), "Type on your keyboard".into(), "Type like you do in a game, without talking.".into(), Hint::new(HintKind::TypeNow, 0.5, "Type now"));
        }
        if longest >= 20 {
            let step = self.tracker.waiting(t);
            return self.base_update(step, None, Some(target), "Just type — no talking".into(), "We only want to hear the keyboard for this one.".into(), Hint::new(HintKind::TypeNow, 0.5, "Type, don't talk"));
        }
        let rel = percentile(&frames.iter().map(|f| f.rms).collect::<Vec<_>>(), 95.0) - self.speech_rms;
        let in_zone = rel <= target.max;
        let raw = 0.78 * ((-5.0 - rel) / 15.0).clamp(0.0, 1.0);
        let step = self.tracker.measure(t, in_zone, raw);
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("Keyboard tamed!".into(), "Typing is now well below your voice.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("Much better — keep typing…".into(), "Hold the mic there while the ring fills.".into(), Hint::new(HintKind::TypeNow, 0.2, "Keep typing"))
        } else {
            (
                "Your keyboard is too loud".into(),
                "Turn the back of the mic toward the keyboard and bring the front closer to your mouth. Keep typing and watch it drop.".into(),
                Hint::new(HintKind::AngleAway, ((rel + 20.0) / 15.0).clamp(0.0, 1.0), "Point the mic's back at the keyboard"),
            )
        };
        let mut u = self.base_update(step, Some(rel), Some(target), headline, detail, hint);
        u.readings.insert("typing_dbfs".into(), round1(rel + self.speech_rms));
        u
    }

    fn eval_logo(&mut self, t: f64) -> CoachUpdate {
        let Some(base) = self.base else { return self.baseline_wait(t) };
        let target = CoachTarget::new(4.0, 40.0, "dB clearer");
        if (self.talking_window(1.5).len() as f64) * FRAME_S < 0.5 {
            let step = self.tracker.waiting(t);
            return self.base_update(step, None, Some(target), "Keep talking while you turn the mic".into(), "".into(), Hint::new(HintKind::KeepTalking, 0.5, "Keep talking"));
        }
        let (_, _, tilt) = voice_signature(&self.window(2.0));
        let rise = tilt - base.tilt;
        let in_zone = rise >= target.min;
        let raw = 0.78 * (rise / target.min).clamp(0.0, 1.0);
        let step = self.tracker.measure(t, in_zone, raw);
        let (headline, detail, hint) = if step.phase == Phase::Done {
            ("Much clearer!".into(), "You're talking into the front of the mic now.".into(), Hint::new(HintKind::Stop, 0.0, "Done"))
        } else if in_zone {
            ("That's it — hold it there".into(), "Keep talking while the ring fills.".into(), Hint::new(HintKind::Stop, 0.0, "Hold"))
        } else if rise > 1.5 {
            ("Clearer — keep turning".into(), "Keep turning until the logo side faces your mouth.".into(), Hint::new(HintKind::FaceLogo, 0.5, "Face the logo"))
        } else {
            (
                "Turn the mic so the logo faces you".into(),
                "This mic hears from its side, not its top. Talk into the side with the logo.".into(),
                Hint::new(HintKind::FaceLogo, 1.0, "Logo side to your mouth"),
            )
        };
        let mut u = self.base_update(step, Some(rise), Some(target), headline, detail, hint);
        u.readings.insert("clarity_change_db".into(), round1(rise));
        u
    }
}

/// (level, low/mid ratio, high/mid tilt) of a stretch of audio, measured on
/// its loud frames (within 10 dB of the 90th percentile) so it doesn't depend
/// on where the talking threshold happens to sit.
fn voice_signature(frames: &[Fr]) -> (f64, f64, f64) {
    let lv: Vec<f64> = frames.iter().map(|f| f.rms).collect();
    let level = percentile(&lv, 90.0);
    let loud: Vec<&Fr> = frames.iter().filter(|f| f.rms >= level - 10.0).collect();
    let lfr = median(&loud.iter().map(|f| f.lf - f.mid).collect::<Vec<_>>());
    let tilt = median(&loud.iter().map(|f| f.hi - f.mid).collect::<Vec<_>>());
    (level, lfr, tilt)
}
