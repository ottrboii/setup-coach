//! The app's brain, independent of Tauri: every command in API.md is a method
//! here, so it can be tested with the mock platform and a fake OBS.

use crate::camera::{self, ANALYSIS_WIDTH};
use crate::obs::{self, client::ObsClient, files, ObsApi};
use crate::platform::{Platform, StreamHandle};
use crate::types::*;
use coach_core::audio::{
    self,
    findings::{Address as MAddr, Capsule as MCap, Connection as MConn, MicContext},
    process, AudioBuffer, MicReport, MicTest, ProcessingChain, Segment, SegmentKind,
};
use coach_core::health::{post_stream_report, HealthMonitor, MonitorConfig, ReportConfig, StatsSample, StreamReport};
use coach_core::profiles::{self as prof, ConnectionHint, DeviceIdentity, ProfileDb};
use coach_core::system::{self, Action, MonitoringType, ObsState, SystemSnapshot};
use coach_core::video::{self, CameraClass, CameraSettings, CameraState, TaggedFrame, TestStep, VideoTestCapture};
use coach_core::{Area, Constraint};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod ab;
mod camera_busy;
mod coaching;
mod micchoice;
mod vault_ops;
pub use ab::{match_gains, read_clip, slug, TakeMeta};
pub use coaching::{NoWindow, RecordingWindow, WindowControl};
pub use vault_ops::Target;

/// Sends events to the UI (Tauri in the app, a collector in tests).
pub trait Emitter: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Emitter that keeps events in memory (tests, headless use).
#[derive(Default)]
pub struct CollectingEmitter {
    pub events: Mutex<Vec<(String, Value)>>,
}

impl Emitter for CollectingEmitter {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
}

impl CollectingEmitter {
    pub fn count(&self, event: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|e| e.0 == event).count()
    }
}

/// Live recording shared with the audio thread.
struct RecBuf {
    sample_rate: u32,
    channels: Vec<Vec<f32>>,
    marks: Vec<(SegmentKind, Edge, f64)>,
    acc_sq: f64,
    acc_peak: f32,
    acc_n: usize,
}

impl RecBuf {
    fn len(&self) -> usize {
        self.channels.first().map(|c| c.len()).unwrap_or(0)
    }
}

struct CameraSession {
    camera_id: String,
    obs_source: Option<String>,
    frames: Vec<TaggedFrame>,
    running: Arc<AtomicBool>,
    started: Instant,
    requested_fps: f64,
}

struct HealthSession {
    running: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<StatsSample>>>,
    report_cfg: ReportConfig,
}

#[derive(Default)]
struct Inner {
    recording: Option<Arc<Mutex<RecBuf>>>,
    capture: Option<Box<dyn StreamHandle>>,
    last_test: Option<MicTest>,
    last_report: Option<MicReport>,
    last_context: Option<MicContext>,
    last_constraints: Vec<Constraint>,
    last_device: Option<String>,
    applied_chain: Option<ProcessingChain>,
    applied_input: Option<String>,
    preview: Option<Box<dyn StreamHandle>>,
    plan: Vec<PlannedActionView>,
    last_snapshot: Option<SystemSnapshot>,
    camera: Option<CameraSession>,
    last_camera: Option<(String, CameraReport)>,
    applied_camera: Option<(String, CameraSettings)>,
    health: Option<HealthSession>,
    coach: Option<coaching::CoachSession>,
    /// Devices we changed something for in this session (their next test
    /// isn't "original" any more).
    applied_devices: std::collections::HashSet<String>,
    /// Devices whose original recording was saved in this session.
    originals_saved: std::collections::HashSet<String>,
    /// Vault keys already backed up during the running coach step (auto gain
    /// writes many times; one backup per step).
    coach_backed_up: Vec<String>,
    /// OBS video source we released so the camera is free (put back later).
    freed_camera: Option<String>,
    /// Last `camera-picture` event sent: true = live.
    picture_live: Option<bool>,
}

pub struct AppCore {
    pub platform: Platform,
    pub profiles: ProfileDb,
    pub data_dir: PathBuf,
    pub emitter: Arc<dyn Emitter>,
    /// Interval between stream-health polls.
    pub health_interval: Duration,
    /// Interval between camera frames during a test step.
    pub frame_interval: Duration,
    /// How long one camera read may take before we call the camera unresponsive.
    pub grab_timeout: Duration,
    /// Interval between live-coaching ticks (~8 updates/s).
    pub coach_tick: Duration,
    /// Look for the GoXLR Utility daemon on localhost (off in tests).
    pub goxlr_autodetect: AtomicBool,
    obs: tokio::sync::Mutex<Option<Arc<dyn ObsApi>>>,
    goxlr: tokio::sync::Mutex<Option<Arc<dyn crate::goxlr::GoXlrApi>>>,
    window: Mutex<Arc<dyn WindowControl>>,
    inner: Mutex<Inner>,
}

type R<T> = Result<T, String>;

impl AppCore {
    pub fn new(platform: Platform, data_dir: PathBuf, emitter: Arc<dyn Emitter>) -> Self {
        AppCore {
            platform,
            profiles: ProfileDb::from_embedded().expect("embedded device profiles are valid"),
            data_dir,
            emitter,
            health_interval: Duration::from_secs(2),
            frame_interval: Duration::from_millis(250),
            grab_timeout: Duration::from_secs(6),
            coach_tick: Duration::from_millis(125),
            goxlr_autodetect: AtomicBool::new(cfg!(windows)),
            obs: tokio::sync::Mutex::new(None),
            goxlr: tokio::sync::Mutex::new(None),
            window: Mutex::new(Arc::new(NoWindow)),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Use an already-connected OBS (tests, or after `connect_obs`).
    pub async fn set_obs(&self, obs: Arc<dyn ObsApi>) {
        *self.obs.lock().await = Some(obs);
    }

    /// The live OBS connection, if any (dropped once OBS went away).
    pub async fn obs(&self) -> Option<Arc<dyn ObsApi>> {
        let mut g = self.obs.lock().await;
        if g.as_ref().is_some_and(|o| !o.is_alive()) {
            *g = None;
        }
        g.clone()
    }

    fn obs_dir(&self) -> Option<PathBuf> {
        self.platform.system.obs_config_dir()
    }

    // ------------------------------------------------------------ environment

    pub async fn connect_obs(&self, password: Option<String>) -> ObsStatus {
        let running = self.platform.system.obs_running();
        let cfg = self.obs_dir().and_then(|d| obs::read_websocket_config(&d));
        let enabled = cfg.as_ref().map(|c| c.server_enabled);
        let port = cfg.as_ref().map(|c| c.server_port).unwrap_or(4455);
        let pw = password.or_else(|| cfg.as_ref().map(|c| c.server_password.clone()));
        if !running {
            return ObsStatus { running, connected: false, version: None, websocket_enabled: enabled, error: Some("OBS isn't running. Open OBS and press Connect.".into()) };
        }
        if enabled == Some(false) {
            return ObsStatus {
                running,
                connected: false,
                version: None,
                websocket_enabled: enabled,
                error: Some("Turn on OBS's WebSocket server: OBS → Tools → WebSocket Server Settings → Enable WebSocket server.".into()),
            };
        }
        match ObsClient::connect(port, pw.as_deref()).await {
            Ok(c) => {
                let c: Arc<dyn ObsApi> = Arc::new(c);
                let version = c.request("GetVersion", Value::Null).await.ok().and_then(|v| v["obsVersion"].as_str().map(str::to_string));
                self.set_obs(c).await;
                ObsStatus { running, connected: true, version, websocket_enabled: enabled.or(Some(true)), error: None }
            }
            Err(e) => ObsStatus { running, connected: false, version: None, websocket_enabled: enabled, error: Some(e) },
        }
    }

    async fn obs_status(&self) -> ObsStatus {
        if let Some(o) = self.obs().await {
            let version = o.request("GetVersion", Value::Null).await.ok().and_then(|v| v["obsVersion"].as_str().map(str::to_string));
            if version.is_some() {
                return ObsStatus { running: true, connected: true, version, websocket_enabled: Some(true), error: None };
            }
        }
        self.connect_obs(None).await
    }

    pub async fn get_environment(&self) -> Environment {
        let obs_status = self.obs_status().await;
        let obs_state = match self.obs().await {
            Some(o) => obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok(),
            None => None,
        };
        let setup = self.load_setup();
        let mics = self
            .platform
            .audio
            .list_inputs()
            .into_iter()
            .map(|d| {
                let choice = setup.as_ref().and_then(|s| s.mic_behind.get(&d.id)).cloned();
                AudioDeviceInfo {
                    profile: self.mic_view_for(&d.name, d.usb, choice.as_ref()),
                    needs_mic_choice: choice.is_none() && self.asks_for_mic(&d.name, d.usb),
                    mic_choice: choice,
                    obs_input: obs_state.as_ref().and_then(|s| obs_input_for_device(s, &d.id, d.is_default)),
                    id: d.id,
                    name: d.name,
                    is_default: d.is_default,
                    is_default_comms: d.is_default_comms,
                    channels: d.channels,
                    sample_rate: d.sample_rate,
                }
            })
            .collect();
        let cameras = self
            .platform
            .camera
            .list()
            .into_iter()
            .map(|c| CameraDeviceInfo {
                obs_source: obs_state.as_ref().and_then(|s| obs_source_for_camera(s, &c.name)),
                profile: self.match_camera_view(&c.name, c.usb),
                id: c.id,
                name: c.name,
            })
            .collect();
        Environment {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            windows_build: self.platform.system.windows_build(),
            obs: obs_status,
            mics,
            cameras,
            // A setup.json that only holds mic answers is not a "saved setup".
            saved_profile: setup.filter(|p| !p.saved_at.is_empty()),
        }
    }

    fn identity(name: &str, usb: Option<(u16, u16)>) -> DeviceIdentity {
        let mut id = DeviceIdentity::named(name);
        if let Some((v, p)) = usb {
            id = id.with_usb(v, p);
        }
        if name.to_ascii_lowercase().contains("hands-free") {
            id = id.with_hint(ConnectionHint::Bluetooth);
        }
        id
    }

    pub fn match_mic_view(&self, name: &str, usb: Option<(u16, u16)>) -> Option<ProfileMatch> {
        let m = self.profiles.match_input(&Self::identity(name, usb))?;
        let p = m.mic.profile;
        Some(ProfileMatch {
            kind: "mic".into(),
            profile_id: p.id.clone(),
            display_name: format!("{} {}", p.brand, p.model).trim().to_string(),
            confidence: m.mic.confidence,
            verified: p.verified,
            questions: m.mic.needs_questions.clone(),
            interface: m.interface.map(|i| {
                Box::new(ProfileMatch {
                    kind: "interface".into(),
                    profile_id: i.profile.id.clone(),
                    display_name: format!("{} {}", i.profile.brand, i.profile.model),
                    confidence: i.confidence,
                    verified: i.profile.verified,
                    questions: i.needs_questions.clone(),
                    interface: None,
                    profile: serde_json::to_value(i.profile).unwrap_or(Value::Null),
                    chosen: false,
                })
            }),
            profile: serde_json::to_value(p).unwrap_or(Value::Null),
            chosen: false,
        })
    }

    pub fn match_camera_view(&self, name: &str, usb: Option<(u16, u16)>) -> Option<ProfileMatch> {
        let m = self.profiles.match_camera(&Self::identity(name, usb))?;
        Some(ProfileMatch {
            kind: "camera".into(),
            profile_id: m.profile.id.clone(),
            display_name: format!("{} {}", m.profile.brand, m.profile.model).trim().to_string(),
            confidence: m.confidence,
            verified: m.profile.verified,
            questions: m.needs_questions.clone(),
            interface: None,
            profile: serde_json::to_value(m.profile).unwrap_or(Value::Null),
            chosen: false,
        })
    }

    // ------------------------------------------------------------ quick checks

    async fn snapshot(&self) -> SystemSnapshot {
        let obs_state = match self.obs().await {
            Some(o) => obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok(),
            None => None,
        };
        let sys = &self.platform.system;
        let mut camera = sys.camera_system();
        if let Some(c) = camera.as_mut() {
            c.vendor_apps_running = sys.vendor_camera_apps();
        }
        let mut processing = sys.processing_apps();
        processing.goxlr_utility_running |= self.goxlr().await.is_some();
        let mut snap = SystemSnapshot {
            windows: sys.windows_audio(),
            obs: obs_state,
            camera,
            processing,
            hardware: sys.hardware(),
            network: sys.network(),
            ..Default::default()
        };
        let inner = self.inner.lock().unwrap();
        if let Some(r) = &inner.last_report {
            snap.mic.measured_noise_floor_dbfs = Some(r.metrics.noise_floor_dbfs as f32);
            snap.mic.measured_quiet_speech_dbfs = Some(r.metrics.quiet_speech_dbfs as f32);
            snap.mic.single_live_channel = r.metrics.dead_channel.is_some();
        }
        // Interface input 1 arriving left-only, known from the profile.
        if let Some(ep) = snap.mic_endpoint() {
            if let Some(m) = self.profiles.match_input(&Self::identity(&ep.name, None)) {
                if m.interface.as_ref().is_some_and(|i| i.profile.input1_left_only) {
                    snap.mic.single_live_channel = true;
                }
                snap.mic.windows_slider_is_hardware_gain = m.mic.profile.windows_slider_is_hw_gain.unwrap_or(false);
                snap.mic.onboard_dsp_active = m.mic.profile.onboard_dsp.iter().map(|d| d.name.clone()).collect();
            }
        }
        snap
    }

    /// Append one line to `<data dir>\logs\ui.log` (kept under ~512 KB).
    pub fn log_ui(&self, level: &str, source: &str, message: &str) {
        use std::io::Write;
        let dir = self.data_dir.join("logs");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("ui.log");
        if std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&path, dir.join("ui.old.log"));
        }
        let clean = |s: &str, n: usize| s.chars().take(n).collect::<String>().replace(['\r', '\n'], " ");
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{secs} [{}] {}: {}", clean(level, 10), clean(source, 40), clean(message, 2000));
        }
    }

    pub async fn run_quick_checks(&self) -> QuickChecks {
        let snap = self.snapshot().await;
        let mut findings = system::audit(&snap, &[]);
        let prev = self.inner.lock().unwrap().last_snapshot.clone();
        if let Some(prev) = prev {
            for f in system::snapshot_diff(&prev, &snap) {
                if !findings.iter().any(|x| x.id == f.id) {
                    findings.push(f);
                }
            }
        }
        coach_core::finding::rank(&mut findings);
        let obs_running = self.platform.system.obs_running();
        // Never offer a one-click fix that is certain to fail (no admin rights):
        // those findings become "you do it" steps up front.
        let (fixable, blocked): (Vec<_>, Vec<_>) =
            system::quick_fixes(&snap, &findings).into_iter().partition(|p| !needs_admin(&p.action) || self.platform.system.can_apply(&p.action));
        let blocked_ids: Vec<String> = blocked.iter().flat_map(|p| p.resolves.clone()).collect();
        let still_fixable: Vec<String> = fixable.iter().flat_map(|p| p.resolves.clone()).collect();
        downgrade_unfixable(&mut findings, blocked_ids.iter().filter(|id| !still_fixable.contains(id)));
        let actions: Vec<PlannedActionView> = fixable
            .into_iter()
            .enumerate()
            .map(|(i, p)| PlannedActionView {
                id: format!("act-{i}"),
                label: label_for(&p.action),
                needs_admin: needs_admin(&p.action),
                needs_obs_closed: obs_running && needs_obs_closed(&p.action),
                resolves: p.resolves,
                action: p.action,
            })
            .collect();
        let score = [Area::System, Area::Mic, Area::Camera, Area::Stream].iter().map(|a| coach_core::score::score_area(*a, &findings)).collect();
        let mut inner = self.inner.lock().unwrap();
        inner.plan = actions.clone();
        inner.last_snapshot = Some(snap);
        QuickChecks { findings, actions, score }
    }

    pub async fn apply_actions(&self, ids: &[String]) -> ApplyResult {
        let plan = self.inner.lock().unwrap().plan.clone();
        let mut res = ApplyResult::default();
        let mut targets = Vec::new();
        for p in plan.iter().filter(|p| ids.contains(&p.id)) {
            targets.extend(self.targets_for_action(&p.action).await);
        }
        let label = if ids.len() == 1 {
            plan.iter().find(|p| p.id == ids[0]).map(|p| format!("Before: {}", p.label)).unwrap_or_else(|| "Before quick fixes".into())
        } else {
            "Before quick fixes".into()
        };
        if let Err(e) = self.backup_before(&label, &targets).await {
            for id in ids {
                res.fail(id, e.clone());
            }
            return res;
        }
        for id in ids {
            let Some(p) = plan.iter().find(|p| &p.id == id) else {
                res.fail(id, "That fix is out of date. Run the checks again.");
                continue;
            };
            let mut outcome = self.execute(&p.action).await;
            // Never say "fixed" on the strength of a call returning Ok: for the
            // Windows audio effects, read the real state back.
            if outcome.is_ok() && fx_check_id(&p.action).is_some() && self.fx_really_off(&p.action) != Some(true) {
                outcome = Err("Windows still shows this as on, so we haven't counted it as fixed.".into());
            }
            match outcome {
                Ok(restart) => {
                    res.restart_obs_needed |= restart;
                    res.ok(id);
                    if let Some(ep) = action_endpoint(&p.action) {
                        self.inner.lock().unwrap().applied_devices.insert(ep);
                    }
                }
                Err(e) => match (fx_check_id(&p.action), action_endpoint(&p.action)) {
                    // Windows keeps these for the user: hand over a guided step
                    // that opens the exact page and watches for the change.
                    (Some(check), Some(ep)) => res.fail_guided(
                        id,
                        format!("{e} You can switch it off yourself in a few clicks: we'll open the right page and watch for the change."),
                        coaching::fx_guided_step(check, &ep),
                    ),
                    _ => res.fail(id, e),
                },
            }
        }
        res
    }

    /// Re-read Windows after an enhancements / Voice Clarity change: is it
    /// really off now? `None` = not one of those actions; `Some(false)` also
    /// when Windows can't be read (we only report success we have seen).
    fn fx_really_off(&self, action: &Action) -> Option<bool> {
        fx_check_id(action)?;
        let w = self.platform.system.windows_audio();
        let off = |ep: &str| -> bool {
            let Some(e) = w.as_ref().and_then(|w| w.endpoints.iter().find(|e| e.id == ep)) else { return false };
            match action {
                Action::DisableEnhancements { .. } => !e.enhancements_enabled,
                Action::DisableListen { .. } => !e.listen_to_device,
                Action::DisableExclusiveMode { .. } => !e.exclusive_mode_allowed,
                _ => !e.voice_clarity_enabled,
            }
        };
        Some(off(&action_endpoint(action)?))
    }

    /// Run one core `Action`. Ok(true) = OBS needs a restart to pick it up.
    pub async fn execute(&self, action: &Action) -> R<bool> {
        let obs_running = self.platform.system.obs_running();
        let need_obs = || async { self.obs().await.ok_or_else(|| "Connect to OBS first.".to_string()) };
        let close_obs = "Close OBS, then press Apply again — OBS only allows this change while it's closed.".to_string();
        let simple = |o: &Arc<dyn ObsApi>| {
            let o = o.clone();
            async move {
                o.request("GetProfileParameter", json!({ "parameterCategory": "Output", "parameterName": "Mode" }))
                    .await
                    .ok()
                    .and_then(|v| v["parameterValue"].as_str().map(str::to_string))
                    .map(|m| m != "Advanced")
                    .unwrap_or(true)
            }
        };
        let set_param = |o: Arc<dyn ObsApi>, cat: &'static str, name: &'static str, value: String| async move {
            o.request("SetProfileParameter", json!({ "parameterCategory": cat, "parameterName": name, "parameterValue": value })).await.map(|_| ())
        };
        match action {
            Action::SetObsInputMono { input } => {
                if obs_running {
                    return Err(close_obs);
                }
                let dir = self.obs_dir().ok_or("Couldn't find OBS's settings folder.")?;
                files::set_force_mono(&dir, input, true)?;
                Ok(false)
            }
            Action::SetObsSampleRate { hz } => {
                if let Some(o) = self.obs().await {
                    set_param(o, "Audio", "SampleRate", hz.to_string()).await?;
                    return Ok(true);
                }
                let dir = self.obs_dir().ok_or("Couldn't find OBS's settings folder.")?;
                files::set_sample_rate(&dir, *hz)?;
                Ok(false)
            }
            Action::EnableObsWebsocket => {
                if obs_running {
                    return Err("In OBS: Tools → WebSocket Server Settings → tick \"Enable WebSocket server\".".into());
                }
                files::enable_websocket(&self.obs_dir().ok_or("Couldn't find OBS's settings folder.")?)?;
                Ok(false)
            }
            Action::SetObsInputMuted { input, muted } => {
                need_obs().await?.request("SetInputMute", json!({ "inputName": input, "inputMuted": muted })).await.map(|_| false)
            }
            Action::RemoveObsFilter { input, filter } => {
                need_obs().await?.request("RemoveSourceFilter", json!({ "sourceName": input, "filterName": filter })).await.map(|_| false)
            }
            Action::ReorderObsFilters { input, order } => {
                let o = need_obs().await?;
                for (i, f) in order.iter().enumerate() {
                    o.request("SetSourceFilterIndex", json!({ "sourceName": input, "filterName": f, "filterIndex": i })).await?;
                }
                Ok(false)
            }
            Action::SetObsFilterSettings { input, filter, settings } => need_obs()
                .await?
                .request("SetSourceFilterSettings", json!({ "sourceName": input, "filterName": filter, "filterSettings": settings, "overlay": true }))
                .await
                .map(|_| false),
            Action::AddObsFilter { input, name, kind, settings } => need_obs()
                .await?
                .request("CreateSourceFilter", json!({ "sourceName": input, "filterName": name, "filterKind": kind, "filterSettings": settings }))
                .await
                .map(|_| false),
            Action::SetObsMonitoring { input, monitoring } => need_obs()
                .await?
                .request("SetInputAudioMonitorType", json!({ "inputName": input, "monitorType": obs::monitoring_to_obs(*monitoring) }))
                .await
                .map(|_| false),
            Action::PinObsMicDevice { input, device_id } => need_obs()
                .await?
                .request("SetInputSettings", json!({ "inputName": input, "inputSettings": { "device_id": device_id }, "overlay": true }))
                .await
                .map(|_| false),
            Action::SetObsVideoCaptureMode { input, width, height, fps, format } => {
                let mut s = json!({ "res_type": 1, "resolution": format!("{width}x{height}"), "frame_interval": (1e7 / (*fps as f64).max(1.0)).round() as i64 });
                if format.eq_ignore_ascii_case("mjpeg") {
                    s["video_format"] = json!(400);
                }
                need_obs().await?.request("SetInputSettings", json!({ "inputName": input, "inputSettings": s, "overlay": true })).await.map(|_| false)
            }
            Action::SetObsStreamEncoder { encoder_id } => {
                let o = need_obs().await?;
                if simple(&o).await {
                    let name = match encoder_id.as_str() {
                        e if e.contains("nvenc") => "nvenc",
                        e if e.contains("amf") => "amd",
                        e if e.contains("qsv") => "qsv",
                        _ => "x264",
                    };
                    set_param(o, "SimpleOutput", "StreamEncoder", name.into()).await.map(|_| false)
                } else {
                    set_param(o, "AdvOut", "Encoder", encoder_id.clone()).await.map(|_| false)
                }
            }
            Action::SetObsStreamBitrate { kbps } => {
                let o = need_obs().await?;
                if simple(&o).await {
                    set_param(o, "SimpleOutput", "VBitrate", kbps.to_string()).await.map(|_| false)
                } else {
                    Err(format!("In OBS: Settings → Output → Streaming → set Bitrate to {kbps} Kbps."))
                }
            }
            Action::SetObsRateControl { mode } => {
                let o = need_obs().await?;
                if simple(&o).await {
                    Ok(false) // Simple mode always streams CBR.
                } else {
                    let dir = self.obs_dir().ok_or_else(|| format!("In OBS: Settings → Output → Streaming → set Rate Control to {mode}."))?;
                    files::set_stream_encoder_field(&dir, "rate_control", json!(mode)).map_err(|e| format!("{e} In OBS: Settings → Output → Streaming → set Rate Control to {mode}."))?;
                    Ok(false)
                }
            }
            Action::SetObsKeyframeInterval { seconds } => {
                let o = need_obs().await?;
                if simple(&o).await {
                    Ok(false) // Simple mode uses a 2 s keyframe interval.
                } else {
                    let dir = self.obs_dir().ok_or_else(|| format!("In OBS: Settings → Output → Streaming → set Keyframe Interval to {seconds} s."))?;
                    files::set_stream_encoder_field(&dir, "keyint_sec", json!(seconds)).map_err(|e| format!("{e} In OBS: Settings → Output → Streaming → set Keyframe Interval to {seconds} s."))?;
                    Ok(false)
                }
            }
            Action::SetObsOutputResolution { width, height } => need_obs()
                .await?
                .request("SetVideoSettings", json!({ "outputWidth": width, "outputHeight": height }))
                .await
                .map(|_| false),
            Action::SetObsAudioBitrate { kbps } => {
                let o = need_obs().await?;
                if simple(&o).await {
                    set_param(o, "SimpleOutput", "ABitrate", kbps.to_string()).await.map(|_| false)
                } else {
                    set_param(o, "AdvOut", "Track1Bitrate", kbps.to_string()).await.map(|_| false)
                }
            }
            Action::SetObsRecordingFormat { format } => {
                let o = need_obs().await?;
                let cat = if simple(&o).await { "SimpleOutput" } else { "AdvOut" };
                set_param(o, cat, "RecFormat2", format.clone()).await.map(|_| false)
            }
            // Windows-side actions.
            // (Off the async threads: some of these wait for Windows' permission prompt.)
            other => {
                let (sys, a) = (self.platform.system.clone(), other.clone());
                tokio::task::spawn_blocking(move || sys.apply(&a)).await.map_err(|e| e.to_string())?.map(|_| false)
            }
        }
    }

    // ------------------------------------------------------------ mic test

    pub fn mic_context(&self, device_id: &str, answers: &MicAnswers) -> R<MicContext> {
        let dev = self.platform.audio.list_inputs().into_iter().find(|d| d.id == device_id).ok_or("That microphone isn't connected any more.")?;
        let m = self.profiles.match_input(&Self::identity(&dev.name, dev.usb));
        // The mic the user named wins over what the Windows device name suggests.
        let mic = answers
            .mic_model_id
            .as_deref()
            .and_then(|id| self.find_mic(id))
            .or_else(|| self.chosen_profile(device_id))
            .or_else(|| m.as_ref().map(|m| m.mic.profile));
        let iface = m.as_ref().and_then(|m| m.interface.as_ref().map(|i| i.profile));
        let mut ctx = MicContext::default();
        if let Some(p) = mic {
            ctx.mic_name = Some(format!("{} {}", p.brand, p.model).trim().to_string());
            ctx.capsule = Some(match p.capsule {
                prof::Capsule::Dynamic => MCap::Dynamic,
                prof::Capsule::Condenser => MCap::Condenser,
                prof::Capsule::Electret => MCap::Electret,
            });
            ctx.address = match &p.address {
                prof::Address::Side => Some(MAddr::Side),
                prof::Address::End | prof::Address::Top => Some(MAddr::End),
                prof::Address::MultiPattern { facing, .. } => Some(if matches!(facing, prof::Facing::Side) { MAddr::Side } else { MAddr::End }),
                prof::Address::Any => None,
            };
            ctx.connection = Some(match &p.connection {
                prof::MicConnection::Usb => MConn::Usb,
                prof::MicConnection::Xlr => MConn::Xlr,
                prof::MicConnection::UsbXlrHybrid => if iface.is_some() { MConn::Xlr } else { MConn::Usb },
                prof::MicConnection::Headset { path: prof::HeadsetPath::BluetoothHfp } => MConn::Bluetooth,
                prof::MicConnection::Headset { .. } => MConn::Headset,
                prof::MicConnection::Wireless => MConn::Wireless,
                prof::MicConnection::Builtin => MConn::BuiltIn,
            });
            ctx.gain_needed_db = p.gain_needed_db.map(f64::from);
            ctx.distance_cm = Some((p.distance_cm.min as f64, p.distance_cm.max as f64));
            if p.onboard_dsp.iter().any(|d| matches!(d.kind, prof::DspKind::AutoLevel | prof::DspKind::AutoGain)) {
                ctx.onboard_auto_level = None; // unknown whether it's on: let the test decide
            } else {
                ctx.onboard_auto_level = Some(false);
            }
        }
        if dev.name.to_ascii_lowercase().contains("hands-free") {
            ctx.connection = Some(MConn::Bluetooth);
        }
        ctx.interface_max_gain_db = iface.and_then(|i| i.max_gain_db).map(f64::from);
        if answers.booster == Some(true) {
            ctx.booster_db = Some(self.profiles.boosters.first().map(|b| b.gain_db as f64).unwrap_or(25.0));
        }
        if answers.gain_at_max.is_some() {
            ctx.gain_at_max = answers.gain_at_max;
        }
        if answers.onboard_auto_level.is_some() {
            ctx.onboard_auto_level = answers.onboard_auto_level;
        }
        Ok(ctx)
    }

    pub fn start_mic_test(&self, device_id: &str) -> R<()> {
        self.stop_capture();
        let rec = Arc::new(Mutex::new(RecBuf { sample_rate: 48_000, channels: Vec::new(), marks: Vec::new(), acc_sq: 0.0, acc_peak: 0.0, acc_n: 0 }));
        let sink_rec = rec.clone();
        let emitter = self.emitter.clone();
        let sink = Box::new(move |data: &[f32], ch: u16, sr: u32| {
            let ch = ch.max(1) as usize;
            let mut r = sink_rec.lock().unwrap();
            if r.channels.len() != ch {
                r.channels = vec![Vec::new(); ch];
                r.sample_rate = sr;
            }
            for frame in data.chunks(ch) {
                let mut mono = 0.0f32;
                for (c, v) in frame.iter().enumerate() {
                    r.channels[c].push(*v);
                    mono = mono.max(v.abs());
                }
                r.acc_sq += (mono as f64) * (mono as f64);
                r.acc_peak = r.acc_peak.max(mono);
                r.acc_n += 1;
                if r.acc_n >= (sr / 20) as usize {
                    let rms = (r.acc_sq / r.acc_n as f64).sqrt();
                    let lvl = MicLevel {
                        rms_db: (20.0 * rms.max(1e-9).log10() * 10.0).round() / 10.0,
                        peak_db: (20.0 * (r.acc_peak as f64).max(1e-9).log10() * 10.0).round() / 10.0,
                        clipped: r.acc_peak >= 0.999,
                    };
                    r.acc_sq = 0.0;
                    r.acc_peak = 0.0;
                    r.acc_n = 0;
                    emitter.emit(crate::types::events::MIC_LEVEL, serde_json::to_value(lvl).unwrap_or(Value::Null));
                }
            }
        });
        let handle = self.platform.audio.start_capture(device_id, sink)?;
        let mut inner = self.inner.lock().unwrap();
        inner.recording = Some(rec);
        inner.capture = Some(handle);
        inner.last_device = Some(device_id.to_string());
        Ok(())
    }

    fn stop_capture(&self) {
        let h = self.inner.lock().unwrap().capture.take();
        if let Some(h) = h {
            h.stop();
        }
    }

    /// Mark the start/end of a guided step at the current recording position.
    pub fn mark_step(&self, kind: SegmentKind, edge: Edge) -> R<()> {
        let inner = self.inner.lock().unwrap();
        let rec = inner.recording.as_ref().ok_or("Start the mic test first.")?;
        let mut r = rec.lock().unwrap();
        let t = r.len() as f64 / r.sample_rate.max(1) as f64;
        r.marks.push((kind, edge, t));
        Ok(())
    }

    /// Mark at an explicit time (tests and replayed recordings).
    pub fn mark_step_at(&self, kind: SegmentKind, edge: Edge, t_s: f64) -> R<()> {
        let inner = self.inner.lock().unwrap();
        let rec = inner.recording.as_ref().ok_or("Start the mic test first.")?;
        rec.lock().unwrap().marks.push((kind, edge, t_s));
        Ok(())
    }

    pub fn finish_mic_test(&self, ctx: MicContext, constraints: Vec<Constraint>) -> R<MicReport> {
        self.stop_capture();
        let rec = self.inner.lock().unwrap().recording.take().ok_or("Start the mic test first.")?;
        let r = rec.lock().unwrap();
        if r.len() < r.sample_rate as usize {
            return Err("The recording is too short. Please run the test again.".into());
        }
        let segments = segments_from_marks(&r.marks, r.len() as f64 / r.sample_rate as f64);
        if !segments.iter().any(|s| s.kind == SegmentKind::Read) {
            return Err("We didn't get the reading step. Please run the test again.".into());
        }
        let test = MicTest { audio: AudioBuffer { sample_rate: r.sample_rate, channels: r.channels.clone() }, segments };
        drop(r);
        let report = audio::scan(&test, &ctx, &constraints);
        let device = self.inner.lock().unwrap().last_device.clone();
        if let Some(d) = device {
            // Best effort: a full disk mustn't stop the test itself.
            let _ = self.maybe_save_original(&d, &test, &report);
        }
        let mut inner = self.inner.lock().unwrap();
        inner.last_test = Some(test);
        inner.last_report = Some(report.clone());
        inner.last_context = Some(ctx);
        inner.last_constraints = constraints;
        Ok(report)
    }

    pub fn rescan_mic(&self, ctx: MicContext, constraints: Vec<Constraint>) -> R<MicReport> {
        let test = self.inner.lock().unwrap().last_test.clone().ok_or("Run the mic test first.")?;
        let report = audio::scan(&test, &ctx, &constraints);
        let mut inner = self.inner.lock().unwrap();
        inner.last_report = Some(report.clone());
        inner.last_context = Some(ctx);
        inner.last_constraints = constraints;
        Ok(report)
    }

    pub fn write_chain_json(&self, chain: &ProcessingChain) -> R<PathBuf> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| format!("Couldn't save settings ({e})."))?;
        let p = self.data_dir.join("chain.json");
        let tmp = self.data_dir.join("chain.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(chain).map_err(|e| e.to_string())?).map_err(|e| format!("Couldn't save settings ({e})."))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("Couldn't save settings ({e})."))?;
        Ok(p)
    }

    pub async fn apply_mic_chain(&self, chain: ProcessingChain, obs_input: &str) -> ApplyResult {
        let mut res = ApplyResult::default();
        let obs_input = self.resolve_obs_input(obs_input).await;
        let obs_input = obs_input.as_str();
        let mut targets = vec![Target::ChainJson, Target::ObsFilters(obs_input.to_string())];
        if chain.mono_downmix {
            targets.extend(self.targets_for_action(&Action::SetObsInputMono { input: obs_input.to_string() }).await);
        }
        let device = self.inner.lock().unwrap().last_device.clone();
        let mic = device.as_deref().and_then(|d| self.device_name(d)).map(|n| plain_device_name(&n)).unwrap_or_else(|| "mic".into());
        if let Err(e) = self.backup_before(&format!("Before setting up your {mic}"), &targets).await {
            res.fail("backup", e);
            return res;
        }
        if let Some(d) = device {
            self.inner.lock().unwrap().applied_devices.insert(d);
        }
        match self.write_chain_json(&chain) {
            Ok(_) => res.ok("chain_json"),
            Err(e) => res.fail("chain_json", e),
        }
        match self.obs().await {
            Some(o) => {
                let vst = crate::paths::vst3_path();
                let (ok, fail) = obs::apply_chain(o.as_ref(), obs_input, &chain, vst.as_deref()).await;
                let vst_failed = fail.iter().any(|f| f.0 == "vst3");
                res.applied.extend(ok);
                for (id, e) in fail {
                    res.fail(id, e);
                }
                // Without our plugin, OBS's own mono switch must do the downmix.
                if chain.mono_downmix && vst_failed {
                    match self.execute(&Action::SetObsInputMono { input: obs_input.to_string() }).await {
                        Ok(_) => res.ok("mono"),
                        Err(e) => {
                            res.restart_obs_needed = true;
                            res.fail("mono", e)
                        }
                    }
                }
            }
            None => res.fail("obs", "Connect to OBS to apply the settings there. Your settings are saved."),
        }
        let mut inner = self.inner.lock().unwrap();
        inner.applied_chain = Some(chain);
        inner.applied_input = Some(obs_input.to_string());
        res
    }

    /// The OBS mic input to use: `name` if OBS has it, otherwise the input
    /// capturing the device of the last mic test (or OBS's main mic).
    pub async fn resolve_obs_input(&self, name: &str) -> String {
        let Some(o) = self.obs().await else { return name.to_string() };
        let Ok(st) = obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await else { return name.to_string() };
        if !name.is_empty() && st.audio_inputs.iter().any(|i| i.name == name) {
            return name.to_string();
        }
        let dev = self.inner.lock().unwrap().last_device.clone();
        let dev = dev.and_then(|id| self.platform.audio.list_inputs().into_iter().find(|d| d.id == id));
        dev.and_then(|d| obs_input_for_device(&st, &d.id, d.is_default))
            .or_else(|| st.main_mic().map(|i| i.name.clone()))
            .unwrap_or_else(|| name.to_string())
    }

    /// Render the last recording through the applied (or recommended) chain
    /// and re-analyse it: the "did it work?" check.
    pub fn verify_mic(&self) -> R<VerifyResult> {
        let inner = self.inner.lock().unwrap();
        let test = inner.last_test.clone().ok_or("Run the mic test first.")?;
        let before = inner.last_report.clone().ok_or("Run the mic test first.")?;
        let chain = inner.applied_chain.clone().unwrap_or_else(|| before.chain.clone());
        let ctx = inner.last_context.clone().unwrap_or_default();
        let constraints = inner.last_constraints.clone();
        drop(inner);
        let after = audio::scan(&process::render_test(&test, &chain), &ctx, &constraints);
        Ok(VerifyResult { before, after })
    }

    /// The read + loud steps of the last recording, as heard before/after.
    pub fn preview_buffer(&self, which: PreviewWhich, phone: bool) -> R<AudioBuffer> {
        if matches!(which, PreviewWhich::Original | PreviewWhich::Final) {
            let buf = self.ab_preview(which == PreviewWhich::Final)?;
            return Ok(if phone { process::phone_speaker(&buf) } else { buf });
        }
        let inner = self.inner.lock().unwrap();
        let test = inner.last_test.as_ref().ok_or("Run the mic test first.")?;
        let chain = inner.applied_chain.clone().or_else(|| inner.last_report.as_ref().map(|r| r.chain.clone())).ok_or("Run the mic test first.")?;
        let mut channels = vec![Vec::new(); test.audio.channels.len()];
        for seg in test.segments.iter().filter(|s| matches!(s.kind, SegmentKind::Read | SegmentKind::Loud)) {
            for (c, ch) in test.audio.channels.iter().enumerate() {
                channels[c].extend_from_slice(&ch[test.range(seg)]);
            }
        }
        let buf = AudioBuffer { sample_rate: test.audio.sample_rate, channels };
        let mut out = match which {
            PreviewWhich::Before => {
                // Play the raw take in both ears so a left-only input is audible as-is.
                buf
            }
            PreviewWhich::After | PreviewWhich::Original | PreviewWhich::Final => process::render(&buf, &chain),
        };
        if phone {
            out = process::phone_speaker(&out);
        }
        Ok(out)
    }

    pub fn play_preview(&self, which: PreviewWhich, phone: bool) -> R<()> {
        self.stop_preview();
        let buf = self.preview_buffer(which, phone)?;
        let h = self.platform.audio.play(buf)?;
        self.inner.lock().unwrap().preview = Some(h);
        Ok(())
    }

    pub fn stop_preview(&self) {
        let h = self.inner.lock().unwrap().preview.take();
        if let Some(h) = h {
            h.stop();
        }
    }

    // ------------------------------------------------------------ camera

    pub fn camera_class(&self, camera_id: &str) -> CameraClass {
        let dev = self.platform.camera.list().into_iter().find(|c| c.id == camera_id);
        let class = dev.and_then(|d| self.profiles.match_camera(&Self::identity(&d.name, d.usb)).map(|m| m.profile.class));
        match class {
            Some(prof::CameraClass::CaptureCard) => CameraClass::CaptureCard,
            Some(prof::CameraClass::Phone) | Some(prof::CameraClass::Virtual) => CameraClass::PhoneOrVirtual,
            Some(prof::CameraClass::LimitedUvc) => CameraClass::LimitedUvc,
            _ => CameraClass::FullUvc,
        }
    }

    pub fn camera_state(&self, camera_id: &str) -> R<CameraState> {
        if !self.camera_class(camera_id).controllable() {
            return Ok(CameraState::default());
        }
        self.platform.camera.read_state(camera_id)
    }

    pub async fn start_camera_test(self: &Arc<Self>, camera_id: &str) -> Result<(), CameraError> {
        self.stop_camera_preview();
        let source = self.camera_obs_source(camera_id).await;
        // Make sure there is a real picture before starting (a camera another
        // app holds gives black frames or nothing).
        if let Err(reason) = self.grab_checked(camera_id, source.as_deref()).await {
            self.note_picture(Some(reason));
            return Err(CameraError::no_picture(reason));
        }
        self.note_picture(None);
        let requested_fps = self.camera_state(camera_id).map(|s| s.requested_fps).unwrap_or(30.0);
        let running = Arc::new(AtomicBool::new(true));
        {
            let mut inner = self.inner.lock().unwrap();
            inner.camera = Some(CameraSession {
                camera_id: camera_id.to_string(),
                obs_source: source.clone(),
                frames: Vec::new(),
                running: running.clone(),
                started: Instant::now(),
                requested_fps: if requested_fps > 0.0 { requested_fps } else { 30.0 },
            });
        }
        let me = self.clone();
        let id = camera_id.to_string();
        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                match me.grab_checked(&id, source.as_deref()).await {
                    Ok(f) => {
                        me.note_picture(None);
                        if let Some(url) = camera::jpeg_data_url(&camera::downscale(&f, 480)) {
                            me.emitter.emit(crate::types::events::CAMERA_PREVIEW, json!({ "image": url, "paper_guide": camera::PAPER_GUIDE }));
                        }
                    }
                    Err(reason) => me.note_picture(Some(reason)),
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });
        Ok(())
    }

    fn stop_camera_preview(&self) {
        if let Some(s) = self.inner.lock().unwrap().camera.as_ref() {
            s.running.store(false, Ordering::SeqCst);
        }
    }

    pub async fn capture_camera_step(&self, step: TestStep, seconds: f64) -> Result<CaptureStepResult, CameraError> {
        let (id, source, started, fps) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.camera.as_ref().ok_or("Start the camera test first.")?;
            (s.camera_id.clone(), s.obs_source.clone(), s.started, s.requested_fps)
        };
        let n = ((seconds * 4.0).round() as usize).max(3);
        let mut frames = Vec::with_capacity(n);
        let mut failed = None;
        let mut faces = 0;
        let base_index = self.inner.lock().unwrap().camera.as_ref().map(|s| s.frames.len()).unwrap_or(0);
        for k in 0..n {
            let raw = match self.grab_checked(&id, source.as_deref()).await {
                Ok(r) => r,
                Err(reason) => {
                    // A dropped frame is fine; a step with no picture at all is not.
                    failed = Some(reason);
                    if k + 1 < n {
                        tokio::time::sleep(self.frame_interval).await;
                    }
                    continue;
                }
            };
            let small = camera::downscale(&raw, ANALYSIS_WIDTH);
            let face = self.platform.camera.detect_face(&raw).map(|r| camera::scale_rect(r, (raw.width, raw.height), (small.width, small.height)));
            let mut f = small;
            // Screenshot sampling can't measure the camera's real frame rate,
            // so frames are stamped at the requested rate (see API.md notes).
            f.timestamp_s = (base_index + k) as f64 / fps;
            let _ = started;
            let mut tf = TaggedFrame::new(step, f);
            if let Some(r) = face {
                faces += 1;
                tf = tf.with_face(r);
            }
            if step == TestStep::HoldPaper {
                let p = camera::paper_rect(&tf.frame);
                tf = tf.with_paper(p);
            }
            frames.push(tf);
            if k + 1 < n {
                tokio::time::sleep(self.frame_interval).await;
            }
        }
        let count = frames.len();
        if count == 0 {
            let reason = failed.unwrap_or(PictureReason::CantOpen);
            self.note_picture(Some(reason));
            return Err(CameraError::no_picture(reason));
        }
        let mut inner = self.inner.lock().unwrap();
        let s = inner.camera.as_mut().ok_or("The camera test was stopped.")?;
        s.frames.extend(frames);
        Ok(CaptureStepResult { frames: count, faces_found: faces })
    }

    pub fn finish_camera_test(&self, constraints: Vec<Constraint>) -> Result<CameraReport, CameraError> {
        // Frames stay in the session so the UI can call this again with new
        // constraints ("I can't add a light") without re-capturing.
        self.stop_camera_preview();
        let (camera_id, frames) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.camera.as_ref().ok_or("Start the camera test first.")?;
            (s.camera_id.clone(), s.frames.clone())
        };
        if frames.is_empty() {
            return Err("We didn't capture any pictures. Please run the camera test again.".into());
        }
        let session_camera = camera_id.clone();
        let class = self.camera_class(&camera_id);
        let state = self.camera_state(&camera_id).unwrap_or_default();
        let mains = self.mains();
        let capture = VideoTestCapture { frames };
        // Never analyse a capture without a real picture: no findings, no plan.
        let (metrics, findings, _live) = video::analyze_live(&capture, &state, class, mains).map_err(|np| {
            let reason = camera_busy::reason_of(np);
            self.note_picture(Some(reason));
            CameraError::no_picture(reason)
        })?;
        let settings = video::recommend_settings(&metrics, &state, class, mains, &constraints);
        let score = coach_core::score::score_area(Area::Camera, &findings);
        let report = CameraReport { metrics, findings, settings, score, class };
        self.inner.lock().unwrap().last_camera = Some((session_camera, report.clone()));
        Ok(report)
    }

    pub async fn apply_camera_settings(&self, camera_id: &str, settings: CameraSettings) -> ApplyResult {
        let mut res = ApplyResult::default();
        let cam_name = self.platform.camera.list().into_iter().find(|c| c.id == camera_id).map(|c| c.name);
        let obs_src = match (self.obs().await, &cam_name) {
            (Some(o), Some(n)) => obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok().and_then(|s| obs_source_for_camera(&s, n)),
            _ => None,
        };
        let mut targets = Vec::new();
        if settings.uvc.is_some() {
            targets.push(Target::Camera(camera_id.to_string()));
        }
        if let Some(src) = &obs_src {
            targets.push(Target::ObsInputSettings(src.clone()));
        }
        let label = format!("Before changing your {} settings", cam_name.as_deref().map(plain_device_name).unwrap_or_else(|| "camera".into()));
        if let Err(e) = self.backup_before(&label, &targets).await {
            res.fail("backup", e);
            return res;
        }
        if let Some(uvc) = &settings.uvc {
            let cam = self.platform.camera.clone();
            let (id, u) = (camera_id.to_string(), uvc.clone());
            match tokio::task::spawn_blocking(move || cam.apply(&id, &u)).await.map_err(|e| e.to_string()).and_then(|r| r) {
                Ok(warnings) => {
                    res.ok("camera_controls");
                    for w in warnings {
                        res.fail("camera_control", w);
                    }
                }
                Err(e) => res.fail("camera_controls", e),
            }
        }
        if let Some(o) = self.obs().await {
            let name = self.platform.camera.list().into_iter().find(|c| c.id == camera_id).map(|c| c.name).unwrap_or_default();
            if let Some(src) = obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok().and_then(|s| obs_source_for_camera(&s, &name)) {
                let f = &settings.format;
                let action = Action::SetObsVideoCaptureMode {
                    input: src,
                    width: f.width as u32,
                    height: f.height as u32,
                    fps: f.fps as f32,
                    format: if matches!(f.format, video::VideoFormat::Mjpeg) { "mjpeg".into() } else { "any".into() },
                };
                match self.execute(&action).await {
                    Ok(_) => res.ok("obs_camera_mode"),
                    Err(e) => res.fail("obs_camera_mode", e),
                }
            }
        }
        self.inner.lock().unwrap().applied_camera = Some((camera_id.to_string(), settings));
        res
    }

    // ------------------------------------------------------------ stream health

    pub async fn start_health_monitor(self: &Arc<Self>) -> R<()> {
        let o = self.obs().await.ok_or("Connect to OBS first.")?;
        self.stop_health_session();
        let st: Option<ObsState> = obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok();
        let enc = st.as_ref().and_then(|s| s.encoder.clone());
        let cfg = MonitorConfig {
            configured_bitrate_kbps: enc.as_ref().map(|e| e.bitrate_kbps).filter(|b| *b > 0),
            encoder_is_cpu: enc.as_ref().is_some_and(|e| e.is_cpu_encoder()),
            ..Default::default()
        };
        let report_cfg = ReportConfig {
            configured_bitrate_kbps: cfg.configured_bitrate_kbps,
            target_fps: enc.as_ref().map(|e| e.fps).filter(|f| *f > 0.0),
            encoder_is_cpu: cfg.encoder_is_cpu,
            ..Default::default()
        };
        let running = Arc::new(AtomicBool::new(true));
        let samples = Arc::new(Mutex::new(Vec::new()));
        self.inner.lock().unwrap().health = Some(HealthSession { running: running.clone(), samples: samples.clone(), report_cfg });
        let me = self.clone();
        let interval = self.health_interval;
        tokio::spawn(async move {
            let mut monitor = HealthMonitor::new(cfg);
            let t0 = Instant::now();
            while running.load(Ordering::SeqCst) {
                let t_ms = t0.elapsed().as_millis() as u64;
                if let Ok(sample) = obs::stats_sample(o.as_ref(), t_ms).await {
                    for alert in monitor.ingest(sample.clone()) {
                        me.emitter.emit(crate::types::events::HEALTH_ALERT, serde_json::to_value(alert).unwrap_or(Value::Null));
                    }
                    let view = HealthSampleView { sample: sample.clone(), window: monitor.window_stats(), active: monitor.active_alerts() };
                    me.emitter.emit(crate::types::events::HEALTH_SAMPLE, serde_json::to_value(view).unwrap_or(Value::Null));
                    samples.lock().unwrap().push(sample);
                }
                tokio::time::sleep(interval).await;
            }
        });
        Ok(())
    }

    fn stop_health_session(&self) -> Option<HealthSession> {
        let s = self.inner.lock().unwrap().health.take();
        if let Some(s) = &s {
            s.running.store(false, Ordering::SeqCst);
        }
        s
    }

    pub fn stop_health_monitor(&self) -> R<StreamReport> {
        let s = self.stop_health_session().ok_or("Stream monitoring isn't running.")?;
        let samples = s.samples.lock().unwrap().clone();
        if samples.len() < 2 {
            return Err("Not enough stream data yet — keep monitoring for at least a minute.".into());
        }
        Ok(post_stream_report(&samples, &s.report_cfg))
    }

    // ------------------------------------------------------------ persistence

    fn setup_path(&self) -> PathBuf {
        self.data_dir.join("setup.json")
    }

    pub fn load_setup(&self) -> Option<SavedProfile> {
        serde_json::from_str(&std::fs::read_to_string(self.setup_path()).ok()?).ok()
    }

    pub fn save_setup(&self, name: &str) -> R<SavedProfile> {
        // Keep the answers to "What mic is plugged into this?".
        let existing_choices = self.load_setup().map(|p| p.mic_behind).unwrap_or_default();
        let inner = self.inner.lock().unwrap();
        let p = SavedProfile {
            name: if name.trim().is_empty() { "My setup".into() } else { name.trim().to_string() },
            saved_at: crate::paths::now_iso(),
            mic_chain: inner.applied_chain.clone(),
            mic_behind: existing_choices,
            camera_settings: inner.applied_camera.as_ref().map(|c| c.1.clone()),
            camera_id: inner.applied_camera.as_ref().map(|c| c.0.clone()),
            obs_input: inner.applied_input.clone(),
        };
        drop(inner);
        std::fs::create_dir_all(&self.data_dir).map_err(|e| format!("Couldn't save ({e})."))?;
        std::fs::write(self.setup_path(), serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?).map_err(|e| format!("Couldn't save ({e})."))?;
        Ok(p)
    }

    /// Re-apply the saved setup (startup, camera re-plug). Webcams forget their
    /// settings; OBS filters normally persist but are re-checked.
    pub async fn reapply_saved_setup(&self) -> ApplyResult {
        let Some(p) = self.load_setup() else {
            let mut r = ApplyResult::default();
            r.fail("setup", "No saved setup yet.");
            return r;
        };
        let mut res = ApplyResult::default();
        if let (Some(id), Some(s)) = (&p.camera_id, &p.camera_settings) {
            if self.platform.camera.list().iter().any(|c| &c.id == id) {
                let r = self.apply_camera_settings(id, s.clone()).await;
                res.applied.extend(r.applied);
                res.failed.extend(r.failed);
            }
        }
        if let Some(chain) = &p.mic_chain {
            match &p.obs_input {
                Some(input) if self.obs().await.is_some() => {
                    let r = self.apply_mic_chain(chain.clone(), input).await;
                    res.applied.extend(r.applied);
                    res.failed.extend(r.failed);
                    res.restart_obs_needed |= r.restart_obs_needed;
                }
                _ => match self.backup_before("Before re-applying your saved setup", &[Target::ChainJson]).await.and_then(|_| self.write_chain_json(chain)) {
                    Ok(_) => res.ok("chain_json"),
                    Err(e) => res.fail("chain_json", e),
                },
            }
        }
        res
    }
}

/// Pair start/end marks into segments; an unfinished step ends at `end_s`.
pub fn segments_from_marks(marks: &[(SegmentKind, Edge, f64)], end_s: f64) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut open: Vec<(SegmentKind, f64)> = Vec::new();
    for (kind, edge, t) in marks {
        match edge {
            Edge::Start => {
                open.retain(|o| o.0 != *kind);
                open.push((*kind, *t));
            }
            Edge::End => {
                if let Some(pos) = open.iter().position(|o| o.0 == *kind) {
                    let (_, s) = open.remove(pos);
                    if *t > s {
                        out.push(Segment { kind: *kind, start_s: s, end_s: t.min(end_s) });
                    }
                }
            }
        }
    }
    for (kind, s) in open {
        if end_s > s {
            out.push(Segment { kind, start_s: s, end_s });
        }
    }
    out.sort_by(|a, b| a.start_s.partial_cmp(&b.start_s).unwrap());
    out
}

/// The OBS audio input capturing a Windows endpoint ("default" counts for the
/// default device).
pub fn obs_input_for_device(s: &ObsState, device_id: &str, is_default: bool) -> Option<String> {
    s.audio_inputs
        .iter()
        .filter(|i| i.kind == coach_core::system::OBS_KIND_MIC)
        .find(|i| i.device_id == device_id || (is_default && (i.device_id == "default" || i.device_id.is_empty())))
        .map(|i| i.name.clone())
}

/// The OBS video capture source showing this camera.
pub fn obs_source_for_camera(s: &ObsState, camera_name: &str) -> Option<String> {
    let want = prof::normalize(camera_name);
    s.video_captures
        .iter()
        .find(|v| {
            let n = prof::normalize(&v.device_name);
            !n.is_empty() && (n == want || n.contains(&want) || want.contains(&n))
        })
        .map(|v| v.name.clone())
}

/// `Microphone (Yeti Stereo Microphone)` → `Yeti Stereo Microphone` for labels.
pub fn plain_device_name(name: &str) -> String {
    match (name.find('('), name.rfind(')')) {
        (Some(a), Some(b)) if b > a + 1 => name[a + 1..b].trim().to_string(),
        _ => name.trim().to_string(),
    }
}

/// The `CheckSetting` check that proves a Windows audio-effect action worked.
fn fx_check_id(a: &Action) -> Option<&'static str> {
    match a {
        Action::DisableEnhancements { .. } => Some("enhancements_off"),
        Action::DisableVoiceClarity { .. } => Some("voice_clarity_off"),
        Action::DisableListen { .. } => Some("listen_off"),
        Action::DisableExclusiveMode { .. } => Some("exclusive_off"),
        _ => None,
    }
}

/// The Windows endpoint an action changes, if any.
fn action_endpoint(a: &Action) -> Option<String> {
    match a {
        Action::DisableEnhancements { endpoint }
        | Action::DisableVoiceClarity { endpoint }
        | Action::SetMicBoost { endpoint, .. }
        | Action::SetEndpointVolume { endpoint, .. }
        | Action::DisableExclusiveMode { endpoint }
        | Action::DisableListen { endpoint } => Some(endpoint.clone()),
        Action::PinObsMicDevice { device_id, .. } => Some(device_id.clone()),
        _ => None,
    }
}

/// Findings we turned out not to be able to fix automatically become guided
/// steps: no "We can fix this" tag, and the "We can do it for you." sentence
/// is dropped from the advice.
pub fn downgrade_unfixable<'a>(findings: &mut [coach_core::finding::Finding], ids: impl Iterator<Item = &'a String>) {
    let ids: Vec<&String> = ids.collect();
    for f in findings.iter_mut().filter(|f| ids.contains(&&f.id)) {
        f.fix_kind = coach_core::finding::FixKind::Guided;
        if let Some(i) = f.fix.rfind(" We can ") {
            f.fix.truncate(i);
        }
    }
}

pub fn needs_admin(a: &Action) -> bool {
    matches!(
        a,
        Action::DisableEnhancements { .. } | Action::DisableVoiceClarity { .. } | Action::DisableExclusiveMode { .. } | Action::DisableListen { .. } | Action::SetMicBoost { .. }
    )
}

pub fn needs_obs_closed(a: &Action) -> bool {
    matches!(a, Action::SetObsInputMono { .. } | Action::EnableObsWebsocket)
}

/// Plain-language label for a fix button.
pub fn label_for(a: &Action) -> String {
    match a {
        Action::SetObsInputMono { input } => format!("Send \"{input}\" to both ears (mono)"),
        Action::SetObsInputMuted { input, muted } => format!("{} \"{input}\" in OBS", if *muted { "Mute" } else { "Unmute" }),
        Action::RemoveObsFilter { filter, .. } => format!("Remove the \"{filter}\" filter"),
        Action::ReorderObsFilters { .. } => "Put your mic filters in the right order".into(),
        Action::SetObsFilterSettings { filter, .. } => format!("Fix the \"{filter}\" settings"),
        Action::AddObsFilter { name, .. } => format!("Add \"{name}\""),
        Action::SetObsSampleRate { hz } => format!("Set OBS audio to {} kHz", *hz as f64 / 1000.0),
        Action::SetObsMonitoring { monitoring, .. } => match monitoring {
            MonitoringType::Off => "Stop OBS playing your mic back to you".into(),
            _ => "Change OBS mic monitoring".into(),
        },
        Action::PinObsMicDevice { .. } => "Make OBS always use this mic".into(),
        Action::EnableObsWebsocket => "Turn on OBS's WebSocket server".into(),
        Action::SetObsVideoCaptureMode { width, height, fps, .. } => format!("Set the webcam to {width}×{height} at {fps:.0} fps"),
        Action::SetObsStreamEncoder { .. } => "Use your graphics card to encode the stream".into(),
        Action::SetObsRateControl { mode } => format!("Use {mode} rate control"),
        Action::SetObsKeyframeInterval { seconds } => format!("Set keyframes every {seconds} s"),
        Action::SetObsStreamBitrate { kbps } => format!("Set stream bitrate to {kbps} Kbps"),
        Action::SetObsOutputResolution { width, height } => format!("Stream at {width}×{height}"),
        Action::SetObsAudioBitrate { kbps } => format!("Set audio bitrate to {kbps} Kbps"),
        Action::SetObsRecordingFormat { format } => format!("Record as {format} (survives crashes)"),
        Action::SetWindowsDucking { .. } => "Stop Windows turning your game down during calls".into(),
        Action::DisableEnhancements { .. } => "Turn off Windows audio enhancements".into(),
        Action::DisableVoiceClarity { .. } => "Turn off Windows Voice Clarity".into(),
        Action::SetMicBoost { db, .. } => format!("Set Windows Microphone Boost to {db:.0} dB"),
        Action::SetEndpointVolume { volume, .. } => format!("Set the Windows mic level to {:.0}", volume * 100.0),
        Action::DisableExclusiveMode { .. } => "Stop apps taking over your mic".into(),
        Action::DisableListen { .. } => "Turn off \"Listen to this device\"".into(),
        Action::DisableUsbSelectiveSuspend => "Stop Windows powering down USB audio".into(),
    }
}
