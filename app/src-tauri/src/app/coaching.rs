//! Live coaching sessions (API.md "Live coaching"): plan steps from the last
//! reports, stream mic chunks / camera frames into the core coaches, emit
//! `coach-update` ~8×/s, drive software gain / camera settings in auto mode,
//! and watch other apps' settings for `CheckSetting` steps.

use super::*;
use crate::goxlr::{self, GoXlrApi, GoXlrDevice, GoXlrHttp, MicType};
use crate::platform::GainControl;
use coach_core::coach::{CoachTaskKind as T, CoachUpdate, LiveCameraCoach, LiveMicCoach, Phase, STUCK_AFTER_S};
use coach_core::system::ids;
use coach_core::video::{Rect, UvcSettings};

/// Controls our own window while the user works in another app.
pub trait WindowControl: Send + Sync {
    /// Small, always-on-top "mini coach" while the user clicks elsewhere.
    fn set_mini(&self, on: bool);
    /// Bring Setup Coach back to the front.
    fn focus(&self);
}

pub struct NoWindow;
impl WindowControl for NoWindow {
    fn set_mini(&self, _on: bool) {}
    fn focus(&self) {}
}

/// Records window calls (tests).
#[derive(Default)]
pub struct RecordingWindow {
    pub calls: Mutex<Vec<String>>,
}
impl WindowControl for RecordingWindow {
    fn set_mini(&self, on: bool) {
        self.calls.lock().unwrap().push(format!("mini:{on}"));
    }
    fn focus(&self) {
        self.calls.lock().unwrap().push("focus".into());
    }
}

pub(super) struct CoachSession {
    running: Arc<AtomicBool>,
    capture: Arc<Mutex<Option<Box<dyn StreamHandle>>>>,
}

impl CoachSession {
    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.capture.lock().unwrap().take() {
            h.stop();
        }
    }
}

/// Where auto mode moves the gain.
#[derive(Clone)]
enum GainSource {
    Windows { endpoint: String },
    GoXlr { api: Arc<dyn GoXlrApi>, serial: String, mic_type: MicType },
}

/// Logical seconds per coach tick (independent of the real tick duration so
/// tests can run fast).
const TICK_S: f64 = 0.125;
const GOXLR_UTILITY_URL: &str = "https://github.com/GoXLR-on-Linux/goxlr-utility/releases/latest";

fn step(id: &str, finding: Option<&str>, task: T, title: &str, explain: &str, animation: &str) -> CoachStep {
    CoachStep {
        id: id.to_string(),
        finding_id: finding.map(str::to_string),
        task,
        title: title.to_string(),
        explain: explain.to_string(),
        animation: animation.to_string(),
        auto: false,
        open_target: None,
        check: None,
    }
}

fn settings_target(uri: &str, label: &str, steps: &[&str]) -> OpenTarget {
    OpenTarget { kind: OpenKind::WindowsSettings, uri_or_app: uri.into(), label: label.into(), steps: steps.iter().map(|s| s.to_string()).collect() }
}

impl AppCore {
    pub fn set_window_control(&self, w: Arc<dyn WindowControl>) {
        *self.window.lock().unwrap() = w;
    }

    fn win(&self) -> Arc<dyn WindowControl> {
        self.window.lock().unwrap().clone()
    }

    /// Use a GoXLR Utility connection (tests) or forget it (`None`).
    pub async fn set_goxlr(&self, api: Option<Arc<dyn GoXlrApi>>) {
        *self.goxlr.lock().await = api;
    }

    /// The GoXLR Utility daemon, if it's running (cached once found).
    pub async fn goxlr(&self) -> Option<Arc<dyn GoXlrApi>> {
        let mut g = self.goxlr.lock().await;
        if g.is_none() && self.goxlr_autodetect.load(Ordering::SeqCst) {
            if let Some(c) = GoXlrHttp::detect(goxlr::DEFAULT_PORT).await {
                *g = Some(Arc::new(c));
            }
        }
        g.clone()
    }

    pub(super) async fn goxlr_device(&self) -> Option<(Arc<dyn GoXlrApi>, GoXlrDevice)> {
        let api = self.goxlr().await?;
        let dev = api.devices().await.ok()?.into_iter().next()?;
        Some((api, dev))
    }

    pub(super) fn device_name(&self, id: &str) -> Option<String> {
        self.platform.audio.list_inputs().into_iter().find(|d| d.id == id).map(|d| d.name)
    }

    fn coach_mic_device(&self) -> Option<String> {
        let last = self.inner.lock().unwrap().last_device.clone();
        let inputs = self.platform.audio.list_inputs();
        last.filter(|id| inputs.iter().any(|d| &d.id == id)).or_else(|| inputs.iter().find(|d| d.is_default).or(inputs.first()).map(|d| d.id.clone()))
    }

    fn is_goxlr_device(&self, id: &str) -> bool {
        self.device_name(id).is_some_and(|n| n.to_ascii_lowercase().contains("goxlr"))
    }

    /// `mic_context` plus what GoXLR Utility can tell us (real gain range,
    /// gain maxed or not).
    pub async fn mic_context_async(&self, device_id: &str, answers: &MicAnswers) -> R<MicContext> {
        let mut ctx = self.mic_context(device_id, answers)?;
        if self.is_goxlr_device(device_id) {
            if let Some((_, dev)) = self.goxlr_device().await {
                ctx.interface_max_gain_db = Some(goxlr::GAIN_RANGE_DB.1);
                if answers.gain_at_max.is_none() {
                    ctx.gain_at_max = Some(dev.mic_gain_db >= goxlr::GAIN_RANGE_DB.1 - 2.0);
                }
            }
        }
        Ok(ctx)
    }

    /// The software gain auto mode may move for this device, if any.
    async fn gain_source(&self, device_id: &str) -> Option<GainSource> {
        if self.is_goxlr_device(device_id) {
            if let Some((api, dev)) = self.goxlr_device().await {
                return Some(GainSource::GoXlr { api, serial: dev.serial, mic_type: dev.mic_type });
            }
        }
        let dev = self.platform.audio.list_inputs().into_iter().find(|d| d.id == device_id)?;
        let hw = self.effective_mic(&dev).and_then(|p| p.windows_slider_is_hw_gain).unwrap_or(false);
        (hw && self.platform.system.endpoint_gain_db(device_id).is_some()).then(|| GainSource::Windows { endpoint: device_id.to_string() })
    }

    async fn gain_get(&self, src: &GainSource) -> Option<GainControl> {
        match src {
            GainSource::Windows { endpoint } => self.platform.system.endpoint_gain_db(endpoint),
            GainSource::GoXlr { api, .. } => {
                let d = api.devices().await.ok()?.into_iter().next()?;
                Some(GainControl { current_db: d.mic_gain_db, min_db: goxlr::GAIN_RANGE_DB.0, max_db: goxlr::GAIN_RANGE_DB.1 })
            }
        }
    }

    async fn gain_set(&self, src: &GainSource, db: f64) -> R<()> {
        let (target, key) = match src {
            GainSource::Windows { endpoint } => (Target::EndpointGain(endpoint.clone()), format!("gain:{endpoint}")),
            GainSource::GoXlr { serial, .. } => (Target::GoXlr, format!("goxlr:{serial}")),
        };
        let done = self.inner.lock().unwrap().coach_backed_up.contains(&key);
        if !done {
            self.backup_before("Before auto-adjusting your mic level", &[target]).await?;
            let mut inner = self.inner.lock().unwrap();
            inner.coach_backed_up.push(key);
            if let Some(d) = inner.last_device.clone() {
                inner.applied_devices.insert(d);
            }
        }
        match src {
            GainSource::Windows { endpoint } => self.platform.system.set_endpoint_gain_db(endpoint, db),
            GainSource::GoXlr { api, serial, mic_type } => api.command(serial, goxlr::cmd_set_mic_gain(*mic_type, db)).await,
        }
    }

    // ------------------------------------------------------------ plan

    pub async fn coach_plan(&self, area: CoachArea) -> R<Vec<CoachStep>> {
        match area {
            CoachArea::Mic => self.mic_plan().await,
            CoachArea::Camera => self.camera_plan(),
        }
    }

    async fn mic_plan(&self) -> R<Vec<CoachStep>> {
        let (report, ctx, snap) = {
            let i = self.inner.lock().unwrap();
            (i.last_report.clone(), i.last_context.clone().unwrap_or_default(), i.last_snapshot.clone())
        };
        let device = self.coach_mic_device();
        let mut out: Vec<CoachStep> = Vec::new();
        let push = |s: CoachStep, out: &mut Vec<CoachStep>| {
            if s.task == T::CheckSetting || !out.iter().any(|x| x.task == s.task) {
                out.push(s);
            }
        };

        // 1) Settings in other apps that block everything else.
        let sys_ids: Vec<String> = snap.as_ref().map(|s| system::audit(s, &[]).into_iter().map(|f| f.id).collect()).unwrap_or_default();
        let has_sys = |id: &str| sys_ids.iter().any(|x| x == id);
        if has_sys(ids::GOXLR_OFFICIAL_APP) {
            let mut s = step(
                "check-goxlr-utility",
                Some(ids::GOXLR_OFFICIAL_APP),
                T::CheckSetting,
                "Switch to GoXLR Utility",
                "The official GoXLR App is no longer maintained. GoXLR Utility is the free replacement, and with it we can set your mic gain and phantom power for you.",
                "app_switch",
            );
            s.open_target = Some(OpenTarget {
                kind: OpenKind::VendorApp,
                uri_or_app: GOXLR_UTILITY_URL.into(),
                label: "the GoXLR Utility download page".into(),
                steps: vec![
                    "Download the Windows installer (.exe) from the page we just opened.".into(),
                    "Close the official GoXLR App (right-click its icon by the clock → Exit) and uninstall it — the two can't run together.".into(),
                    "Run the GoXLR Utility installer and start it.".into(),
                    "Come back here — we'll notice as soon as it's running.".into(),
                ],
            });
            s.check = Some("goxlr_utility_running".into());
            push(s, &mut out);
        }
        if has_sys(ids::MIC_PRIVACY) {
            let mut s = step("check-mic-privacy", Some(ids::MIC_PRIVACY), T::CheckSetting, "Let apps use your mic", "Windows is blocking desktop apps (like OBS) from hearing your mic.", "settings_toggle");
            s.open_target = Some(settings_target("ms-settings:privacy-microphone", "Windows microphone privacy settings", &["Turn on \"Microphone access\".", "Turn on \"Let desktop apps access your microphone\" (near the bottom)."]));
            s.check = Some("mic_privacy_allowed".into());
            push(s, &mut out);
        }
        for (id, check, title) in [
            (ids::VOICE_CLARITY, "voice_clarity_off", "Turn off Windows Voice Clarity"),
            (ids::ENHANCEMENTS, "enhancements_off", "Turn off Windows audio enhancements"),
        ] {
            if has_sys(id) {
                let mut s = step(&format!("check-{check}"), Some(id), T::CheckSetting, title, "Windows is processing your mic on top of everything else, which can make you sound muffled or warbly.", "settings_toggle");
                s.open_target = Some(settings_target("ms-settings:sound", "Windows sound settings", &["Click your microphone under \"Input\".", "Set \"Audio enhancements\" to Off (and turn off Voice Clarity if you see it)."]));
                s.check = Some(check.into());
                push(s, &mut out);
            }
        }

        // 2) GoXLR condenser mics need phantom power (mic type = Condenser).
        if let Some(dev) = device.as_deref().filter(|d| self.is_goxlr_device(d)) {
            let _ = dev;
            if let Some((_, gx)) = self.goxlr_device().await {
                if ctx.capsule == Some(findings_capsule_condenser()) && gx.mic_type != MicType::Condenser {
                    let mut s = step(
                        "check-goxlr-phantom",
                        None,
                        T::CheckSetting,
                        "Turn on phantom power",
                        "Condenser mics like yours need 48 V phantom power from the GoXLR. We'll switch the GoXLR's mic type to Condenser for you.",
                        "phantom",
                    );
                    s.auto = true;
                    s.check = Some("goxlr_condenser".into());
                    push(s, &mut out);
                }
            }
        }

        // 3) Physical coaching from the mic report, in the order that makes
        //    sense: where the mic points, how far, the angle, then the level.
        let Some(report) = report else { return Ok(out) };
        let open: Vec<&coach_core::Finding> = report.findings.iter().filter(|f| f.severity >= coach_core::Severity::Minor).collect();
        let has = |id: &str| open.iter().find(|f| f.id == id).map(|f| f.id.as_str());
        let any = |list: &[&'static str]| list.iter().find_map(|id| has(id));
        let dist = ctx.distance_cm.map(|(a, b)| format!("{a:.0}–{b:.0} cm")).unwrap_or_else(|| "about a fist away".into());
        if let Some(f) = has("mic.wrong_side") {
            push(step("talk-into-logo", Some(f), T::TalkIntoLogo, "Talk into the front of the mic", "This mic hears from its side (the logo side), not the top. Turn it while you talk and we'll tell you when it's right.", "logo_side"), &mut out);
        }
        if let Some(f) = any(&["mic.too_far", "mic.roomy", "mic.thin", "mic.boomy"]) {
            let explain = if f == "mic.boomy" {
                "You're very close, which makes your voice boomy. Back off slowly while you talk — we'll say when it sounds right."
            } else {
                "Distance matters more than any setting: closer = clearer, less echo and less noise. Move the mic while you talk — we'll say when it sounds right."
            };
            push(step("find-distance", Some(f), T::FindDistance, &format!("Find your mic's sweet spot ({dist})"), explain, "mic_distance"), &mut out);
        }
        if let Some(f) = has("mic.plosives") {
            push(step("angle-off-axis", Some(f), T::AngleOffAxis, "Stop the pops", "Puffs of air on P and B hit the mic. Angle it slightly to the side and say \"pop\" until we hear three clean ones.", "mic_angle"), &mut out);
        }
        let hiss = report.metrics.noise_character == coach_core::audio::analyze::NoiseCharacter::Hiss;
        let gain_ids: &[&str] = if hiss {
            &["mic.too_quiet", "mic.too_hot", "mic.clipping", "mic.near_clipping", "mic.not_enough_gain", "mic.background_noise"]
        } else {
            &["mic.too_quiet", "mic.too_hot", "mic.clipping", "mic.near_clipping", "mic.not_enough_gain"]
        };
        if let Some(f) = any(gain_ids) {
            let auto = match device.as_deref() {
                Some(d) => self.gain_source(d).await.is_some(),
                None => false,
            };
            let mut s = step(
                "set-gain",
                Some(f),
                T::SetGain,
                "Set your mic level",
                if auto {
                    "Just talk like you do on stream — we'll set the level for you and show it landing in the green zone."
                } else {
                    "Talk like you do on stream and turn the gain knob slowly — the meter tells you when to stop."
                },
                "knob",
            );
            s.auto = auto;
            push(s, &mut out);
        }
        if let Some(f) = any(&["mic.background_noise", "mic.hum"]).filter(|_| !hiss || has("mic.hum").is_some()) {
            push(step("quiet-room", Some(f), T::QuietRoom, "Quiet the background", "Stay silent while we listen to your room. Switch off or turn away from noisy things and watch the number drop.", "noise_null"), &mut out);
        }
        if let Some(f) = has("mic.keyboard") {
            push(step("keyboard-null", Some(f), T::KeyboardNull, "Tame the keyboard", "Type without talking and turn the back of the mic toward your keyboard — watch it drop.", "keyboard_null"), &mut out);
        }
        if let Some(f) = has("mic.auto_level_active") {
            let app = vendor_app_for(ctx.mic_name.as_deref().unwrap_or(""));
            let mut s = step("check-auto-level", Some(f), T::CheckSetting, "Turn off your mic's auto level", "Your mic levels itself, which fights the settings we set. Turn Auto Level / Auto Gain off in its app (keep clip protection on).", "settings_toggle");
            s.open_target = Some(OpenTarget { kind: OpenKind::VendorApp, uri_or_app: app.1.into(), label: app.0.into(), steps: vec![format!("In {}, find your mic's settings.", app.0), "Turn off Auto Level / Auto Gain / Blue VO!CE.".into(), "Come back and tap Done.".into()] });
            push(s, &mut out);
        }
        Ok(out)
    }

    fn camera_plan(&self) -> R<Vec<CoachStep>> {
        let (cam, snap) = {
            let i = self.inner.lock().unwrap();
            (i.last_camera.clone(), i.last_snapshot.clone())
        };
        let mut out = Vec::new();
        if snap.as_ref().and_then(|s| s.camera.as_ref()).is_some_and(|c| !c.privacy_allowed) {
            let mut s = step("check-camera-privacy", None, T::CheckSetting, "Let apps use your camera", "Windows is blocking desktop apps from your camera.", "settings_toggle");
            s.open_target = Some(settings_target("ms-settings:privacy-webcam", "Windows camera privacy settings", &["Turn on \"Camera access\".", "Turn on \"Let desktop apps access your camera\"."]));
            s.check = Some("camera_privacy_allowed".into());
            out.push(s);
        }
        let Some((_, report)) = cam else { return Ok(out) };
        let controllable = report.class.controllable();
        let ids: Vec<&str> = report.findings.iter().filter(|f| f.severity >= coach_core::Severity::Minor).map(|f| f.id.as_str()).collect();
        let any = |list: &[&str]| list.iter().find(|id| ids.contains(id)).map(|s| s.to_string());
        type Row<'a> = (&'a [&'a str], T, &'a str, &'a str, &'a str, bool);
        let table: [Row; 6] = [
            (&["camera.backlit", "camera.background_competes", "camera.face_needs_light", "camera.underexposed", "camera.too_dark_grainy", "camera.overexposed", "camera.face_clipping"], T::LightFace, "Light your face", "Good light beats any camera setting. Move your light (or yourself) while you watch the preview — we'll tell you when your face pops.", "light_move", false),
            (&["camera.harsh_shadows"], T::LightDirection, "Even out the shadows", "One side of your face is much darker. Move the light toward the darker side — the arrows show which way.", "light_side", false),
            (&["camera.colour_cast", "camera.face_tint", "camera.colour_drift"], T::WhiteBalance, "Make white look white", "Hold a white sheet of paper in the box. We'll read the colour of your light and fix it.", "paper", controllable),
            (&["camera.camera_low"], T::CameraHeight, "Camera at eye level", "A camera below your eyes looks up your nose. Raise it until the lens is level with your eyes.", "camera_height", false),
            (&["camera.framing", "camera.too_close", "camera.too_far"], T::Framing, "Frame yourself", "Head and shoulders, centred, eyes about a third from the top.", "framing", false),
            (&["camera.flicker", "camera.anti_flicker_wrong"], T::Flicker, "Stop the flicker", "Light bands come from your lights flickering with the mains power. We'll match the camera to your power grid and check the bands are gone.", "flicker", controllable),
        ];
        for (list, task, title, explain, anim, auto) in table {
            if let Some(f) = any(list) {
                let id = serde_json::to_value(task).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                let mut s = step(&id, Some(&f), task, title, explain, anim);
                s.auto = auto;
                out.push(s);
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------ checks

    /// Evaluate a `CheckSetting` predicate against a fresh snapshot.
    pub async fn check_passes(&self, check: &str) -> bool {
        match check {
            "goxlr_utility_running" => self.goxlr().await.is_some() || self.platform.system.processing_apps().goxlr_utility_running,
            "goxlr_condenser" => self.goxlr_device().await.is_some_and(|(_, d)| d.mic_type == MicType::Condenser),
            "obs_running" => self.platform.system.obs_running(),
            "camera_privacy_allowed" => self.platform.system.camera_system().is_some_and(|c| c.privacy_allowed),
            _ => {
                let snap = self.snapshot().await;
                let Some(w) = snap.windows.as_ref() else { return false };
                let ep = snap.mic_endpoint();
                match check {
                    "mic_privacy_allowed" => w.mic_privacy_desktop_allowed,
                    "enhancements_off" => ep.is_some_and(|e| !e.enhancements_enabled),
                    "voice_clarity_off" => ep.is_some_and(|e| !e.voice_clarity_enabled),
                    _ => false,
                }
            }
        }
    }

    pub async fn open_for_user(&self, target: &OpenTarget) -> R<()> {
        let sys = self.platform.system.clone();
        let t = target.clone();
        tokio::task::spawn_blocking(move || sys.open_target(&t)).await.map_err(|e| e.to_string())??;
        self.win().set_mini(true);
        Ok(())
    }

    // ------------------------------------------------------------ sessions

    pub fn stop_coach(&self) {
        if let Some(s) = self.inner.lock().unwrap().coach.take() {
            s.stop();
        }
        self.win().set_mini(false);
    }

    fn emit_update(&self, u: &CoachUpdate) {
        self.emitter.emit(crate::types::events::COACH_UPDATE, serde_json::to_value(u).unwrap_or(Value::Null));
    }

    pub async fn start_coach(self: &Arc<Self>, step: CoachStep) -> R<()> {
        self.stop_coach();
        self.stop_capture();
        self.inner.lock().unwrap().coach_backed_up.clear();
        let running = Arc::new(AtomicBool::new(true));
        let capture = Arc::new(Mutex::new(None));
        self.inner.lock().unwrap().coach = Some(CoachSession { running: running.clone(), capture: capture.clone() });
        if step.task == T::CheckSetting {
            self.spawn_check(step, running);
        } else if step.task.is_camera() {
            self.spawn_camera(step, running).await?;
        } else {
            self.spawn_mic(step, running, capture).await?;
        }
        Ok(())
    }

    fn finish(&self, running: &Arc<AtomicBool>) {
        running.store(false, Ordering::SeqCst);
        let session = {
            let mut i = self.inner.lock().unwrap();
            if i.coach.as_ref().is_some_and(|c| Arc::ptr_eq(&c.running, running)) { i.coach.take() } else { None }
        };
        if let Some(s) = session {
            s.stop();
        }
    }

    fn spawn_check(self: &Arc<Self>, step: CoachStep, running: Arc<AtomicBool>) {
        let me = self.clone();
        tokio::spawn(async move {
            let check = step.check.clone().unwrap_or_default();
            let mut ticks: u64 = 0;
            // Auto steps: make the change ourselves first.
            let mut auto_error: Option<String> = None;
            if step.auto && check == "goxlr_condenser" {
                if let Some((api, d)) = me.goxlr_device().await {
                    if let Err(e) = me.backup_before("Before turning on GoXLR phantom power", &[super::Target::GoXlr]).await {
                        auto_error = Some(e);
                    } else if let Err(e) = api.command(&d.serial, goxlr::cmd_set_mic_type(MicType::Condenser)).await {
                        auto_error = Some(e);
                    }
                }
            }
            while running.load(Ordering::SeqCst) {
                let t = ticks as f64 * TICK_S;
                let pass = !check.is_empty() && me.check_passes(&check).await;
                let mut u = if pass {
                    CoachUpdate::simple(T::CheckSetting, Phase::Done, "Done — that's sorted!", "We can see the change. Back to Setup Coach.")
                } else if check.is_empty() {
                    CoachUpdate::simple(T::CheckSetting, Phase::Waiting, &step.title, "Follow the steps, then tap Done.")
                } else if t > STUCK_AFTER_S {
                    let mut u = CoachUpdate::simple(T::CheckSetting, Phase::Stuck, &step.title, "We haven't seen the change yet.");
                    u.stuck_alternative = Some(auto_error.clone().unwrap_or_else(|| "Can't find it? Tap \"Show me again\" to reopen it, or \"I can't do this\" and we'll work around it.".into()));
                    u
                } else {
                    let mut u = CoachUpdate::simple(T::CheckSetting, Phase::Working, &step.title, "Waiting for the change… we'll bring you back here when it's done.");
                    u.auto_adjusting = step.auto;
                    u.hint = coach_core::coach::Hint::new(coach_core::coach::HintKind::Wait, 0.5, "Waiting for the change");
                    u
                };
                u.t_s = t;
                me.emit_update(&u);
                if pass {
                    me.win().set_mini(false);
                    me.win().focus();
                    me.finish(&running);
                    break;
                }
                tokio::time::sleep(me.coach_tick * 4).await;
                ticks += 4;
            }
        });
    }

    async fn spawn_mic(self: &Arc<Self>, step: CoachStep, running: Arc<AtomicBool>, capture: Arc<Mutex<Option<Box<dyn StreamHandle>>>>) -> R<()> {
        let device = self.coach_mic_device().ok_or("Plug in your mic first.")?;
        let (ctx, metrics) = {
            let i = self.inner.lock().unwrap();
            (i.last_context.clone().unwrap_or_default(), i.last_report.as_ref().map(|r| r.metrics.clone()))
        };
        let source = if step.task == T::SetGain { self.gain_source(&device).await } else { None };
        let auto = source.is_some();
        // Audio thread → shared buffer (peak-preserving mono: a left-only
        // interface input isn't halved).
        let buf: Arc<Mutex<(u32, Vec<f32>)>> = Arc::new(Mutex::new((0, Vec::new())));
        let sink_buf = buf.clone();
        let sink = Box::new(move |data: &[f32], ch: u16, sr: u32| {
            let ch = ch.max(1) as usize;
            let mut b = sink_buf.lock().unwrap();
            b.0 = sr;
            for frame in data.chunks(ch) {
                let v = frame.iter().cloned().fold(0.0f32, |m, x| if x.abs() > m.abs() { x } else { m });
                b.1.push(v);
            }
        });
        let handle = self.platform.audio.start_capture(&device, sink)?;
        *capture.lock().unwrap() = Some(handle);
        let me = self.clone();
        let task = step.task;
        tokio::spawn(async move {
            let mut coach: Option<LiveMicCoach> = None;
            let mut last_gain_check = 0usize;
            let mut n = 0usize;
            while running.load(Ordering::SeqCst) {
                tokio::time::sleep(me.coach_tick).await;
                n += 1;
                let (sr, chunk) = {
                    let mut b = buf.lock().unwrap();
                    (b.0, std::mem::take(&mut b.1))
                };
                if sr == 0 || chunk.is_empty() {
                    continue;
                }
                let c = coach.get_or_insert_with(|| LiveMicCoach::new(task, sr, &ctx, metrics.as_ref()).with_auto_gain(auto));
                if let Some(u) = c.push(&chunk) {
                    me.emit_update(&u);
                }
                if let (Some(src), false) = (&source, c.is_done()) {
                    if n - last_gain_check >= 2 {
                        last_gain_check = n;
                        if let Some(g) = me.gain_get(src).await {
                            if let Some(next) = c.auto_gain_step(g.current_db, g.min_db, g.max_db) {
                                if let Err(e) = me.gain_set(src, next).await {
                                    let mut u = CoachUpdate::simple(task, Phase::Stuck, "We couldn't change the level", &e);
                                    u.stuck_alternative = Some("Turn the gain yourself — the meter still shows when to stop.".into());
                                    me.emit_update(&u);
                                }
                            }
                        }
                    }
                }
                if c.is_done() {
                    me.finish(&running);
                    break;
                }
            }
        });
        Ok(())
    }

    async fn spawn_camera(self: &Arc<Self>, step: CoachStep, running: Arc<AtomicBool>) -> R<()> {
        let camera_id = {
            let i = self.inner.lock().unwrap();
            i.last_camera.as_ref().map(|c| c.0.clone()).or_else(|| i.camera.as_ref().map(|c| c.camera_id.clone()))
        }
        .or_else(|| self.platform.camera.list().first().map(|c| c.id.clone()))
        .ok_or("Plug in your camera first.")?;
        let source = match self.obs().await {
            Some(o) => {
                let name = self.platform.camera.list().into_iter().find(|c| c.id == camera_id).map(|c| c.name).unwrap_or_default();
                obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok().and_then(|s| obs_source_for_camera(&s, &name))
            }
            None => None,
        };
        let class = self.camera_class(&camera_id);
        let mains = self.mains();
        let mut state = self.camera_state(&camera_id).unwrap_or_default();
        let auto = step.auto && class.controllable();
        // Flicker: match anti-flicker to the local mains first.
        if auto && step.task == T::Flicker && state.powerline_hz.is_some() && state.powerline_hz != Some(mains.as_u8()) {
            let _ = self.apply_uvc(&camera_id, &state, |u| u.powerline_hz = Some(mains.as_u8())).await;
            state = self.camera_state(&camera_id).unwrap_or(state);
        }
        let me = self.clone();
        let task = step.task;
        tokio::spawn(async move {
            let mut coach = LiveCameraCoach::new(task, &state, class, mains);
            let mut n: u64 = 0;
            let mut tried_other_mains = false;
            while running.load(Ordering::SeqCst) {
                // No real picture (camera busy, black frames): never coach from it.
                let raw = match me.grab_checked(&camera_id, source.as_deref()).await {
                    Ok(r) => {
                        me.note_picture(None);
                        r
                    }
                    Err(reason) => {
                        me.note_picture(Some(reason));
                        tokio::time::sleep(me.coach_tick * 4).await;
                        continue;
                    }
                };
                let mut small = camera::downscale(&raw, ANALYSIS_WIDTH);
                small.timestamp_s = n as f64 / 4.0; // logical 4 fps
                let face = me.platform.camera.detect_face(&raw).map(|r| camera::scale_rect(r, (raw.width, raw.height), (small.width, small.height)));
                let mut tf = TaggedFrame::new(TestStep::Neutral, small.clone());
                if let Some(r) = face {
                    tf = tf.with_face(r);
                }
                let paper: Option<Rect> = (task == T::WhiteBalance).then(|| camera::paper_rect(&small));
                if let Some(p) = paper {
                    tf = tf.with_paper(p);
                }
                let mut u = coach.push(&tf);
                // Auto white balance: read the paper and set Kelvin ourselves.
                if auto && task == T::WhiteBalance && n % 8 == 7 && u.auto_adjusting {
                    let m = video::measure(&VideoTestCapture { frames: vec![tf.clone()] }, state.requested_fps.max(1.0));
                    let rec = video::recommend_settings(&m, &state, class, mains, &[]);
                    if let Some(k) = rec.uvc.as_ref().and_then(|x| x.wb_kelvin) {
                        if me.apply_uvc(&camera_id, &state, |x| {
                            x.auto_wb = false;
                            x.wb_kelvin = Some(k);
                        })
                        .await
                        .is_ok()
                        {
                            state = me.camera_state(&camera_id).unwrap_or(state);
                            coach.settings_changed(&state);
                        }
                    }
                }
                // Flicker still there after a few seconds: try the other mains setting once.
                if auto && task == T::Flicker && n == 24 && u.phase != Phase::Done && u.phase != Phase::Hold && !tried_other_mains {
                    tried_other_mains = true;
                    let other = if mains == video::MainsHz::Hz50 { 60 } else { 50 };
                    if me.apply_uvc(&camera_id, &state, |x| x.powerline_hz = Some(other)).await.is_ok() {
                        state = me.camera_state(&camera_id).unwrap_or(state);
                        coach.settings_changed(&state);
                        u.auto_adjusting = true;
                    }
                }
                me.emit_update(&u);
                if let Some(url) = camera::jpeg_data_url(&small) {
                    let overlay = CameraOverlay {
                        frame: url,
                        face,
                        paper,
                        clipped_mask: None,
                        arrows: coach.overlay_arrows(),
                        face_ok: coach.face_ok(),
                        width: small.width,
                        height: small.height,
                    };
                    me.emitter.emit(crate::types::events::COACH_CAMERA_OVERLAY, serde_json::to_value(overlay).unwrap_or(Value::Null));
                }
                n += 1;
                if coach.is_done() {
                    me.finish(&running);
                    break;
                }
                tokio::time::sleep(me.coach_tick * 2).await;
            }
        });
        Ok(())
    }

    /// Write UVC settings built from the current state with one change.
    async fn apply_uvc(&self, camera_id: &str, state: &CameraState, change: impl FnOnce(&mut UvcSettings)) -> R<()> {
        let key = format!("camera:{camera_id}");
        if !self.inner.lock().unwrap().coach_backed_up.contains(&key) {
            self.backup_before("Before auto-adjusting your camera", &[Target::Camera(camera_id.to_string())]).await?;
            self.inner.lock().unwrap().coach_backed_up.push(key);
        }
        let mut u = UvcSettings {
            auto_exposure: state.auto_exposure,
            ae_priority: state.ae_priority,
            exposure_log2: None,
            target_shutter_s: None,
            shutter_exact: false,
            gain: None,
            auto_wb: state.auto_wb,
            wb_kelvin: None,
            auto_focus: state.auto_focus,
            focus: None,
            sharpness: None,
            saturation: None,
            backlight_comp: None,
            powerline_hz: None,
            hdr: None,
        };
        change(&mut u);
        let cam = self.platform.camera.clone();
        let id = camera_id.to_string();
        tokio::task::spawn_blocking(move || cam.apply(&id, &u)).await.map_err(|e| e.to_string())?.map(|_| ())
    }
}

fn findings_capsule_condenser() -> coach_core::audio::findings::Capsule {
    coach_core::audio::findings::Capsule::Condenser
}

/// (app name, executable/URI to open) for a mic brand's control app.
fn vendor_app_for(mic: &str) -> (&'static str, &'static str) {
    let m = mic.to_ascii_lowercase();
    if m.contains("shure") {
        ("Shure MOTIV Mix", "shell:AppsFolder")
    } else if m.contains("yeti") || m.contains("blue") || m.contains("logitech") {
        ("Logitech G HUB", "lghub:")
    } else if m.contains("elgato") || m.contains("wave") {
        ("Elgato Wave Link", "shell:AppsFolder")
    } else if m.contains("rode") || m.contains("røde") {
        ("RØDE Central", "shell:AppsFolder")
    } else if m.contains("hyperx") || m.contains("quadcast") {
        ("HyperX NGENUITY", "shell:AppsFolder")
    } else {
        ("your mic's app", "shell:AppsFolder")
    }
}
