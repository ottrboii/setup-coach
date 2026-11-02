//! `--self-test` against the mock platform and a fake OBS / GoXLR Utility:
//! JSON shape, zip contents, and that the OBS apply test cleans up after itself.

use serde_json::Value;
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::goxlr::{FakeGoXlr, MicType};
use setup_coach_app::obs::{mock::FakeObs, ObsApi, F_COMPRESSOR, F_EXPANDER, F_GAIN, F_LIMITER, F_VOICE};
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::selftest::{run_self_test, CliOptions, CHECK_TIMEOUT, TEST_INPUT, TEST_SCENE};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CHECK_IDS: [&str; 12] = [
    "app_and_windows",
    "microphones",
    "windows_ai_effects",
    "cameras",
    "vst3_installed",
    "obs_running",
    "obs_websocket",
    "obs_vst3_host",
    "obs_apply_test",
    "obs_apply_cleanup",
    "goxlr_utility",
    "saved_setup",
];

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    dir: tempfile::TempDir,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let core = Arc::new(AppCore::new(mock.platform(), dir.path().join("SetupCoach"), Arc::new(CollectingEmitter::default())));
    *mock.system.desktop_dir.lock().unwrap() = Some(dir.path().join("Desktop"));
    World { core, mock, dir }
}

fn read_zip(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id}"))
}

fn assert_shape(report: &Value) {
    assert_eq!(report["kind"], "setup-coach-self-test");
    assert_eq!(report["schema_version"], 1);
    assert!(report["app_version"].is_string());
    let checks = report["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, CHECK_IDS);
    for c in checks {
        assert!(["pass", "warn", "fail", "skip"].contains(&c["status"].as_str().unwrap()), "{c}");
        assert!(!c["name"].as_str().unwrap().is_empty());
        assert!(!c["note"].as_str().unwrap().is_empty(), "{c}");
        assert!(c.get("details").is_some());
        assert!(c["duration_ms"].is_u64());
    }
    let s = &report["summary"];
    let total: u64 = ["pass", "warn", "fail", "skip"].iter().map(|k| s[*k].as_u64().unwrap()).sum();
    assert_eq!(total as usize, checks.len());
}

#[tokio::test]
async fn full_self_test_with_obs_and_goxlr() {
    let w = world();
    // Saved setup + chain, and an old zip that must not be re-packed.
    let data = w.core.data_dir.clone();
    std::fs::create_dir_all(data.join("sub")).unwrap();
    std::fs::write(data.join("setup.json"), r#"{"name":"Mine","saved_at":"2026-11-01T10:00:00Z","mic_chain":null,"camera_settings":null,"camera_id":null,"obs_input":"Mic/Aux"}"#).unwrap();
    std::fs::write(data.join("chain.json"), serde_json::to_string(&setup_coach_app::selftest::self_test_chain()).unwrap()).unwrap();
    std::fs::write(data.join("sub").join("note.txt"), "hi").unwrap();
    std::fs::write(data.join("old.zip"), "zip").unwrap();
    // OBS config folder with two logs; the newer one is packed.
    let obs_dir = w.dir.path().join("obs-studio");
    std::fs::create_dir_all(obs_dir.join("logs")).unwrap();
    std::fs::write(obs_dir.join("logs").join("2026-10-01 10-00-00.txt"), "old log").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(obs_dir.join("logs").join("2026-11-02 09-00-00.txt"), "new log").unwrap();
    *w.mock.system.obs_dir.lock().unwrap() = Some(obs_dir);
    // Installed VST3 bundle.
    let vst3 = w.dir.path().join("VST3").join("SetupCoachVoice.vst3");
    std::fs::create_dir_all(vst3.join("Contents").join("x86_64-win")).unwrap();
    std::fs::write(vst3.join("Contents").join("x86_64-win").join("SetupCoachVoice.vst3"), "dll").unwrap();
    // OBS with a VST3 host (atkAudio).
    let obs = Arc::new(FakeObs::typical());
    obs.state.lock().unwrap().filter_kinds.push("atkaudio_plugin_host".into());
    w.core.set_obs(obs.clone()).await;
    let user_filters_before = obs.filter_names("Mic/Aux");
    w.core.set_goxlr(Some(Arc::new(FakeGoXlr::with_device("S1", MicType::Dynamic, 40.0)))).await;

    let zip = run_self_test(w.core.clone(), &CliOptions::default(), Some(vst3.display().to_string()), CHECK_TIMEOUT).await.unwrap();
    assert_eq!(zip.parent().unwrap(), w.dir.path().join("Desktop"));
    assert!(zip.file_name().unwrap().to_string_lossy().starts_with("SetupCoach-selftest-"));
    let last = std::fs::read_to_string(data.join("last-self-test.txt")).unwrap();
    assert_eq!(PathBuf::from(last.trim()), zip);

    let files = read_zip(&zip);
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    for n in ["self-test.json", "self-test.txt", "SetupCoach/setup.json", "SetupCoach/chain.json", "SetupCoach/sub/note.txt", "obs-logs/2026-11-02 09-00-00.txt"] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    assert!(!names.iter().any(|n| n.ends_with("old.zip") || n.contains("2026-10-01")), "{names:?}");

    let report: Value = serde_json::from_slice(&files["self-test.json"]).unwrap();
    assert_shape(&report);
    let txt = String::from_utf8(files["self-test.txt"].clone()).unwrap();
    assert!(txt.contains("[PASS] OBS WebSocket reachable"), "{txt}");

    for id in ["microphones", "cameras", "vst3_installed", "obs_running", "obs_websocket", "obs_vst3_host", "obs_apply_test", "obs_apply_cleanup", "goxlr_utility", "saved_setup"] {
        assert_eq!(check(&report, id)["status"], "pass", "{}", check(&report, id));
    }
    // The demo mock has enhancements on but no AI effect.
    assert_eq!(check(&report, "windows_ai_effects")["status"], "pass");
    let apply = &check(&report, "obs_apply_test")["details"];
    assert_eq!(apply["readback_order"], serde_json::json!([F_VOICE, F_GAIN, F_EXPANDER, F_COMPRESSOR, F_LIMITER]));
    assert_eq!(check(&report, "saved_setup")["details"]["setup"]["name"], "Mine");
    assert!(report["bundle"]["files"].as_array().unwrap().iter().any(|f| f == "obs-logs/2026-11-02 09-00-00.txt"));

    // Nothing left behind; the user's filters are untouched.
    let st = obs.state.lock().unwrap();
    assert!(!st.inputs.iter().any(|i| i.name == TEST_INPUT));
    assert!(!st.scenes.iter().any(|s| s == TEST_SCENE));
    assert!(!st.filters.contains_key(TEST_INPUT));
    drop(st);
    assert_eq!(obs.filter_names("Mic/Aux"), user_filters_before);
    assert!(obs.state.lock().unwrap().filters["Mic/Aux"].iter().all(|f| f.enabled));
    // The chain the VST3 reads live was not rewritten by the test.
    let chain_after: Value = serde_json::from_str(&std::fs::read_to_string(data.join("chain.json")).unwrap()).unwrap();
    assert_eq!(chain_after["notes"][0], "Setup Coach self-test (temporary)");
}

#[tokio::test]
async fn without_obs_vst3_host_the_apply_test_warns_and_still_cleans_up() {
    let w = world();
    let obs = Arc::new(FakeObs::typical());
    // A leftover from an interrupted earlier run is removed first.
    obs.request("CreateScene", serde_json::json!({ "sceneName": TEST_SCENE })).await.unwrap();
    w.core.set_obs(obs.clone()).await;
    let opts = CliOptions { out_dir: Some(w.dir.path().join("out")), ..Default::default() };
    let zip = run_self_test(w.core.clone(), &opts, Some("/nonexistent/SetupCoachVoice.vst3".into()), CHECK_TIMEOUT).await.unwrap();
    assert_eq!(zip.parent().unwrap(), w.dir.path().join("out"));
    let report: Value = serde_json::from_slice(&read_zip(&zip)["self-test.json"]).unwrap();
    assert_shape(&report);
    assert_eq!(check(&report, "vst3_installed")["status"], "fail");
    assert_eq!(check(&report, "obs_vst3_host")["status"], "warn");
    let apply = check(&report, "obs_apply_test");
    assert_eq!(apply["status"], "warn", "{apply}");
    assert_eq!(apply["details"]["readback_order"], serde_json::json!([F_GAIN, F_EXPANDER, F_COMPRESSOR, F_LIMITER]));
    assert_eq!(apply["details"]["leftovers_removed"], serde_json::json!([TEST_SCENE]));
    assert_eq!(check(&report, "obs_apply_cleanup")["status"], "pass");
    assert_eq!(check(&report, "goxlr_utility")["status"], "skip");
    assert!(!obs.state.lock().unwrap().scenes.iter().any(|s| s == TEST_SCENE));
}

#[tokio::test]
async fn obs_closed_skips_obs_checks_and_still_writes_the_zip() {
    let w = world();
    *w.mock.system.obs_running.lock().unwrap() = false;
    let zip = run_self_test(w.core.clone(), &CliOptions::default(), None, CHECK_TIMEOUT).await.unwrap();
    let files = read_zip(&zip);
    let report: Value = serde_json::from_slice(&files["self-test.json"]).unwrap();
    assert_shape(&report);
    assert_eq!(check(&report, "obs_running")["status"], "warn");
    for id in ["obs_websocket", "obs_vst3_host", "obs_apply_test", "obs_apply_cleanup", "vst3_installed"] {
        assert_eq!(check(&report, id)["status"], "skip", "{id}");
    }
    // First launch: %APPDATA%\SetupCoach doesn't exist yet. That's fine, and
    // the folder exists afterwards (last-self-test.txt).
    assert_eq!(check(&report, "saved_setup")["status"], "pass");
    assert_eq!(check(&report, "saved_setup")["details"]["exists"], false);
    assert!(report["bundle"]["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("doesn't exist yet")));
    assert!(w.core.data_dir.join("last-self-test.txt").is_file());
    assert!(report["bundle"]["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("OBS")));
}

/// OBS that accepts the connection but never answers.
struct HangingObs;

#[async_trait::async_trait]
impl ObsApi for HangingObs {
    async fn request(&self, _t: &str, _d: Value) -> Result<Value, String> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Ok(Value::Null)
    }
}

#[tokio::test]
async fn a_hanging_obs_times_out_without_blocking_other_checks() {
    let w = world();
    w.core.set_obs(Arc::new(HangingObs)).await;
    let t = Instant::now();
    let zip = run_self_test(w.core.clone(), &CliOptions::default(), None, Duration::from_millis(200)).await.unwrap();
    // 4 OBS checks × 0.2 s + the rest, far from an hour.
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    let report: Value = serde_json::from_slice(&read_zip(&zip)["self-test.json"]).unwrap();
    assert_shape(&report);
    for id in ["obs_vst3_host", "obs_apply_test", "obs_apply_cleanup"] {
        let c = check(&report, id);
        assert_eq!(c["status"], "fail", "{c}");
        assert_eq!(c["details"]["timed_out"], true);
    }
    assert_eq!(check(&report, "microphones")["status"], "pass");
    assert_eq!(check(&report, "saved_setup")["status"], "pass");
}
