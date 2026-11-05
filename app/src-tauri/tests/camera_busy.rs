//! Camera with no usable picture (another app holds it): nothing is analysed,
//! the app says who has it, frees it from OBS and puts it back; mains frequency.

use coach_core::video::TestStep;
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::obs::mock::FakeObs;
use setup_coach_app::platform::mock::{MockPicture, MockPlatform};
use setup_coach_app::platform::{CameraUser, MainsSource};
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

async fn world(with_obs: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let events = Arc::new(CollectingEmitter::default());
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), events.clone());
    core.frame_interval = Duration::from_millis(1);
    core.grab_timeout = Duration::from_millis(150);
    let core = Arc::new(core);
    let obs = Arc::new(FakeObs::typical());
    if with_obs {
        core.set_obs(obs.clone()).await;
    } else {
        // Never reach for a real OBS on the machine running the tests.
        *mock.system.obs_running.lock().unwrap() = false;
    }
    World { core, mock, obs, events, _dir: dir }
}

fn cam(w: &World) -> String {
    w.mock.camera.devices[0].id.clone()
}

fn set_picture(w: &World, p: MockPicture) {
    *w.mock.camera.picture.lock().unwrap() = p;
}

async fn run_steps(w: &World) {
    for (step, secs) in [(TestStep::Neutral, 2.0), (TestStep::TurnHead, 2.0), (TestStep::HoldPaper, 2.0), (TestStep::LeanInOut, 2.0)] {
        w.core.capture_camera_step(step, secs).await.unwrap();
    }
}

#[tokio::test]
async fn black_frames_never_start_a_test_or_make_a_report() {
    let w = world(false).await;
    set_picture(&w, MockPicture::Black);
    let err = w.core.start_camera_test(&cam(&w)).await.unwrap_err();
    assert_eq!(err.code, CameraErrorCode::NoPicture);
    assert_eq!(err.reason, Some(PictureReason::Black));
    assert_eq!(w.events.count(events::CAMERA_PREVIEW), 0, "no black preview");
    // Nothing was captured, so there is nothing to analyse.
    assert!(w.core.finish_camera_test(vec![]).is_err());
}

#[tokio::test]
async fn picture_lost_mid_test_stops_the_step_and_the_report() {
    let w = world(false).await;
    w.core.start_camera_test(&cam(&w)).await.unwrap();
    w.core.capture_camera_step(TestStep::Neutral, 1.0).await.unwrap();
    set_picture(&w, MockPicture::Black);
    let err = w.core.capture_camera_step(TestStep::TurnHead, 1.0).await.unwrap_err();
    assert_eq!(err.code, CameraErrorCode::NoPicture);
    // The report only ever sees the 4 live frames, never the black ones.
    assert!(w.core.finish_camera_test(vec![]).is_ok());
    assert!(w.events.events.lock().unwrap().iter().any(|e| e.0 == events::CAMERA_PICTURE && e.1["live"] == false));
}

#[tokio::test]
async fn open_failure_and_hang_are_no_picture_too() {
    let w = world(false).await;
    set_picture(&w, MockPicture::Fail("busy".into()));
    let e = w.core.start_camera_test(&cam(&w)).await.unwrap_err();
    assert_eq!((e.code, e.reason), (CameraErrorCode::NoPicture, Some(PictureReason::CantOpen)));
    set_picture(&w, MockPicture::Hang(1500));
    let e = w.core.start_camera_test(&cam(&w)).await.unwrap_err();
    assert_eq!((e.code, e.reason), (CameraErrorCode::NoPicture, Some(PictureReason::Timeout)));
}

#[tokio::test]
async fn obs_holding_the_camera_is_found_freed_and_given_back() {
    let w = world(true).await;
    // OBS has the camera open and shows black; so does our own read.
    w.obs.state.lock().unwrap().black_screenshot = true;
    set_picture(&w, MockPicture::Black);
    *w.mock.system.camera_users.lock().unwrap() = vec![CameraUser { name: "OBS Studio".into(), confirmed: true }];

    let st = w.core.camera_picture_status(&cam(&w)).await;
    assert!(!st.live);
    assert_eq!(st.reason, Some(PictureReason::Black));
    assert_eq!(st.users[0].name, "OBS Studio");
    assert_eq!(st.obs.source.as_deref(), Some("Webcam"));
    assert!(st.obs.holding && st.obs.can_free, "{st:?}");

    let freed = w.core.free_camera(&cam(&w)).await;
    assert!(freed.ok, "{freed:?}");
    assert!(w.obs.state.lock().unwrap().inactive.contains("Webcam"));
    // The change is in the settings vault.
    let backups = w.core.list_backups();
    assert!(backups.iter().any(|b| b.items.iter().any(|i| i.kind == "obs_camera_active")), "{backups:?}");

    // The camera is ours now: a live picture, and the test runs.
    set_picture(&w, MockPicture::Live);
    w.obs.state.lock().unwrap().black_screenshot = false;
    let st = w.core.camera_picture_status(&cam(&w)).await;
    assert!(st.live, "{st:?}");
    assert_eq!(st.freed_source.as_deref(), Some("Webcam"));
    w.core.start_camera_test(&cam(&w)).await.unwrap();
    run_steps(&w).await;
    let report = w.core.finish_camera_test(vec![]).unwrap();
    assert!(!report.findings.is_empty() || report.settings.uvc.is_some());

    // Give it back to OBS.
    let back = w.core.restore_freed_camera().await;
    assert!(back.ok, "{back:?}");
    assert!(!w.obs.state.lock().unwrap().inactive.contains("Webcam"));
    assert!(w.core.camera_picture_status(&cam(&w)).await.freed_source.is_none());
    // Nothing left to give back the second time.
    assert!(w.core.restore_freed_camera().await.ok);
    assert!(!w.obs.state.lock().unwrap().inactive.contains("Webcam"));
}

#[tokio::test]
async fn vault_restore_also_gives_the_camera_back() {
    let w = world(true).await;
    set_picture(&w, MockPicture::Black);
    w.obs.state.lock().unwrap().black_screenshot = true;
    assert!(w.core.free_camera(&cam(&w)).await.ok);
    let id = w.core.list_backups().into_iter().find(|b| b.items.iter().any(|i| i.kind == "obs_camera_active")).unwrap().id;
    let r = w.core.restore_backup(&id).await;
    assert!(r.failed.is_empty(), "{r:?}");
    assert!(!w.obs.state.lock().unwrap().inactive.contains("Webcam"));
}

#[tokio::test]
async fn freeing_without_obs_or_when_obs_is_not_the_holder_explains_itself() {
    let w = world(false).await;
    set_picture(&w, MockPicture::Black);
    let r = w.core.free_camera(&cam(&w)).await;
    assert!(!r.ok);
    assert!(r.message.contains("Close OBS"), "{}", r.message);
    // OBS is connected but its source is already released: something else holds the camera.
    let w = world(true).await;
    w.obs.state.lock().unwrap().inactive.insert("Webcam".into());
    let r = w.core.free_camera(&cam(&w)).await;
    assert!(!r.ok && r.message.contains("Another app"), "{}", r.message);
    assert!(w.obs.state.lock().unwrap().inactive.contains("Webcam"), "we pressed nothing");
}

#[tokio::test]
async fn mains_follows_the_pc_and_can_be_overridden() {
    let w = world(false).await;
    let info = w.core.mains_info();
    assert_eq!((info.hz, info.source), (50, MainsSource::TimeZone));
    w.core.start_camera_test(&cam(&w)).await.unwrap();
    run_steps(&w).await;
    let report = w.core.finish_camera_test(vec![]).unwrap();
    assert_eq!(report.settings.uvc.as_ref().unwrap().powerline_hz, Some(50), "never a hard-coded 60");

    let info = w.core.set_mains(Some(60)).unwrap();
    assert_eq!((info.hz, info.source, info.detected_hz), (60, MainsSource::Override, 50));
    let report = w.core.finish_camera_test(vec![]).unwrap();
    assert_eq!(report.settings.uvc.as_ref().unwrap().powerline_hz, Some(60));

    assert!(w.core.set_mains(Some(55)).is_err());
    let info = w.core.set_mains(None).unwrap();
    assert_eq!((info.hz, info.source), (50, MainsSource::TimeZone));
}
