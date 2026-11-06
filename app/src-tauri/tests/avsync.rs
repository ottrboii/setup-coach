//! Lip-sync clap test on the mock platform, and the pre-stream camera runtime warning.

use coach_core::system::{ConnectionState, UsbLink, UsbRole};
use setup_coach_app::app::{AppCore, CollectingEmitter, DriftVerdictView};
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::types::*;
use std::sync::Arc;
use std::time::Duration;

fn core() -> (Arc<AppCore>, MockPlatform, Arc<CollectingEmitter>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    *mock.system.obs_running.lock().unwrap() = false;
    let events = Arc::new(CollectingEmitter::default());
    let core = Arc::new(AppCore::new(mock.platform(), dir.path().join("data"), events.clone()));
    (core, mock, events, dir)
}

#[tokio::test]
async fn clap_test_runs_in_mock_mode_and_never_crashes_on_no_claps() {
    let (core, mock, events, _d) = core();
    assert!(core.finish_sync_test().await.is_err(), "finish before start is a plain error");
    let mic = mock.audio.inputs[0].id.clone();
    let cam = mock.camera.devices[0].id.clone();
    core.start_sync_test(&mic, &cam).await.unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    let r = core.finish_sync_test().await.unwrap();
    // The mock mic has no claps in it: the answer is "unclear" with a reason, never a made-up number.
    assert!(r.fix.is_none() || r.offset_ms.is_some());
    if r.offset_ms.is_none() {
        assert!(r.hint.is_some());
    }
    assert!(events.count(events::SYNC_PROGRESS) > 0, "live meter events were sent");
    // "I can't do this": stopping is always fine, also twice.
    core.stop_sync_test();
    core.stop_sync_test();
}

#[tokio::test]
async fn drift_check_runs_in_mock_mode_and_never_guesses() {
    let (core, mock, events, _d) = core();
    assert!(core.drift_status().is_none());
    assert!(core.finish_drift_clap().await.is_err(), "finish before start is a plain error");
    assert!(core.start_drift_clap().is_err());
    let mic = mock.audio.inputs[0].id.clone();
    let cam = mock.camera.devices[0].id.clone();
    // A live mock mic (quiet noise, real time) so samples keep arriving while the check runs.
    let noise: Vec<f32> = (0..48_000u32).map(|i| ((i.wrapping_mul(2654435761) >> 16) % 200) as f32 / 100_000.0).collect();
    *mock.audio.live.lock().unwrap() = Some(setup_coach_app::platform::mock::LiveMock {
        source: Arc::new(noise),
        sample_rate: 48_000,
        gain_db: Arc::new(std::sync::Mutex::new(0.0)),
        offset_db: 0.0,
        speed: 1.0,
    });
    let s = core.start_drift_check(&mic, &cam, 0.0).await.unwrap();
    assert!(s.clap_open && s.points.is_empty());
    assert_eq!(s.interval_s, 1.0, "interval is clamped to the shortest wait");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = core.finish_drift_clap().await.unwrap();
    assert!(!s.clap_open);
    // The mock mic has no claps: no point, a plain hint, and no fixed offset offered.
    if s.points.is_empty() {
        assert!(s.last.as_ref().unwrap().hint.is_some());
    }
    assert_eq!(s.report.verdict, DriftVerdictView::Unclear);
    assert!(!s.report.fixed_offset_ok);
    assert!(!core.apply_sync_fix().await.ok, "no fixed offset before the offset is shown to hold");
    assert!(core.finish_drift_clap().await.is_err(), "no clap window open");
    core.start_drift_clap().unwrap();
    assert!(core.drift_status().unwrap().clap_open);
    assert!(events.count(events::SYNC_PROGRESS) > 0);
    // Starting the clap test ends the drift check; stopping is safe twice.
    core.start_sync_test(&mic, &cam).await.unwrap();
    assert!(core.drift_status().is_none());
    core.stop_sync_test();
    core.stop_drift_check();
    core.stop_drift_check();
}

#[tokio::test]
async fn camera_close_to_its_usual_cutoff_is_warned_about_before_streaming() {
    let (core, mock, _e, _d) = core();
    let link = |connected| UsbLink { name: "Sony a6000".into(), role: UsbRole::Camera, run_minutes: vec![27, 30, 29], connected_min: connected, ..Default::default() };
    *mock.system.connection.lock().unwrap() = ConnectionState { links: vec![link(Some(5))], ..Default::default() };
    assert!(core.run_quick_checks().await.runtime_warnings.is_empty());
    *mock.system.connection.lock().unwrap() = ConnectionState { links: vec![link(Some(25))], ..Default::default() };
    let q = core.run_quick_checks().await;
    assert_eq!(q.runtime_warnings.len(), 1);
    assert_eq!(q.runtime_warnings[0].minutes_left, 3);
    // The long-standing finding about the pattern is still there too.
    assert!(q.findings.iter().any(|f| f.id == "connection.camera_switches_off_after_a_while"));
}
