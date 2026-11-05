//! Windows "Audio enhancements" / Voice Clarity: the fix is only counted when
//! a re-read shows it is really off, and when Windows says no the user gets a
//! guided step that opens the exact page and notices the change by itself.

use coach_core::system::Action;
use setup_coach_app::app::{AppCore, CollectingEmitter, RecordingWindow};
use setup_coach_app::platform::mock::{FxMode, MockPlatform};
use setup_coach_app::platform::{is_capture_endpoint_id, sound_properties_uri};
use setup_coach_app::types::*;
use std::sync::Arc;
use std::time::Duration;

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    events: Arc<CollectingEmitter>,
    window: Arc<RecordingWindow>,
    _dir: tempfile::TempDir,
}

fn world(tweak: impl FnOnce(&MockPlatform)) -> World {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    tweak(&mock);
    let events = Arc::new(CollectingEmitter::default());
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), events.clone());
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    core.coach_tick = Duration::from_millis(5);
    let core = Arc::new(core);
    let window = Arc::new(RecordingWindow::default());
    core.set_window_control(window.clone());
    World { core, mock, events, window, _dir: dir }
}

async fn action_id(w: &World, wanted: fn(&Action) -> bool) -> String {
    let q = w.core.run_quick_checks().await;
    q.actions.iter().find(|a| wanted(&a.action)).map(|a| a.id.clone()).unwrap_or_else(|| panic!("no such action offered: {:?}", q.actions))
}

fn is_enh(a: &Action) -> bool {
    matches!(a, Action::DisableEnhancements { .. })
}
fn is_vc(a: &Action) -> bool {
    matches!(a, Action::DisableVoiceClarity { .. })
}

async fn wait_done(w: &World, secs: f64) -> bool {
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        if w.events.events.lock().unwrap().iter().any(|e| e.0 == events::COACH_UPDATE && e.1["phase"] == "done") {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn works_and_is_confirmed_by_reading_back() {
    let w = world(|_| {});
    let id = action_id(&w, is_enh).await;
    let r = w.core.apply_actions(std::slice::from_ref(&id)).await;
    assert_eq!(r.applied, vec![id]);
    assert!(r.failed.is_empty(), "{r:?}");
    assert!(!w.mock.system.windows.lock().unwrap().endpoints[0].enhancements_enabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_apply_becomes_a_guided_step_then_the_user_flips_it_and_we_notice() {
    let w = world(|m| *m.system.fx_mode.lock().unwrap() = FxMode::Denied);
    let id = action_id(&w, is_enh).await;
    let r = w.core.apply_actions(std::slice::from_ref(&id)).await;

    // Not counted as fixed; the state really is unchanged.
    assert!(r.applied.is_empty(), "{r:?}");
    assert_eq!(r.failed.len(), 1);
    assert!(w.mock.system.windows.lock().unwrap().endpoints[0].enhancements_enabled);
    assert!(!w.core.check_passes("enhancements_off").await);

    // The failure carries a ready-made step: opens the page for this mic, no "did you do it?".
    let step = r.failed[0].guided.clone().expect("guided step");
    assert_eq!(step.task, coach_core::coach::CoachTaskKind::CheckSetting);
    assert_eq!(step.check.as_deref(), Some("enhancements_off"));
    let target = step.open_target.clone().expect("open target");
    assert!(target.uri_or_app.starts_with("ms-settings:sound-"), "{}", target.uri_or_app);
    assert!(r.failed[0].error.contains("nothing was changed"), "{}", r.failed[0].error);

    // The app opens it and watches.
    w.core.open_for_user(&target).await.unwrap();
    assert_eq!(w.mock.system.opened.lock().unwrap().len(), 1);
    w.core.start_coach(step).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!wait_done(&w, 0.0).await, "must not finish before the user changes anything");

    // The user turns it off in Windows; the app notices and comes back to the front.
    w.mock.system.windows.lock().unwrap().endpoints[0].enhancements_enabled = false;
    assert!(wait_done(&w, 5.0).await);
    assert!(w.window.calls.lock().unwrap().iter().any(|c| c == "focus"));
    assert!(w.core.check_passes("enhancements_off").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_that_claims_success_but_changes_nothing_is_not_reported_fixed() {
    let w = world(|m| *m.system.fx_mode.lock().unwrap() = FxMode::ClaimsButNothing);
    let id = action_id(&w, is_enh).await;
    let r = w.core.apply_actions(&[id]).await;
    assert!(r.applied.is_empty(), "wrong 'fixed' claim: {r:?}");
    assert_eq!(r.failed.len(), 1);
    assert!(r.failed[0].guided.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_clarity_follows_the_same_rules() {
    let w = world(|m| {
        m.system.windows.lock().unwrap().endpoints[0].voice_clarity_enabled = true;
        *m.system.fx_mode.lock().unwrap() = FxMode::Denied;
    });
    let id = action_id(&w, is_vc).await;
    let r = w.core.apply_actions(std::slice::from_ref(&id)).await;
    assert!(r.applied.is_empty() && r.failed.len() == 1, "{r:?}");
    let step = r.failed[0].guided.clone().unwrap();
    assert_eq!(step.check.as_deref(), Some("voice_clarity_off"));
    assert!(!w.core.check_passes("voice_clarity_off").await);
    // Windows lets it through on a retry.
    *w.mock.system.fx_mode.lock().unwrap() = FxMode::Works;
    let r = w.core.apply_actions(std::slice::from_ref(&id)).await;
    assert_eq!(r.applied, vec![id]);
    assert!(w.core.check_passes("voice_clarity_off").await);
}

fn is_listen(a: &Action) -> bool {
    matches!(a, Action::DisableListen { .. })
}
fn is_excl(a: &Action) -> bool {
    matches!(a, Action::DisableExclusiveMode { .. })
}

fn both_on(m: &MockPlatform) {
    let mut w = m.system.windows.lock().unwrap();
    w.endpoints[0].listen_to_device = true;
    w.endpoints[0].exclusive_mode_allowed = true;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_and_exclusive_work_and_are_read_back() {
    let w = world(both_on);
    for f in [is_listen, is_excl] {
        let id = action_id(&w, f).await;
        let r = w.core.apply_actions(std::slice::from_ref(&id)).await;
        assert_eq!(r.applied, vec![id], "{r:?}");
        assert!(r.failed.is_empty(), "{r:?}");
    }
    let ep = w.mock.system.windows.lock().unwrap().endpoints[0].clone();
    assert!(!ep.listen_to_device && !ep.exclusive_mode_allowed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_and_exclusive_denied_become_guided_steps_and_complete_by_themselves() {
    let w = world(|m| {
        both_on(m);
        *m.system.fx_mode.lock().unwrap() = FxMode::Denied;
    });
    for (f, check) in [(is_listen as fn(&Action) -> bool, "listen_off"), (is_excl, "exclusive_off")] {
        let id = action_id(&w, f).await;
        let r = w.core.apply_actions(std::slice::from_ref(&id)).await;
        assert!(r.applied.is_empty(), "{r:?}");
        let step = r.failed[0].guided.clone().expect("guided step");
        assert_eq!(step.check.as_deref(), Some(check));
        assert!(!w.core.check_passes(check).await);
        assert!(step.open_target.is_some());
    }
    // The user flips "listen" in Windows; the app notices.
    let step = {
        let id = action_id(&w, is_listen).await;
        w.core.apply_actions(&[id]).await.failed[0].guided.clone().unwrap()
    };
    w.core.start_coach(step).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!wait_done(&w, 0.0).await);
    w.mock.system.windows.lock().unwrap().endpoints[0].listen_to_device = false;
    assert!(wait_done(&w, 5.0).await);
    assert!(w.core.check_passes("listen_off").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_and_exclusive_that_claim_success_but_change_nothing_are_not_fixed() {
    let w = world(|m| {
        both_on(m);
        *m.system.fx_mode.lock().unwrap() = FxMode::ClaimsButNothing;
    });
    for f in [is_listen, is_excl] {
        let id = action_id(&w, f).await;
        let r = w.core.apply_actions(&[id]).await;
        assert!(r.applied.is_empty(), "wrong 'fixed' claim: {r:?}");
        assert!(r.failed[0].guided.is_some());
    }
}

#[test]
fn settings_deep_link_targets_the_exact_device() {
    let id = "{0.0.1.00000000}.{abc4766e-8854-4d99-a2bb-8fc680147d1a}";
    assert!(is_capture_endpoint_id(id));
    assert_eq!(sound_properties_uri(id), format!("ms-settings:sound-properties?endpointId={id}"));
    // Anything that isn't a Windows capture endpoint id never reaches a URI or the elevated helper.
    for bad in ["", "{0.0.0.00000000}.{abc}", "{0.0.1.00000000}.{x}&calc", "{0.0.1.00000000}.{ab\" --x}"] {
        assert!(!is_capture_endpoint_id(bad), "{bad}");
        assert_eq!(sound_properties_uri(bad), "ms-settings:sound-defaultinputproperties");
    }
}

#[test]
fn the_elevated_helper_ignores_normal_app_starts_and_bad_input() {
    use setup_coach_app::platform::fx_helper_cli;
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(fx_helper_cli(&a(&["app.exe"])), None);
    assert_eq!(fx_helper_cli(&a(&["app.exe", "--self-test"])), None);
    #[cfg(windows)]
    {
        // Wrong shape / not an endpoint id: refuses (exit 2) without touching anything.
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "sysfx", "C:\\evil", "1"])), Some(2));
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "other", "{0.0.1.00000000}.{abc}", "1"])), Some(2));
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper"])), Some(2));
        let ep = "{0.0.1.00000000}.{abc}";
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "regflag", "run", ep, "0"])), Some(2));
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "regflag", "listen", "C:\\evil", "0"])), Some(2));
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "regflag", "listen", ep, "2"])), Some(2));
        assert_eq!(fx_helper_cli(&a(&["app.exe", "--fx-helper", "regflag", "listen", ep])), Some(2));
    }
}
