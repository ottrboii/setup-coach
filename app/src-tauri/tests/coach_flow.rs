//! Live coaching end to end on the mock platform: live mock mic whose gain
//! responds to our changes, fake GoXLR Utility, settings checks, camera.

use coach_core::audio::findings::Capsule;
use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::SegmentKind;
use coach_core::coach::CoachTaskKind;
use serde_json::Value;
use setup_coach_app::app::{AppCore, CollectingEmitter, RecordingWindow};
use setup_coach_app::goxlr::{FakeGoXlr, GoXlrApi, MicType};
use setup_coach_app::obs::mock::FakeObs;
use setup_coach_app::platform::mock::{LiveMock, MockAudio, MockPlatform};
use setup_coach_app::platform::AudioDevice;
use setup_coach_app::types::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    events: Arc<CollectingEmitter>,
    window: Arc<RecordingWindow>,
    _dir: tempfile::TempDir,
}

fn mic(name: &str) -> AudioDevice {
    AudioDevice {
        id: "{0.0.1.00000000}.{mock-yeti}".into(),
        name: name.into(),
        is_default: true,
        is_default_comms: true,
        channels: 1,
        sample_rate: 48_000,
        usb: None,
    }
}

async fn world_with(mic_name: &str, tweak: impl FnOnce(&mut MockPlatform)) -> World {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = MockPlatform::demo();
    mock.audio = Arc::new(MockAudio { inputs: vec![mic(mic_name)], recording: Mutex::new(None), played: Mutex::new(0), live: Mutex::new(None) });
    tweak(&mut mock);
    let events = Arc::new(CollectingEmitter::default());
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), events.clone());
    core.coach_tick = Duration::from_millis(5);
    core.frame_interval = Duration::from_millis(1);
    let core = Arc::new(core);
    core.set_obs(Arc::new(FakeObs::typical())).await;
    let window = Arc::new(RecordingWindow::default());
    core.set_window_control(window.clone());
    World { core, mock, events, window, _dir: dir }
}

fn speech_source() -> Arc<Vec<f32>> {
    let t = build(&Scenario::default());
    let seg = t.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
    let src = &t.audio.channels[0][t.range(seg)];
    let peak = src.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    Arc::new(src.iter().map(|v| v / peak * 0.5).collect())
}

/// Run a scan so the app has a mic report + context.
fn scan(w: &World, s: &Scenario, ctx: coach_core::audio::MicContext) -> coach_core::audio::MicReport {
    let test = build(s);
    let id = w.mock.audio.inputs[0].id.clone();
    *w.mock.audio.recording.lock().unwrap() = Some(test.clone());
    w.core.start_mic_test(&id).unwrap();
    for seg in &test.segments {
        w.core.mark_step_at(seg.kind, Edge::Start, seg.start_s).unwrap();
        w.core.mark_step_at(seg.kind, Edge::End, seg.end_s).unwrap();
    }
    w.core.finish_mic_test(ctx, vec![]).unwrap()
}

fn updates(w: &World) -> Vec<Value> {
    w.events.events.lock().unwrap().iter().filter(|e| e.0 == events::COACH_UPDATE).map(|e| e.1.clone()).collect()
}

async fn wait_done(w: &World, secs: f64) -> bool {
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        if updates(w).iter().any(|u| u["phase"] == "done") {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_gain_via_windows_level_lands_in_the_green() {
    let w = world_with("Microphone (HyperX SoloCast)", |_| {}).await;
    let ctx = w.core.mic_context(&w.mock.audio.inputs[0].id, &MicAnswers::default()).unwrap();
    let report = scan(&w, &Scenario { speech_level_dbfs: -44.0, ..Default::default() }, ctx);
    assert!(report.findings.iter().any(|f| f.id == "mic.too_quiet"));
    let plan = w.core.coach_plan(CoachArea::Mic).await.unwrap();
    let gain = plan.iter().find(|s| s.task == CoachTaskKind::SetGain).expect("set-gain step").clone();
    assert!(gain.auto, "SoloCast's Windows level is its real gain");
    assert_eq!(gain.animation, "knob");

    *w.mock.audio.live.lock().unwrap() = Some(LiveMock {
        source: speech_source(),
        sample_rate: 48_000,
        gain_db: w.mock.system.mic_gain_db.clone(),
        offset_db: -38.0,
        speed: 20.0,
    });
    w.core.start_coach(gain).await.unwrap();
    assert!(wait_done(&w, 20.0).await, "never finished: {:?}", updates(&w).iter().rev().take(3).collect::<Vec<_>>());
    let g = *w.mock.system.mic_gain_db.lock().unwrap();
    assert!(g > 12.0 && g < 30.0, "gain {g}");
    let ups = updates(&w);
    assert!(ups.iter().any(|u| u["auto_adjusting"] == true));
    assert!(ups.iter().any(|u| u["phase"] == "hold"));
    // Values never overshoot the zone on the way up.
    assert!(ups.iter().filter_map(|u| u["value"].as_f64()).all(|v| v <= -10.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goxlr_utility_sets_gain_and_phantom() {
    let w = world_with("Chat Mic (TC-Helicon GoXLR)", |_| {}).await;
    let link = Arc::new(Mutex::new(30.0));
    let mut fake = FakeGoXlr::with_device("S220202153DI7", MicType::Dynamic, 30.0);
    fake.gain_link = Some(link.clone());
    let fake = Arc::new(fake);
    w.core.set_goxlr(Some(fake.clone() as Arc<dyn GoXlrApi>)).await;

    // GoXLR tells us the real gain range.
    let ctx = w.core.mic_context_async(&w.mock.audio.inputs[0].id, &MicAnswers::default()).await.unwrap();
    assert_eq!(ctx.interface_max_gain_db, Some(72.0));
    assert_eq!(ctx.gain_at_max, Some(false));

    // A condenser (NT1-A) on a GoXLR in Dynamic mode → phantom step, done by us.
    let ctx = coach_core::audio::MicContext { capsule: Some(Capsule::Condenser), ..ctx };
    scan(&w, &Scenario { speech_level_dbfs: -44.0, ..Default::default() }, ctx);
    let plan = w.core.coach_plan(CoachArea::Mic).await.unwrap();
    let phantom = plan.iter().find(|s| s.id == "check-goxlr-phantom").expect("phantom step").clone();
    assert!(phantom.auto);
    assert_eq!(plan[0].id, "check-goxlr-phantom", "settings come before physical coaching");
    w.core.start_coach(phantom).await.unwrap();
    assert!(wait_done(&w, 5.0).await);
    assert_eq!(fake.devices().await.unwrap()[0].mic_type, MicType::Condenser);
    assert!(w.window.calls.lock().unwrap().contains(&"focus".to_string()));

    // Auto gain through the Utility.
    let gain = plan.iter().find(|s| s.task == CoachTaskKind::SetGain).expect("gain step").clone();
    assert!(gain.auto);
    w.events.events.lock().unwrap().clear();
    *w.mock.audio.live.lock().unwrap() = Some(LiveMock { source: speech_source(), sample_rate: 48_000, gain_db: link.clone(), offset_db: -72.0, speed: 20.0 });
    w.core.start_coach(gain).await.unwrap();
    assert!(wait_done(&w, 20.0).await, "{:?}", updates(&w).iter().rev().take(3).collect::<Vec<_>>());
    let d = fake.devices().await.unwrap();
    assert!(d[0].mic_gain_db > 40.0, "gain {}", d[0].mic_gain_db);
    assert_eq!(d[0].mic_type, MicType::Condenser, "gain changes keep the mic type");
    assert!(fake.commands.lock().unwrap().iter().any(|c| c.to_string().contains("SetMicrophoneGain")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_check_opens_windows_and_comes_back() {
    let w = world_with("Microphone (Yeti Stereo Microphone)", |_| {}).await;
    w.mock.system.windows.lock().unwrap().mic_privacy_desktop_allowed = false;
    w.core.run_quick_checks().await;
    let plan = w.core.coach_plan(CoachArea::Mic).await.unwrap();
    let step = plan.iter().find(|s| s.id == "check-mic-privacy").expect("privacy step").clone();
    let target = step.open_target.clone().unwrap();
    assert_eq!(target.uri_or_app, "ms-settings:privacy-microphone");
    assert!(!target.steps.is_empty());
    w.core.open_for_user(&target).await.unwrap();
    assert_eq!(w.mock.system.opened.lock().unwrap().len(), 1);
    assert!(w.window.calls.lock().unwrap().contains(&"mini:true".to_string()));
    w.core.start_coach(step).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!updates(&w).iter().any(|u| u["phase"] == "done"));
    assert!(updates(&w).iter().any(|u| u["phase"] == "working"));
    w.mock.system.windows.lock().unwrap().mic_privacy_desktop_allowed = true;
    assert!(wait_done(&w, 5.0).await);
    let calls = w.window.calls.lock().unwrap().clone();
    assert!(calls.ends_with(&["mini:false".to_string(), "focus".to_string()]) || calls.contains(&"focus".to_string()), "{calls:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_goxlr_app_gets_a_switch_step() {
    let w = world_with("Chat Mic (TC-Helicon GoXLR)", |m| {
        Arc::get_mut(&mut m.system).unwrap().processing.goxlr_app_running = true;
    })
    .await;
    let q = w.core.run_quick_checks().await;
    assert!(q.findings.iter().any(|f| f.id == "system.goxlr_official_app"));
    let plan = w.core.coach_plan(CoachArea::Mic).await.unwrap();
    let s = &plan[0];
    assert_eq!(s.id, "check-goxlr-utility");
    assert!(s.open_target.as_ref().unwrap().uri_or_app.contains("goxlr-utility"));
    assert_eq!(s.check.as_deref(), Some("goxlr_utility_running"));
    // Once the Utility answers, the check passes.
    assert!(!w.core.check_passes("goxlr_utility_running").await);
    w.core.set_goxlr(Some(Arc::new(FakeGoXlr::with_device("S1", MicType::Dynamic, 40.0)))).await;
    assert!(w.core.check_passes("goxlr_utility_running").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn camera_coaching_streams_overlay_and_finishes() {
    let w = world_with("Microphone (Yeti Stereo Microphone)", |_| {}).await;
    let mk = |task: CoachTaskKind, anim: &str| CoachStep {
        id: anim.into(),
        finding_id: None,
        task,
        title: String::new(),
        explain: String::new(),
        animation: anim.into(),
        auto: false,
        open_target: None,
        check: None,
    };
    // The mock room is a little dim: live guidance asks for light.
    w.core.start_coach(mk(CoachTaskKind::LightFace, "light_move")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    w.core.stop_coach();
    assert!(updates(&w).iter().any(|u| u["hint"]["kind"] == "add_light"), "{:?}", updates(&w).last());
    // Framing in the mock is already right: it holds and finishes.
    w.events.events.lock().unwrap().clear();
    w.core.start_coach(mk(CoachTaskKind::Framing, "framing")).await.unwrap();
    assert!(wait_done(&w, 10.0).await, "{:?}", updates(&w).iter().rev().take(2).collect::<Vec<_>>());
    let overlays = w.events.count(events::COACH_CAMERA_OVERLAY);
    assert!(overlays >= 4, "overlays {overlays}");
    let o = w.events.events.lock().unwrap().iter().find(|e| e.0 == events::COACH_CAMERA_OVERLAY).unwrap().1.clone();
    assert!(o["frame"].as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    assert!(o["face"].is_object());
    assert!(o["face_ok"].is_boolean());
    assert!(o["arrows"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flicker_coaching_fixes_anti_flicker_itself() {
    let w = world_with("Microphone (Yeti Stereo Microphone)", |_| {}).await;
    assert_eq!(w.mock.camera.state.lock().unwrap().powerline_hz, Some(60));
    let step = CoachStep {
        id: "flicker".into(),
        finding_id: Some("camera.anti_flicker_wrong".into()),
        task: CoachTaskKind::Flicker,
        title: "Stop the flicker".into(),
        explain: String::new(),
        animation: "flicker".into(),
        auto: true,
        open_target: None,
        check: None,
    };
    w.core.start_coach(step).await.unwrap();
    assert!(wait_done(&w, 15.0).await, "{:?}", updates(&w).iter().rev().take(2).collect::<Vec<_>>());
    assert_eq!(w.mock.camera.state.lock().unwrap().powerline_hz, Some(50), "matched to the 50 Hz mains");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn camera_plan_from_report_and_stop() {
    let w = world_with("Microphone (Yeti Stereo Microphone)", |_| {}).await;
    let cam = w.mock.camera.devices[0].id.clone();
    w.core.start_camera_test(&cam).await.unwrap();
    for s in [coach_core::video::TestStep::Neutral, coach_core::video::TestStep::HoldPaper] {
        w.core.capture_camera_step(s, 1.0).await.unwrap();
    }
    let report = w.core.finish_camera_test(vec![]).unwrap();
    let plan = w.core.coach_plan(CoachArea::Camera).await.unwrap();
    // Every planned task comes from a finding in the report.
    for s in &plan {
        assert!(s.finding_id.as_ref().is_none_or(|f| report.findings.iter().any(|x| &x.id == f)), "{s:?}");
    }
    // Stopping a session stops its updates.
    let step = CoachStep {
        id: "framing".into(),
        finding_id: None,
        task: CoachTaskKind::SetGain,
        title: "Set your level".into(),
        explain: String::new(),
        animation: "knob".into(),
        auto: false,
        open_target: None,
        check: None,
    };
    *w.mock.audio.live.lock().unwrap() = Some(LiveMock { source: speech_source(), sample_rate: 48_000, gain_db: Arc::new(Mutex::new(0.0)), offset_db: -60.0, speed: 5.0 });
    w.core.start_coach(step).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.core.stop_coach();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let n = updates(&w).len();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(updates(&w).len(), n, "updates after stop");
}
