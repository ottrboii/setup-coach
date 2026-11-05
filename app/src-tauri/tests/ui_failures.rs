//! Quick-scan honesty: fixes we can't do are not offered, OBS keyframe/rate
//! control really are written, and UI errors reach a log in the data folder.

use coach_core::finding::{Area, Finding, FixKind, Severity};
use setup_coach_app::app::{downgrade_unfixable, AppCore, CollectingEmitter};
use setup_coach_app::obs::files;
use setup_coach_app::platform::mock::MockPlatform;
use std::sync::Arc;

fn finding(id: &str, fix: &str) -> Finding {
    Finding {
        id: id.into(),
        area: Area::System,
        severity: Severity::Major,
        title: "t".into(),
        why: "w".into(),
        fix: fix.into(),
        fix_kind: FixKind::Auto,
        can_decline: false,
        evidence: Default::default(),
        suggest_product: None,
    }
}

#[test]
fn unfixable_findings_lose_the_we_can_fix_it_promise() {
    let mut fs = vec![finding("a", "Turn it off. We can do it for you."), finding("b", "Other. We can set it for you.")];
    let ids = vec!["a".to_string()];
    downgrade_unfixable(&mut fs, ids.iter());
    assert_eq!(fs[0].fix_kind, FixKind::Guided);
    assert_eq!(fs[0].fix, "Turn it off.");
    assert_eq!(fs[1].fix_kind, FixKind::Auto);
}

#[test]
fn keyframe_interval_is_written_to_the_advanced_encoder_file() {
    let dir = tempfile::tempdir().unwrap();
    let obs = dir.path();
    std::fs::write(obs.join("user.ini"), "[Basic]\nProfileDir=Untitled\nSceneCollectionFile=Main\n").unwrap();
    let profile = obs.join("basic").join("profiles").join("Untitled");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("streamEncoder.json"), r#"{"bitrate":6000,"keyint_sec":0,"rate_control":"VBR"}"#).unwrap();
    files::set_stream_encoder_field(obs, "keyint_sec", serde_json::json!(2)).unwrap();
    files::set_stream_encoder_field(obs, "rate_control", serde_json::json!("CBR")).unwrap();
    let v = files::advanced_stream_encoder(obs).unwrap();
    assert_eq!(v["keyint_sec"], 2);
    assert_eq!(v["rate_control"], "CBR");
    assert_eq!(v["bitrate"], 6000);
}

#[test]
fn missing_encoder_file_is_a_plain_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("user.ini"), "[Basic]\nProfileDir=Untitled\nSceneCollectionFile=Main\n").unwrap();
    let e = files::set_stream_encoder_field(dir.path(), "keyint_sec", serde_json::json!(2)).unwrap_err();
    assert!(e.contains("encoder settings"), "{e}");
}

#[test]
fn ui_errors_are_logged_to_the_data_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let core = Arc::new(AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default())));
    core.log_ui("error", "scan", "run_quick_checks failed\nsecond line");
    let text = std::fs::read_to_string(dir.path().join("data").join("logs").join("ui.log")).unwrap();
    assert!(text.contains("[error] scan: run_quick_checks failed second line"), "{text}");
}
