//! Real-PC smoke tests: the native Windows platform, no mocks. They only run
//! on Windows and only when asked (`cargo test --release -p setup-coach-app
//! --test real_windows -- --ignored --nocapture`), because they touch the
//! machine's audio devices, registry and (if running) GoXLR Utility / OBS.
//!
//! They exist because the quick scan once hung forever on a real PC while the
//! mock-platform tests were green.
#![cfg(windows)]

use setup_coach_app::app::{AppCore, CollectingEmitter};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn core() -> Arc<AppCore> {
    let dir = tempfile::tempdir().unwrap().keep();
    Arc::new(AppCore::new(setup_coach_app::platform::native(), dir, Arc::new(CollectingEmitter::default())))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "touches the real PC"]
async fn real_quick_checks_finish_and_serialize() {
    let core = core();
    let t = Instant::now();
    let env = tokio::time::timeout(Duration::from_secs(40), core.get_environment()).await.expect("get_environment hung");
    println!("get_environment: {:?}, {} mics", t.elapsed(), env.mics.len());
    for m in &env.mics {
        println!("  mic {} ({})", m.name, m.id);
    }
    let t = Instant::now();
    let q = tokio::time::timeout(Duration::from_secs(40), core.run_quick_checks()).await.expect("run_quick_checks hung");
    println!("run_quick_checks: {:?}, {} findings, {} actions", t.elapsed(), q.findings.len(), q.actions.len());
    let v = serde_json::to_value(&q).expect("QuickChecks serializes");
    assert!(v["findings"].is_array() && v["actions"].is_array() && v["score"].is_array());
}
