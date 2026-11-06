//! "Fix these for me" must be judged by a re-scan, per item, on the mic the
//! user actually picked (not whatever Windows has as default), and the fresh
//! scan must be what the screen draws (no stale ids that make fixed findings
//! "come back").

use coach_core::system::{Action, AudioEndpoint};
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::platform::mock::{FxMode, MockPlatform};
use setup_coach_app::types::*;
use std::sync::Arc;

const ENH: &str = "system.audio_enhancements_on";
const WEBCAM: &str = "{0.0.1.00000000}.{mock-c922-webcam}";
const GOXLR: &str = "{0.0.1.00000000}.{mock-goxlr}";

fn core(tweak: impl FnOnce(&MockPlatform)) -> (Arc<AppCore>, MockPlatform, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    tweak(&mock);
    let c = AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default()));
    c.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    (Arc::new(c), mock, dir)
}

/// Windows default = a webcam's built-in mic; the mic the user set up = the GoXLR.
fn two_mics(m: &MockPlatform) {
    let mut w = m.system.windows.lock().unwrap();
    let base = w.endpoints[0].clone();
    w.endpoints[0].id = WEBCAM.into();
    w.endpoints[0].name = "Microphone (C922 Pro Stream Webcam)".into();
    w.endpoints[0].enhancements_enabled = false;
    w.endpoints.push(AudioEndpoint { id: GOXLR.into(), name: "Chat Mic (2- TC-HELICON GoXLR Mini)".into(), is_default: false, is_default_comms: false, enhancements_enabled: true, ..base });
}

fn enh_action(q: &QuickChecks) -> Option<&PlannedActionView> {
    q.actions.iter().find(|a| matches!(a.action, Action::DisableEnhancements { .. }))
}

fn enhancements_on(m: &MockPlatform, id: &str) -> bool {
    m.system.windows.lock().unwrap().endpoints.iter().find(|e| e.id == id).unwrap().enhancements_enabled
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fix_that_worked_is_fixed_in_the_fresh_scan_and_stays_fixed() {
    let (core, mock, _d) = core(|_| {});
    let q = core.run_quick_checks().await;
    let id = enh_action(&q).expect("enhancements fix offered").id.clone();
    let r = core.fix_and_verify(std::slice::from_ref(&id), None).await;
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].status, FixStatus::Fixed, "{:?}", r.items[0]);
    let mic_id = mock.system.windows.lock().unwrap().endpoints[0].id.clone();
    assert!(!enhancements_on(&mock, &mic_id));
    // The scan that comes back already lacks the finding and the action: nothing to "come back".
    assert!(!r.quick.findings.iter().any(|f| f.id == ENH), "{:?}", r.quick.findings.iter().map(|f| &f.id).collect::<Vec<_>>());
    assert!(enh_action(&r.quick).is_none());
    // A second fix round for something else must not resurrect it.
    let r2 = core.fix_and_verify(&[], None).await;
    assert!(!r2.quick.findings.iter().any(|f| f.id == ENH));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_fix_says_why_and_the_finding_stays_with_a_new_action_to_retry() {
    let (core, mock, _d) = core(|m| *m.system.fx_mode.lock().unwrap() = FxMode::Denied);
    let q = core.run_quick_checks().await;
    let id = enh_action(&q).unwrap().id.clone();
    let r = core.fix_and_verify(std::slice::from_ref(&id), None).await;
    let item = &r.items[0];
    assert_eq!(item.status, FixStatus::NeedsYou, "{item:?}");
    assert!(item.message.contains("nothing was changed"), "{}", item.message);
    assert!(item.guided.is_some(), "a guided step to finish it by hand");
    assert_eq!(item.label, "Turn off Windows audio enhancements");
    // Verified by the re-scan, not by the call: still there, still fixable.
    assert!(r.quick.findings.iter().any(|f| f.id == ENH));
    assert!(enh_action(&r.quick).is_some());
    assert!(mock.system.windows.lock().unwrap().endpoints[0].enhancements_enabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_that_claims_success_but_changes_nothing_is_not_counted() {
    let (core, _mock, _d) = core(|m| *m.system.fx_mode.lock().unwrap() = FxMode::ClaimsButNothing);
    let q = core.run_quick_checks().await;
    let id = enh_action(&q).unwrap().id.clone();
    let r = core.fix_and_verify(std::slice::from_ref(&id), None).await;
    assert_ne!(r.items[0].status, FixStatus::Fixed, "{:?}", r.items[0]);
    assert!(r.quick.findings.iter().any(|f| f.id == ENH));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_scan_and_the_fix_use_the_mic_the_user_picked_not_the_windows_default() {
    let (core, mock, _d) = core(two_mics);
    // Windows default is the webcam mic, which has no problem: nothing to fix there.
    let q_default = core.run_quick_checks().await;
    assert!(enh_action(&q_default).is_none());
    // The user picked the GoXLR: its enhancements are what the scan reports and the fix changes.
    let q = core.run_quick_checks_for(Some(GOXLR)).await;
    let a = enh_action(&q).expect("fix for the picked mic");
    assert!(matches!(&a.action, Action::DisableEnhancements { endpoint } if endpoint == GOXLR), "{:?}", a.action);
    let r = core.fix_and_verify(&[a.id.clone()], Some(GOXLR)).await;
    assert_eq!(r.items[0].status, FixStatus::Fixed, "{:?}", r.items[0]);
    assert!(!enhancements_on(&mock, GOXLR));
    assert!(!enhancements_on(&mock, WEBCAM));
    assert!(!r.quick.findings.iter().any(|f| f.id == ENH));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_out_of_date_fix_id_is_reported_not_dropped() {
    let (core, _mock, _d) = core(|_| {});
    core.run_quick_checks().await;
    let r = core.fix_and_verify(&["act-99".to_string()], None).await;
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].status, FixStatus::Failed);
}
