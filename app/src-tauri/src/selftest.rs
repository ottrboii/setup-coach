//! `Setup Coach.exe --self-test [--out <dir>] [--skip-obs] [--no-obs-password]`
//!
//! A no-questions diagnostic run without a window: read-only checks plus one
//! reversible OBS test (a temporary scene + throwaway input that are removed
//! again). Writes `self-test.json` / `self-test.txt` and packs them, the
//! contents of `%APPDATA%\SetupCoach\` and the newest OBS log into
//! `SetupCoach-selftest-<timestamp>.zip` (Desktop by default). The zip path is
//! printed to stdout and written to `%APPDATA%\SetupCoach\last-self-test.txt`.
//!
//! Every check runs in its own task with a time limit, so a hanging device
//! or OBS can't block the others. Only localhost is contacted (OBS, GoXLR
//! Utility). No audio or video is captured or processed.

use crate::app::{AppCore, CollectingEmitter};
use crate::goxlr::{self, GoXlrApi, GoXlrHttp};
use crate::obs::{self, ObsApi, F_COMPRESSOR, F_EXPANDER, F_GAIN, F_LIMITER, F_VOICE};
use crate::types::SavedProfile;
use coach_core::audio::chain::{Compressor, Expander, Limiter};
use coach_core::audio::ProcessingChain;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Name of the temporary OBS scene the apply test creates (and removes).
pub const TEST_SCENE: &str = "Setup Coach self-test";
/// Name of the throwaway OBS input inside it (scene and input names must differ).
pub const TEST_INPUT: &str = "Setup Coach self-test mic";
/// Default limit per check.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Files bigger than this are left out of the zip (noted in the report).
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        }
    }
}

/// What a check found.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: Status,
    pub note: String,
    pub details: Value,
}

impl Outcome {
    fn new(status: Status, note: impl Into<String>, details: Value) -> Self {
        Outcome { status, note: note.into(), details }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// Stable machine id, e.g. `obs_apply_test`.
    pub id: String,
    /// Short human title.
    pub name: String,
    pub status: Status,
    /// One plain-language sentence or two.
    pub note: String,
    pub details: Value,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
    pub skip: usize,
    /// `fail` if any check failed, else `warn` if any warned, else `pass`.
    pub overall: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Bundle {
    /// Paths inside the zip.
    pub files: Vec<String>,
    /// Files we wanted but couldn't include, with the reason.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub kind: &'static str,
    pub schema_version: u32,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub windows_build: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub summary: Summary,
    pub checks: Vec<Check>,
    pub bundle: Bundle,
}

/// Options from the command line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliOptions {
    pub out_dir: Option<PathBuf>,
    /// Don't talk to OBS at all.
    pub skip_obs: bool,
    /// Don't read the OBS WebSocket password from OBS's config file.
    pub no_obs_password: bool,
}

/// `Some` when the arguments ask for the self-test.
pub fn parse_args(args: &[String]) -> Option<CliOptions> {
    if !args.iter().any(|a| a == "--self-test") {
        return None;
    }
    let mut o = CliOptions::default();
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => o.out_dir = it.next().map(PathBuf::from),
            "--skip-obs" => o.skip_obs = true,
            "--no-obs-password" => o.no_obs_password = true,
            s if s.starts_with("--out=") => o.out_dir = Some(PathBuf::from(&s[6..])),
            _ => {}
        }
    }
    Some(o)
}

/// Everything the checks need.
pub struct SelfTest {
    pub core: Arc<AppCore>,
    /// Where the installer puts our VST3 (`paths::vst3_path()`).
    pub vst3_path: Option<String>,
    pub skip_obs: bool,
    pub no_obs_password: bool,
    pub check_timeout: Duration,
    /// Filters on the user's own sources before the apply test (to prove they
    /// weren't touched).
    user_filters_before: Mutex<Option<BTreeMap<String, Vec<String>>>>,
}

impl SelfTest {
    pub fn new(core: Arc<AppCore>, vst3_path: Option<String>) -> Self {
        SelfTest { core, vst3_path, skip_obs: false, no_obs_password: false, check_timeout: CHECK_TIMEOUT, user_filters_before: Mutex::new(None) }
    }

    /// Run every check; never panics, never takes much longer than
    /// `check_timeout` per check.
    pub async fn run(self: &Arc<Self>) -> Report {
        let started_at = crate::paths::now_iso();
        let mut checks = Vec::new();
        macro_rules! check {
            ($id:expr, $name:expr, $method:ident) => {{
                let me = self.clone();
                checks.push(run_check($id, $name, self.check_timeout, async move { me.$method().await }).await);
            }};
        }
        check!("app_and_windows", "App and Windows version", check_versions);
        check!("microphones", "Microphones found", check_mics);
        check!("windows_ai_effects", "Windows / app AI effects on the mic", check_ai_effects);
        check!("cameras", "Cameras found", check_cameras);
        check!("vst3_installed", "Setup Coach Voice plugin installed", check_vst3);
        check!("obs_running", "OBS is open", check_obs_running);
        check!("obs_websocket", "OBS WebSocket reachable", check_obs_websocket);
        check!("obs_vst3_host", "OBS can load VST3 plugins", check_obs_vst3_host);
        check!("obs_apply_test", "Apply the mic chain to a temporary OBS source", check_obs_apply);
        // Always runs (own time limit), also when the apply test timed out.
        check!("obs_apply_cleanup", "Remove the temporary OBS source again", check_obs_cleanup);
        check!("goxlr_utility", "GoXLR Utility reachable", check_goxlr);
        check!("saved_setup", "Saved setup readable", check_saved_setup);
        let windows_build = checks.first().and_then(|c| c.details["windows_build"].as_str().map(str::to_string));
        Report {
            kind: "setup-coach-self-test",
            schema_version: 1,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            windows_build,
            started_at,
            finished_at: crate::paths::now_iso(),
            summary: summarize(&checks),
            checks,
            bundle: Bundle::default(),
        }
    }

    // ------------------------------------------------------------------ checks

    async fn check_versions(self: Arc<Self>) -> Outcome {
        let sys = self.core.platform.system.clone();
        let build = blocking(move || sys.windows_build()).await.flatten();
        let exe = std::env::current_exe().ok().map(|p| p.display().to_string());
        let details = json!({
            "app_version": env!("CARGO_PKG_VERSION"),
            "windows_build": build,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "exe": exe,
        });
        match &build {
            Some(b) => Outcome::new(Status::Pass, format!("Setup Coach {} on Windows build {b}.", env!("CARGO_PKG_VERSION")), details),
            None => Outcome::new(Status::Warn, format!("Setup Coach {}; couldn't read the Windows version.", env!("CARGO_PKG_VERSION")), details),
        }
    }

    async fn check_mics(self: Arc<Self>) -> Outcome {
        let audio = self.core.platform.audio.clone();
        let mics = blocking(move || audio.list_inputs()).await.unwrap_or_default();
        let list: Vec<Value> = mics
            .iter()
            .map(|d| {
                let p = self.core.match_mic_view(&d.name, d.usb);
                json!({
                    "name": d.name, "id": d.id, "is_default": d.is_default, "is_default_comms": d.is_default_comms,
                    "channels": d.channels, "sample_rate": d.sample_rate,
                    "usb": d.usb.map(|(v, p)| format!("{v:04X}:{p:04X}")),
                    "profile": p.map(|p| json!({ "id": p.profile_id, "name": p.display_name, "confidence": p.confidence, "verified": p.verified })),
                })
            })
            .collect();
        let default = mics.iter().find(|d| d.is_default);
        let details = json!({ "count": mics.len(), "default": default.map(|d| &d.name), "devices": list });
        match (mics.len(), default) {
            (0, _) => Outcome::new(Status::Fail, "Windows reports no microphones. Plug one in (or check Sound settings) and run the test again.", details),
            (n, Some(d)) => Outcome::new(Status::Pass, format!("Found {n} microphone(s). Windows uses \u{201c}{}\u{201d} by default ({} Hz).", d.name, d.sample_rate), details),
            (n, None) => Outcome::new(Status::Warn, format!("Found {n} microphone(s), but none is set as the Windows default."), details),
        }
    }

    async fn check_ai_effects(self: Arc<Self>) -> Outcome {
        let sys = self.core.platform.system.clone();
        let Some((audio, apps)) = blocking(move || (sys.windows_audio(), sys.processing_apps())).await else {
            return Outcome::new(Status::Fail, "Couldn't read the Windows sound settings.", Value::Null);
        };
        let ai_apps: Vec<&str> = apps.ai_active();
        let dsp_apps: Vec<&str> = apps.dsp_active();
        let Some(audio) = audio else {
            return Outcome::new(Status::Skip, "Windows sound settings aren't readable here.", json!({ "ai_apps": ai_apps, "dsp_apps": dsp_apps }));
        };
        let mics: Vec<_> = audio.endpoints.iter().filter(|e| e.is_capture).collect();
        let clarity: Vec<&str> = mics.iter().filter(|e| e.voice_clarity_enabled).map(|e| e.name.as_str()).collect();
        let enh: Vec<&str> = mics.iter().filter(|e| e.enhancements_enabled).map(|e| e.name.as_str()).collect();
        let details = json!({
            "voice_clarity_on": clarity,
            "audio_enhancements_on": enh,
            "ai_apps_active": ai_apps,
            "other_mic_effect_apps": dsp_apps,
            "mic_privacy_desktop_allowed": audio.mic_privacy_desktop_allowed,
            "endpoints": mics.iter().map(|e| json!({
                "name": e.name, "default": e.is_default, "sample_rate": e.sample_rate, "channels": e.channels,
                "voice_clarity": e.voice_clarity_enabled, "enhancements": e.enhancements_enabled,
                "mic_boost_db": e.mic_boost_db, "volume": e.endpoint_volume, "bluetooth_handsfree": e.is_bluetooth_handsfree,
            })).collect::<Vec<_>>(),
            "not_detected_yet": ["Windows Studio Effects"],
        });
        let mut problems = Vec::new();
        if !clarity.is_empty() {
            problems.push(format!("Windows Voice Clarity is on for {}", clarity.join(", ")));
        }
        if !ai_apps.is_empty() {
            problems.push(format!("{} is processing the mic", ai_apps.join(" and ")));
        }
        if !audio.mic_privacy_desktop_allowed {
            problems.push("Windows blocks desktop apps from using the mic".to_string());
        }
        if problems.is_empty() {
            Outcome::new(Status::Pass, "No AI voice effects found on your mics.", details)
        } else {
            Outcome::new(Status::Warn, format!("{}. The app offers to turn this off.", problems.join("; ")), details)
        }
    }

    async fn check_cameras(self: Arc<Self>) -> Outcome {
        let cam = self.core.platform.camera.clone();
        let sys = self.core.platform.system.clone();
        let Some((cams, camsys)) = blocking(move || (cam.list(), sys.camera_system())).await else {
            return Outcome::new(Status::Fail, "Couldn't list cameras.", Value::Null);
        };
        let list: Vec<Value> = cams
            .iter()
            .map(|c| {
                let p = self.core.match_camera_view(&c.name, c.usb);
                json!({ "name": c.name, "id": c.id, "usb": c.usb.map(|(v, p)| format!("{v:04X}:{p:04X}")),
                        "profile": p.map(|p| json!({ "id": p.profile_id, "name": p.display_name, "confidence": p.confidence, "verified": p.verified })) })
            })
            .collect();
        let privacy = camsys.as_ref().map(|c| c.privacy_allowed);
        let details = json!({ "count": cams.len(), "devices": list, "camera_privacy_allowed": privacy, "vendor_apps_running": camsys.map(|c| c.vendor_apps_running) });
        if cams.is_empty() {
            return Outcome::new(Status::Warn, "No cameras found (fine if you don't use one).", details);
        }
        if privacy == Some(false) {
            return Outcome::new(Status::Warn, format!("Found {} camera(s), but Windows blocks apps from using the camera.", cams.len()), details);
        }
        let names: Vec<&str> = cams.iter().map(|c| c.name.as_str()).collect();
        Outcome::new(Status::Pass, format!("Found {} camera(s): {}.", cams.len(), names.join(", ")), details)
    }

    async fn check_vst3(self: Arc<Self>) -> Outcome {
        let Some(path) = self.vst3_path.clone() else {
            return Outcome::new(Status::Skip, "The voice plugin only exists on Windows.", Value::Null);
        };
        let p = PathBuf::from(&path);
        let binary = p.join("Contents").join("x86_64-win").join("SetupCoachVoice.vst3");
        let details = json!({ "path": path, "exists": p.exists(), "is_bundle_dir": p.is_dir(), "binary_found": binary.is_file() });
        if !p.exists() {
            Outcome::new(Status::Fail, "The Setup Coach Voice plugin isn't installed. Re-run the installer.", details)
        } else if p.is_dir() && !binary.is_file() {
            Outcome::new(Status::Warn, "The voice plugin folder is there but looks incomplete. Re-run the installer.", details)
        } else {
            Outcome::new(Status::Pass, "The Setup Coach Voice plugin is installed.", details)
        }
    }

    async fn check_obs_running(self: Arc<Self>) -> Outcome {
        let sys = self.core.platform.system.clone();
        let running = blocking(move || sys.obs_running()).await.unwrap_or(false);
        if running {
            Outcome::new(Status::Pass, "OBS is open.", json!({ "running": true }))
        } else {
            Outcome::new(Status::Warn, "OBS isn't open, so the OBS checks were skipped. Open OBS and run the self-test again.", json!({ "running": false }))
        }
    }

    async fn check_obs_websocket(self: Arc<Self>) -> Outcome {
        if self.skip_obs {
            return Outcome::new(Status::Skip, "Skipped: started with --skip-obs.", Value::Null);
        }
        let cfg = self.core.platform.system.obs_config_dir().and_then(|d| obs::read_websocket_config(&d));
        // Never the password itself, only whether one is saved.
        let mut details = json!({
            "config_found": cfg.is_some(),
            "server_enabled": cfg.as_ref().map(|c| c.server_enabled),
            "port": cfg.as_ref().map(|c| c.server_port),
            "auth_required": cfg.as_ref().map(|c| c.auth_required),
            "password_saved": cfg.as_ref().map(|c| !c.server_password.is_empty()),
            "password_read": !self.no_obs_password,
        });
        let existing = match self.core.obs().await {
            Some(o) => o.request("GetVersion", Value::Null).await.ok(),
            None => None,
        };
        let version = match existing {
            Some(v) => v,
            None => {
                let pw = self.no_obs_password.then(String::new);
                let st = self.core.connect_obs(pw).await;
                if !st.running {
                    return Outcome::new(Status::Skip, "OBS isn't open.", details);
                }
                if !st.connected {
                    let mut note = st.error.unwrap_or_else(|| "Couldn't connect to OBS.".into());
                    if self.no_obs_password {
                        note.push_str(" (Started with --no-obs-password, so the saved password wasn't read.)");
                    }
                    return Outcome::new(Status::Fail, note, details);
                }
                match self.core.obs().await {
                    Some(o) => o.request("GetVersion", Value::Null).await.unwrap_or(Value::Null),
                    None => Value::Null,
                }
            }
        };
        details["obs_version"] = version["obsVersion"].clone();
        details["obs_websocket_version"] = version["obsWebSocketVersion"].clone();
        Outcome::new(Status::Pass, format!("Connected to OBS {}.", version["obsVersion"].as_str().unwrap_or("")), details)
    }

    async fn connected_obs(&self) -> Result<Arc<dyn ObsApi>, Outcome> {
        if self.skip_obs {
            return Err(Outcome::new(Status::Skip, "Skipped: started with --skip-obs.", Value::Null));
        }
        self.core.obs().await.ok_or_else(|| Outcome::new(Status::Skip, "Not connected to OBS (see the WebSocket check).", Value::Null))
    }

    async fn check_obs_vst3_host(self: Arc<Self>) -> Outcome {
        let o = match self.connected_obs().await {
            Ok(o) => o,
            Err(skip) => return skip,
        };
        let kinds = match filter_kinds(o.as_ref()).await {
            Ok(k) => k,
            Err(e) => return Outcome::new(Status::Fail, format!("OBS didn't list its filter types ({e})."), Value::Null),
        };
        let host = obs::vst3_host_kind(&kinds);
        let related: Vec<&String> = kinds.iter().filter(|k| k.contains("vst") || k.contains("atk")).collect();
        let details = json!({ "host_kind": host, "vst_related_kinds": related, "all_filter_kinds": kinds });
        match host {
            Some(h) => {
                let how = if h.contains("atk") { "via atkAudio" } else { "natively" };
                Outcome::new(Status::Pass, format!("OBS can load our voice plugin ({how}, filter \u{201c}{h}\u{201d})."), details)
            }
            None => Outcome::new(
                Status::Warn,
                "OBS can't load VST3 plugins yet. Install the free atkAudio plugin for OBS; the rest of the mic setup still works.",
                details,
            ),
        }
    }

    async fn check_obs_apply(self: Arc<Self>) -> Outcome {
        let o = match self.connected_obs().await {
            Ok(o) => o,
            Err(skip) => return skip,
        };
        let o = o.as_ref();
        // Never touch OBS while the user is live or recording.
        let live = obs_live_outputs(o).await;
        if !live.is_empty() {
            return Outcome::new(
                Status::Skip,
                format!("OBS is {}, so the test left OBS alone. Run the self-test again when you're not live.", live.join(" and ")),
                json!({ "live_outputs": live }),
            );
        }
        // Snapshot the user's sources so cleanup can prove they're untouched.
        match user_filters(o).await {
            Ok(m) => *self.user_filters_before.lock().unwrap() = Some(m),
            Err(e) => return Outcome::new(Status::Fail, format!("Couldn't read OBS's sources ({e})."), Value::Null),
        }
        // A leftover from an interrupted earlier run is ours: remove it first.
        let leftovers = remove_test_objects(o).await;
        let mut details = json!({ "scene": TEST_SCENE, "input": TEST_INPUT, "leftovers_removed": leftovers });
        if let Err(e) = o.request("CreateScene", json!({ "sceneName": TEST_SCENE })).await {
            return Outcome::new(Status::Fail, format!("Couldn't create a temporary OBS scene ({e})."), details);
        }
        // A media source with no file: has audio (so audio filters apply) but
        // opens no device and plays nothing. Hidden in a scene nobody shows.
        let create = o
            .request(
                "CreateInput",
                json!({ "sceneName": TEST_SCENE, "inputName": TEST_INPUT, "inputKind": "ffmpeg_source",
                        "inputSettings": { "is_local_file": true, "local_file": "", "looping": false }, "sceneItemEnabled": false }),
            )
            .await;
        if let Err(e) = create {
            return Outcome::new(Status::Fail, format!("Couldn't create a temporary OBS source ({e})."), details);
        }
        let host = filter_kinds(o).await.ok().and_then(|k| obs::vst3_host_kind(&k));
        let (applied, failed) = obs::apply_chain(o, TEST_INPUT, &self_test_chain(), self.vst3_path.as_deref()).await;
        let readback = o.request("GetSourceFilterList", json!({ "sourceName": TEST_INPUT })).await;
        let order: Vec<String> = readback
            .as_ref()
            .ok()
            .and_then(|r| r["filters"].as_array())
            .map(|a| a.iter().filter_map(|f| f["filterName"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut expected: Vec<String> = Vec::new();
        if host.is_some() && self.vst3_path.is_some() {
            expected.push(F_VOICE.into());
        }
        expected.extend([F_GAIN, F_EXPANDER, F_COMPRESSOR, F_LIMITER].map(String::from));
        details["applied"] = json!(applied);
        details["failed"] = json!(failed.iter().map(|(id, e)| json!({ "id": id, "error": e })).collect::<Vec<_>>());
        details["expected_order"] = json!(expected);
        details["readback_order"] = json!(order);
        details["readback_filters"] = readback.clone().unwrap_or(Value::Null)["filters"].clone();
        if let Err(e) = readback {
            return Outcome::new(Status::Fail, format!("Couldn't read the filters back from OBS ({e})."), details);
        }
        let core_only: Vec<&String> = order.iter().filter(|n| n.as_str() != F_VOICE).collect();
        let core_ok = core_only == expected.iter().filter(|n| n.as_str() != F_VOICE).collect::<Vec<_>>();
        if order == expected && failed.is_empty() {
            let with = if expected.first().is_some_and(|n| n == F_VOICE) { "voice plugin, " } else { "" };
            Outcome::new(Status::Pass, format!("Applied the mic setup ({with}gain, expander, compressor, limiter) to a test source and read it back in the right order."), details)
        } else if core_ok {
            let why = failed.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>().join(" ");
            Outcome::new(Status::Warn, format!("OBS's own filters went on in the right order, but not everything worked: {why}"), details)
        } else {
            Outcome::new(Status::Fail, "The filters in OBS didn't end up in the expected order.", details)
        }
    }

    async fn check_obs_cleanup(self: Arc<Self>) -> Outcome {
        let o = match self.connected_obs().await {
            Ok(o) => o,
            Err(skip) => return skip,
        };
        let o = o.as_ref();
        let before = self.user_filters_before.lock().unwrap().take();
        let Some(before) = before else {
            // The apply test never started: nothing was created.
            let left = remove_test_objects(o).await;
            return Outcome::new(Status::Skip, "The apply test didn't run, so there was nothing to remove.", json!({ "leftovers_removed": left }));
        };
        let removed = remove_test_objects(o).await;
        let still_there = test_objects_present(o).await;
        let after = user_filters(o).await.unwrap_or_default();
        let changed: Vec<&String> = before.keys().chain(after.keys()).filter(|k| before.get(*k) != after.get(*k)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let details = json!({ "removed": removed, "still_present": still_there, "user_sources_checked": before.len(), "user_sources_changed": changed });
        if !still_there.is_empty() {
            Outcome::new(Status::Fail, format!("Couldn't remove {} from OBS. Delete it by hand in OBS.", still_there.join(" and ")), details)
        } else if !changed.is_empty() {
            Outcome::new(Status::Fail, "The test source is gone, but filters on your own OBS sources changed during the test.", details)
        } else {
            Outcome::new(Status::Pass, "Removed everything the test created; your own OBS sources are unchanged.", details)
        }
    }

    async fn check_goxlr(self: Arc<Self>) -> Outcome {
        let sys = self.core.platform.system.clone();
        let official = blocking(move || sys.processing_apps().goxlr_app_running).await.unwrap_or(false);
        let mut details = json!({ "port": goxlr::DEFAULT_PORT, "official_goxlr_app_running": official });
        if let Some(api) = self.core.goxlr().await {
            return match api.devices().await {
                Ok(devs) => {
                    details["devices"] = json!(devs);
                    let d = devs.first();
                    let what = d.map(|d| format!(" (GoXLR {}, {:?} mic, gain {:.0} dB)", d.device_type, d.mic_type, d.mic_gain_db)).unwrap_or_default();
                    if official {
                        Outcome::new(Status::Warn, format!("GoXLR Utility answers{what}, but the old official GoXLR app is also running. Close it: both can't run together."), details)
                    } else {
                        Outcome::new(Status::Pass, format!("GoXLR Utility answers{what}."), details)
                    }
                }
                Err(e) => Outcome::new(Status::Warn, format!("GoXLR Utility answered but couldn't report the GoXLR ({e})."), details),
            };
        }
        // Not found: tell "not running" apart from "running, no GoXLR".
        let raw = if self.core.goxlr_autodetect.load(std::sync::atomic::Ordering::SeqCst) {
            Some(GoXlrHttp { port: goxlr::DEFAULT_PORT }.request(json!("GetStatus")).await)
        } else {
            None
        };
        match raw {
            Some(Ok(_)) => Outcome::new(Status::Warn, "GoXLR Utility is running but doesn't see a GoXLR. Check the USB cable.", details),
            _ if official => Outcome::new(
                Status::Warn,
                "The old official GoXLR app is running, not GoXLR Utility. Setup Coach works best with GoXLR Utility.",
                details,
            ),
            _ => Outcome::new(Status::Skip, "GoXLR Utility isn't running (fine if you don't use a GoXLR).", details),
        }
    }

    async fn check_saved_setup(self: Arc<Self>) -> Outcome {
        let dir = self.core.data_dir.clone();
        blocking(move || saved_setup_outcome(&dir)).await.unwrap_or_else(|| Outcome::new(Status::Fail, "Couldn't read the saved setup.", Value::Null))
    }
}

fn saved_setup_outcome(dir: &Path) -> Outcome {
    let mut details = json!({ "data_dir": dir.display().to_string(), "exists": dir.is_dir() });
    let mut problems = Vec::new();
    let setup = dir.join("setup.json");
    match std::fs::read_to_string(&setup) {
        Ok(t) => match serde_json::from_str::<SavedProfile>(&t) {
            Ok(p) => details["setup"] = json!({ "name": p.name, "saved_at": p.saved_at, "has_mic_chain": p.mic_chain.is_some(), "has_camera": p.camera_settings.is_some(), "obs_input": p.obs_input }),
            Err(e) => problems.push(format!("setup.json is damaged ({e})")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => details["setup"] = Value::Null,
        Err(e) => problems.push(format!("setup.json can't be read ({e})")),
    }
    match std::fs::read_to_string(dir.join("chain.json")) {
        Ok(t) => match serde_json::from_str::<ProcessingChain>(&t) {
            Ok(_) => details["chain_json"] = json!("ok"),
            Err(e) => problems.push(format!("chain.json is damaged ({e})")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => details["chain_json"] = Value::Null,
        Err(e) => problems.push(format!("chain.json can't be read ({e})")),
    }
    // Can we save? Write and remove a probe file (only when the folder exists).
    if dir.is_dir() {
        let probe = dir.join(".self-test-write-probe");
        let ok = std::fs::write(&probe, b"ok").is_ok();
        let _ = std::fs::remove_file(&probe);
        details["writable"] = json!(ok);
        if !ok {
            problems.push("the settings folder isn't writable".into());
        }
    }
    details["problems"] = json!(problems);
    if !problems.is_empty() {
        Outcome::new(Status::Fail, format!("Saved setup problem: {}.", problems.join("; ")), details)
    } else if details["setup"].is_null() {
        Outcome::new(Status::Pass, "No saved setup yet (normal before the first \u{201c}Save\u{201d}).", details)
    } else {
        Outcome::new(Status::Pass, "The saved setup reads back fine.", details)
    }
}

/// A neutral, fixed chain for the apply test (nothing measured, nothing written
/// to `chain.json`, so the user's live voice plugin settings don't change).
pub fn self_test_chain() -> ProcessingChain {
    ProcessingChain {
        mono_downmix: false,
        hardware_gain_change_db: 0.0,
        input_gain_db: 0.0,
        highpass_hz: 80.0,
        highpass_slope_db_oct: 12,
        eq: Vec::new(),
        expander: Expander { threshold_db: -50.0, ratio: 2.0, attack_ms: 10.0, release_ms: 100.0 },
        compressor: Compressor { threshold_db: -18.0, ratio: 3.0, attack_ms: 6.0, release_ms: 60.0, makeup_db: 0.0 },
        deesser: None,
        limiter: Limiter { ceiling_db: -1.0, release_ms: 60.0 },
        target_lufs: -16.0,
        notes: vec!["Setup Coach self-test (temporary)".into()],
    }
}

/// Which OBS outputs are active right now ("streaming", "recording").
/// A failed request counts as not active.
async fn obs_live_outputs(o: &dyn ObsApi) -> Vec<String> {
    let mut live = Vec::new();
    for (req, label) in [("GetStreamStatus", "streaming"), ("GetRecordStatus", "recording")] {
        if let Ok(v) = o.request(req, Value::Null).await {
            if v.get("outputActive").and_then(Value::as_bool) == Some(true) {
                live.push(label.to_string());
            }
        }
    }
    live
}

async fn filter_kinds(o: &dyn ObsApi) -> Result<Vec<String>, String> {
    let r = o.request("GetSourceFilterKindList", Value::Null).await?;
    Ok(r["sourceFilterKinds"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

fn is_test_name(n: &str) -> bool {
    n == TEST_SCENE || n == TEST_INPUT
}

/// Filter names per user input and scene (everything that isn't ours).
async fn user_filters(o: &dyn ObsApi) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut names: Vec<String> = Vec::new();
    let inputs = o.request("GetInputList", Value::Null).await?;
    names.extend(inputs["inputs"].as_array().into_iter().flatten().filter_map(|i| i["inputName"].as_str().map(str::to_string)));
    if let Ok(sc) = o.request("GetSceneList", Value::Null).await {
        names.extend(sc["scenes"].as_array().into_iter().flatten().filter_map(|s| s["sceneName"].as_str().map(str::to_string)));
    }
    let mut out = BTreeMap::new();
    for n in names.into_iter().filter(|n| !is_test_name(n)) {
        let f = o.request("GetSourceFilterList", json!({ "sourceName": n })).await.ok();
        let list = f
            .and_then(|f| f["filters"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|f| format!("{}|{}|{}", f["filterName"].as_str().unwrap_or(""), f["filterKind"].as_str().unwrap_or(""), f["filterEnabled"]))
            .collect();
        out.insert(n, list);
    }
    Ok(out)
}

/// Which of our two test objects exist in OBS right now.
async fn test_objects_present(o: &dyn ObsApi) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(i) = o.request("GetInputList", Value::Null).await {
        if i["inputs"].as_array().into_iter().flatten().any(|i| i["inputName"] == TEST_INPUT) {
            v.push(TEST_INPUT.to_string());
        }
    }
    if let Ok(s) = o.request("GetSceneList", Value::Null).await {
        if s["scenes"].as_array().into_iter().flatten().any(|s| s["sceneName"] == TEST_SCENE) {
            v.push(TEST_SCENE.to_string());
        }
    }
    v
}

/// Remove our test input and scene if they exist. Never touches anything else.
async fn remove_test_objects(o: &dyn ObsApi) -> Vec<String> {
    let present = test_objects_present(o).await;
    let mut removed = Vec::new();
    if present.iter().any(|n| n == TEST_INPUT) && o.request("RemoveInput", json!({ "inputName": TEST_INPUT })).await.is_ok() {
        removed.push(TEST_INPUT.to_string());
    }
    if present.iter().any(|n| n == TEST_SCENE) && o.request("RemoveScene", json!({ "sceneName": TEST_SCENE })).await.is_ok() {
        removed.push(TEST_SCENE.to_string());
    }
    removed
}

/// Run a blocking platform call off the async threads. `None` if it panicked.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

/// Run one check in its own task with a time limit; a panic or timeout
/// becomes a `fail` with a note instead of stopping the self-test.
pub async fn run_check<F>(id: &str, name: &str, limit: Duration, fut: F) -> Check
where
    F: Future<Output = Outcome> + Send + 'static,
{
    let start = Instant::now();
    let handle = tokio::spawn(fut);
    let abort = handle.abort_handle();
    let out = match tokio::time::timeout(limit, handle).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => Outcome::new(Status::Fail, "This check crashed. The details are in the report.", json!({ "crash": e.to_string() })),
        Err(_) => {
            abort.abort();
            Outcome::new(
                Status::Fail,
                format!("This check took longer than {} seconds and was stopped.", limit.as_secs_f64()),
                json!({ "timed_out": true, "limit_ms": limit.as_millis() as u64 }),
            )
        }
    };
    Check { id: id.into(), name: name.into(), status: out.status, note: out.note, details: out.details, duration_ms: start.elapsed().as_millis() as u64 }
}

fn summarize(checks: &[Check]) -> Summary {
    let n = |s: Status| checks.iter().filter(|c| c.status == s).count();
    let mut sm = Summary { pass: n(Status::Pass), warn: n(Status::Warn), fail: n(Status::Fail), skip: n(Status::Skip), overall: String::new() };
    sm.overall = if sm.fail > 0 { "fail" } else if sm.warn > 0 { "warn" } else { "pass" }.into();
    sm
}

/// The human-readable report.
pub fn render_text(r: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("Setup Coach self-test, {} (UTC)\n", r.started_at));
    s.push_str(&format!("App {} on {} {}{}\n", r.app_version, r.os, r.arch, r.windows_build.as_ref().map(|b| format!(", Windows build {b}")).unwrap_or_default()));
    s.push_str(&format!(
        "Result: {} ({} pass, {} warn, {} fail, {} skipped)\n\n",
        r.summary.overall.to_uppercase(),
        r.summary.pass,
        r.summary.warn,
        r.summary.fail,
        r.summary.skip
    ));
    for c in &r.checks {
        s.push_str(&format!("[{}] {}\n       {}\n", c.status.label(), c.name, c.note));
    }
    s.push_str("\nIn this zip:\n");
    for f in &r.bundle.files {
        s.push_str(&format!("  {f}\n"));
    }
    for n in &r.bundle.notes {
        s.push_str(&format!("  (not included: {n})\n"));
    }
    s.push_str("\nAll numbers and details are in self-test.json.\n");
    s
}

// ---------------------------------------------------------------- bundle / zip

/// Files to pack besides the reports: (path in zip, file on disk).
pub fn collect_files(data_dir: &Path, obs_dir: Option<&Path>, notes: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if data_dir.is_dir() {
        walk(data_dir, data_dir, "SetupCoach", &mut out, notes);
    } else {
        notes.push(format!("{} doesn't exist yet", data_dir.display()));
    }
    match obs_dir.map(|d| d.join("logs")) {
        Some(logs) => match newest_file(&logs) {
            Some(p) => {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "obs.log".into());
                out.push((format!("obs-logs/{name}"), p));
            }
            None => notes.push(format!("no OBS log found in {}", logs.display())),
        },
        None => notes.push("OBS settings folder unknown".into()),
    }
    out
}

fn walk(root: &Path, dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>, notes: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        notes.push(format!("couldn't list {}", dir.display()));
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(root, &p, prefix, out, notes);
            continue;
        }
        let rel = p.strip_prefix(root).unwrap_or(&p).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")) {
            continue;
        }
        if e.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            notes.push(format!("{rel} is bigger than 25 MB"));
            continue;
        }
        out.push((format!("{prefix}/{rel}"), p));
    }
}

fn newest_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|x| x.1)
}

/// `SetupCoach-selftest-20261102-142530.zip` (UTC).
pub fn zip_name(iso: &str) -> String {
    let compact: String = iso.chars().filter(|c| c.is_ascii_digit() || *c == 'T').collect();
    format!("SetupCoach-selftest-{}.zip", compact.replace('T', "-"))
}

/// Write the zip; files that can't be read are noted in `self-test.txt`/json
/// rather than failing the whole run. Returns the zip path.
pub fn write_bundle(report: &mut Report, data_dir: &Path, obs_dir: Option<&Path>, out_dir: &Path) -> Result<PathBuf, String> {
    let mut notes = Vec::new();
    let files = collect_files(data_dir, obs_dir, &mut notes);
    // Read first so unreadable files (e.g. a locked log) are known up front.
    let mut blobs = Vec::new();
    for (name, path) in files {
        match std::fs::read(&path) {
            Ok(b) => blobs.push((name, b)),
            Err(e) => notes.push(format!("{name}: {e}")),
        }
    }
    report.bundle.files = ["self-test.json", "self-test.txt"].iter().map(|s| s.to_string()).chain(blobs.iter().map(|b| b.0.clone())).collect();
    report.bundle.notes = notes;
    let json = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    let text = render_text(report);

    std::fs::create_dir_all(out_dir).map_err(|e| format!("Couldn't create {} ({e})", out_dir.display()))?;
    let path = out_dir.join(zip_name(&report.started_at));
    let file = std::fs::File::create(&path).map_err(|e| format!("Couldn't create {} ({e})", path.display()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(false);
    let mut add = |name: &str, bytes: &[u8]| -> Result<(), String> {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(bytes).map_err(|e| e.to_string())
    };
    add("self-test.json", &json)?;
    add("self-test.txt", text.as_bytes())?;
    for (name, bytes) in &blobs {
        add(name, bytes)?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(path)
}

/// Desktop, else `~/Desktop`, else the app's data folder.
pub fn default_out_dir(core: &AppCore) -> PathBuf {
    if let Some(d) = core.platform.system.desktop_dir() {
        return d;
    }
    for var in ["USERPROFILE", "HOME"] {
        if let Some(h) = std::env::var_os(var) {
            let d = PathBuf::from(h).join("Desktop");
            if d.is_dir() {
                return d;
            }
        }
    }
    core.data_dir.clone()
}

/// The whole self-test: checks, bundle, `last-self-test.txt`. Returns the zip path.
pub async fn run_self_test(core: Arc<AppCore>, opts: &CliOptions, vst3_path: Option<String>, check_timeout: Duration) -> Result<PathBuf, String> {
    let mut st = SelfTest::new(core.clone(), vst3_path);
    st.skip_obs = opts.skip_obs;
    st.no_obs_password = opts.no_obs_password;
    st.check_timeout = check_timeout;
    let st = Arc::new(st);
    let mut report = st.run().await;
    let out_dir = opts.out_dir.clone().unwrap_or_else(|| default_out_dir(&core));
    let obs_dir = core.platform.system.obs_config_dir();
    let data_dir = core.data_dir.clone();
    let zip = tokio::task::spawn_blocking(move || write_bundle(&mut report, &data_dir, obs_dir.as_deref(), &out_dir).map(|p| (p, report)))
        .await
        .map_err(|e| format!("Writing the zip crashed ({e})"))??
        .0;
    let _ = std::fs::create_dir_all(&core.data_dir);
    let _ = std::fs::write(core.data_dir.join("last-self-test.txt"), format!("{}\n", zip.display()));
    Ok(zip)
}

/// Entry point for `--self-test`; returns the process exit code
/// (0 = report written, 1 = the self-test itself failed).
pub fn cli_main(opts: CliOptions) -> i32 {
    crate::platform::attach_parent_console();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
        let res = rt.block_on(async {
            let core = AppCore::new(crate::platform::native(), crate::paths::data_dir(), Arc::new(CollectingEmitter::default()));
            core.goxlr_autodetect.store(true, std::sync::atomic::Ordering::SeqCst);
            run_self_test(Arc::new(core), &opts, crate::paths::vst3_path(), CHECK_TIMEOUT).await
        });
        // Don't wait for a stuck device call that already timed out.
        rt.shutdown_timeout(Duration::from_secs(1));
        res
    }));
    let res = result.unwrap_or_else(|_| Err("The self-test crashed.".into()));
    match res {
        Ok(zip) => {
            println!("{}", zip.display());
            eprintln!("Setup Coach self-test done. Send this file: {}", zip.display());
            0
        }
        Err(e) => {
            let dir = crate::paths::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("last-self-test.txt"), format!("FAILED: {e}\n"));
            eprintln!("Setup Coach self-test failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["app"]), None);
        assert_eq!(a(&["app", "--self-test"]), Some(CliOptions::default()));
        let o = a(&["app", "--self-test", "--out", "C:\\x y", "--skip-obs"]).unwrap();
        assert_eq!(o.out_dir, Some(PathBuf::from("C:\\x y")));
        assert!(o.skip_obs && !o.no_obs_password);
        assert_eq!(a(&["app", "--out=d", "--self-test", "--no-obs-password"]).unwrap().out_dir, Some(PathBuf::from("d")));
    }

    #[test]
    fn zip_names() {
        assert_eq!(zip_name("2026-11-02T14:25:30Z"), "SetupCoach-selftest-20261102-142530.zip");
    }

    #[tokio::test]
    async fn timeout_and_panic_become_failures() {
        let c = run_check("slow", "Slow", Duration::from_millis(50), async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Outcome::new(Status::Pass, "", Value::Null)
        })
        .await;
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.details["timed_out"], true);
        assert!(c.duration_ms < 5_000);
        let explode = c.duration_ms < u64::MAX;
        let c = run_check("boom", "Boom", Duration::from_secs(1), async move {
            if explode {
                panic!("boom");
            }
            Outcome::new(Status::Pass, "", Value::Null)
        })
        .await;
        assert_eq!(c.status, Status::Fail);
        assert!(c.details["crash"].is_string());
    }
}
