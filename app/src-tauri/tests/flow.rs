//! End-to-end app flow on the mock platform with a fake OBS.

use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::SegmentKind;
use coach_core::health::AlertKind;
use coach_core::video::TestStep;
use coach_core::Constraint;
use setup_coach_app::app::{segments_from_marks, AppCore, CollectingEmitter};
use setup_coach_app::obs::{mock::FakeObs, F_COMPRESSOR, F_EXPANDER, F_GAIN, F_LIMITER};
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::types::*;
use std::sync::Arc;
use std::time::Duration;

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    obs: Arc<FakeObs>,
    events: Arc<CollectingEmitter>,
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let events = Arc::new(CollectingEmitter::default());
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), events.clone());
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    core.health_interval = Duration::from_millis(5);
    core.frame_interval = Duration::from_millis(1);
    let core = Arc::new(core);
    let obs = Arc::new(FakeObs::typical());
    core.set_obs(obs.clone()).await;
    World { core, mock, obs, events, _dir: dir }
}

fn mic_id(w: &World) -> String {
    w.mock.audio.inputs[0].id.clone()
}

/// Record a synthetic scenario through the real capture path and mark the
/// steps at the scenario's segment times.
fn record(w: &World, s: &Scenario) {
    let test = build(s);
    *w.mock.audio.recording.lock().unwrap() = Some(test.clone());
    w.core.start_mic_test(&mic_id(w)).unwrap();
    for seg in &test.segments {
        w.core.mark_step_at(seg.kind, Edge::Start, seg.start_s).unwrap();
        w.core.mark_step_at(seg.kind, Edge::End, seg.end_s).unwrap();
    }
}

#[tokio::test]
async fn environment_lists_devices_and_obs() {
    let w = world().await;
    let env = w.core.get_environment().await;
    assert!(env.obs.connected);
    assert_eq!(env.obs.version.as_deref(), Some("32.0.1"));
    assert_eq!(env.mics.len(), 1);
    let p = env.mics[0].profile.as_ref().expect("Yeti matched");
    assert!(p.display_name.to_lowercase().contains("yeti"), "{}", p.display_name);
    assert_eq!(env.cameras[0].obs_source.as_deref(), Some("Webcam"));
    // The default mic is captured by OBS's "Mic/Aux" (device_id "default").
    assert_eq!(env.mics[0].obs_input.as_deref(), Some("Mic/Aux"));
    assert!(env.cameras[0].profile.as_ref().unwrap().display_name.contains("C920"));
    // Serializes for the UI.
    let v = serde_json::to_value(&env).unwrap();
    assert!(v["mics"][0]["profile"]["profile"]["id"].is_string());
}

#[tokio::test]
async fn quick_checks_find_and_fix_problems() {
    let w = world().await;
    let q = w.core.run_quick_checks().await;
    let ids: Vec<&str> = q.findings.iter().map(|f| f.id.as_str()).collect();
    assert!(!q.findings.is_empty());
    assert!(!q.actions.is_empty(), "findings {ids:?} gave no actions");
    assert_eq!(q.score.len(), 4);
    // AI noise suppression in OBS gets a removal action; Windows ducking a registry fix.
    let rm = q.actions.iter().find(|a| a.label.contains("Noise Suppression")).expect("remove RNNoise action");
    let duck = q.actions.iter().find(|a| matches!(a.action, coach_core::system::Action::SetWindowsDucking { .. })).expect("ducking");
    let all: Vec<String> = q.actions.iter().map(|a| a.id.clone()).collect();
    let res = w.core.apply_actions(&all).await;
    assert!(res.applied.contains(&rm.id) && res.applied.contains(&duck.id), "{res:?}");
    assert!(!w.obs.filter_names("Mic/Aux").contains(&"Noise Suppression".to_string()));
    // Things that need OBS closed come back as a friendly failure, never a panic.
    for f in &res.failed {
        assert!(!f.error.is_empty());
    }
    // A second run no longer reports what was fixed.
    let q2 = w.core.run_quick_checks().await;
    assert!(q2.findings.len() < q.findings.len(), "{} -> {}", q.findings.len(), q2.findings.len());
    assert!(w.core.apply_actions(&["nope".into()]).await.failed.len() == 1);
}

#[tokio::test]
async fn mic_test_report_rescan_apply_verify() {
    let w = world().await;
    let ctx = w.core.mic_context(&mic_id(&w), &MicAnswers::default()).unwrap();
    assert_eq!(ctx.address, Some(coach_core::audio::findings::Address::Side));
    record(&w, &Scenario { speech_level_dbfs: -40.0, rt60_s: 0.9, reverb_wet_db: 0.0, ..Default::default() });
    assert!(w.events.count(events::MIC_LEVEL) > 100, "level events: {}", w.events.count(events::MIC_LEVEL));
    let report = w.core.finish_mic_test(ctx.clone(), vec![]).unwrap();
    assert!(report.findings.iter().any(|f| f.id == "mic.too_far"), "{:?}", report.findings.iter().map(|f| &f.id).collect::<Vec<_>>());

    // "I can't move the mic closer" → recalibrated, finding downgraded.
    let again = w.core.rescan_mic(ctx, vec![Constraint::CantMoveMicCloser]).unwrap();
    let far = again.findings.iter().find(|f| f.id == "mic.too_far").unwrap();
    assert_eq!(far.severity, coach_core::Severity::Info);

    // Apply: chain.json written, OBS filters created in order (VST3 host missing → friendly failure).
    // An unknown input name is resolved to the OBS input capturing the tested mic.
    let res = w.core.apply_mic_chain(again.chain.clone(), "").await;
    assert!(res.applied.contains(&"chain_json".to_string()));
    assert!(res.failed.iter().any(|f| f.id == "vst3" && f.error.contains("atkAudio")));
    let names = w.obs.filter_names("Mic/Aux");
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap_or_else(|| panic!("{n} missing in {names:?}"));
    assert!(pos(F_GAIN) < pos(F_EXPANDER) && pos(F_EXPANDER) < pos(F_COMPRESSOR) && pos(F_COMPRESSOR) < pos(F_LIMITER));
    let written: coach_core::audio::ProcessingChain = serde_json::from_str(&std::fs::read_to_string(w.core.data_dir.join("chain.json")).unwrap()).unwrap();
    assert_eq!(written, again.chain);

    let v = w.core.verify_mic().unwrap();
    assert!(v.after.metrics.speech_peak_dbfs > v.before.metrics.speech_peak_dbfs + 6.0);

    w.core.play_preview(PreviewWhich::After, true).unwrap();
    assert_eq!(*w.mock.audio.played.lock().unwrap(), 1);
    let before = w.core.preview_buffer(PreviewWhich::Before, false).unwrap();
    assert!(before.duration_s() > 9.0);
}

#[tokio::test]
async fn left_only_without_vst3_needs_obs_closed_for_mono() {
    let w = world().await;
    record(&w, &Scenario { left_only: true, ..Default::default() });
    let r = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    assert!(r.chain.mono_downmix);
    let res = w.core.apply_mic_chain(r.chain.clone(), "Mic/Aux").await;
    assert!(res.restart_obs_needed);
    assert!(res.failed.iter().any(|f| f.id == "mono" && f.error.contains("Close OBS")));
}

#[tokio::test]
async fn mic_test_errors_are_friendly() {
    let w = world().await;
    assert!(w.core.finish_mic_test(Default::default(), vec![]).unwrap_err().contains("Start"));
    assert!(w.core.start_mic_test("missing").is_err());
    // A test without a reading step is rejected.
    *w.mock.audio.recording.lock().unwrap() = Some(build(&Scenario::default()));
    w.core.start_mic_test(&mic_id(&w)).unwrap();
    w.core.mark_step_at(SegmentKind::Silence, Edge::Start, 0.0).unwrap();
    w.core.mark_step_at(SegmentKind::Silence, Edge::End, 3.0).unwrap();
    assert!(w.core.finish_mic_test(Default::default(), vec![]).unwrap_err().contains("reading"));
}

#[test]
fn marks_pair_into_segments() {
    let m = vec![
        (SegmentKind::Silence, Edge::Start, 0.0),
        (SegmentKind::Silence, Edge::End, 5.0),
        (SegmentKind::Read, Edge::Start, 5.5),
        (SegmentKind::Read, Edge::End, 15.0),
        (SegmentKind::Loud, Edge::Start, 16.0),
    ];
    let s = segments_from_marks(&m, 18.0);
    assert_eq!(s.len(), 3);
    assert_eq!(s[2].kind, SegmentKind::Loud);
    assert_eq!(s[2].end_s, 18.0);
}

#[tokio::test]
async fn camera_flow() {
    let w = world().await;
    let cam = w.mock.camera.devices[0].id.clone();
    w.core.start_camera_test(&cam).await.unwrap();
    for (step, secs) in [(TestStep::Neutral, 1.0), (TestStep::TurnHead, 1.0), (TestStep::HoldPaper, 1.0), (TestStep::LeanInOut, 1.0)] {
        let r = w.core.capture_camera_step(step, secs).await.unwrap();
        assert_eq!(r.frames, 4);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(w.events.count(events::CAMERA_PREVIEW) >= 1);
    let report = w.core.finish_camera_test(vec![]).unwrap();
    // Frames are kept: finishing again with a constraint recalibrates.
    let again = w.core.finish_camera_test(vec![Constraint::CantAddLight]).unwrap();
    assert_eq!(again.metrics, report.metrics);
    assert_eq!(report.class, coach_core::video::CameraClass::FullUvc);
    let uvc = report.settings.uvc.clone().expect("webcam gets UVC settings");
    assert!(!uvc.auto_exposure && !uvc.auto_wb);
    assert_eq!(uvc.powerline_hz, Some(50));
    let res = w.core.apply_camera_settings(&cam, report.settings.clone()).await;
    assert!(res.applied.contains(&"camera_controls".to_string()), "{res:?}");
    assert!(res.applied.contains(&"obs_camera_mode".to_string()), "{res:?}");
    assert!(!w.mock.camera.state.lock().unwrap().auto_exposure);
    let webcam = w.obs.state.lock().unwrap().inputs.iter().find(|i| i.name == "Webcam").unwrap().settings.clone();
    assert_eq!(webcam["res_type"], 1);
    serde_json::to_string(&report).unwrap();
}

#[tokio::test]
async fn health_monitor_alerts_and_report() {
    let w = world().await;
    w.core.start_health_monitor().await.unwrap();
    // Healthy for a bit, then heavy network loss.
    for i in 0..40 {
        if i < 10 {
            w.obs.advance(2.0, 0.0, 0.0, 6000.0);
        } else {
            w.obs.advance(2.0, 0.0, 0.08, 3000.0);
        }
        tokio::time::sleep(Duration::from_millis(8)).await;
    }
    let report = w.core.stop_health_monitor().unwrap();
    assert!(w.events.count(events::HEALTH_SAMPLE) > 5);
    let alerts: Vec<serde_json::Value> = w.events.events.lock().unwrap().iter().filter(|e| e.0 == events::HEALTH_ALERT).map(|e| e.1.clone()).collect();
    assert!(alerts.iter().any(|a| a["kind"] == serde_json::json!(AlertKind::Network)), "{alerts:?}");
    serde_json::to_string(&report).unwrap();
    assert!(w.core.stop_health_monitor().is_err());
}

#[tokio::test]
async fn save_and_reapply_setup() {
    let w = world().await;
    record(&w, &Scenario::default());
    let r = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    w.core.apply_mic_chain(r.chain.clone(), "Mic/Aux").await;
    let cam = w.mock.camera.devices[0].id.clone();
    w.core.start_camera_test(&cam).await.unwrap();
    w.core.capture_camera_step(TestStep::HoldPaper, 1.0).await.unwrap();
    let cr = w.core.finish_camera_test(vec![]).unwrap();
    w.core.apply_camera_settings(&cam, cr.settings.clone()).await;
    let saved = w.core.save_setup("Main").unwrap();
    assert_eq!(saved.name, "Main");
    assert_eq!(w.core.load_setup().unwrap(), saved);
    // Camera "forgets" its settings (replug) → re-apply restores them.
    w.mock.camera.state.lock().unwrap().auto_exposure = true;
    let res = w.core.reapply_saved_setup().await;
    assert!(res.applied.contains(&"camera_controls".to_string()), "{res:?}");
    assert!(!w.mock.camera.state.lock().unwrap().auto_exposure);
    let env = w.core.get_environment().await;
    assert_eq!(env.saved_profile.unwrap().name, "Main");
}
