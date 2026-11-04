//! Settings vault (backup before every write, `original`, restore) and the
//! real A/B recordings, end to end on the mock platform with a fake OBS.

use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::{dsp, MicTest, SegmentKind};
use coach_core::system::DuckingPreference;
use setup_coach_app::app::{AppCore, CollectingEmitter, Target};
use setup_coach_app::goxlr::{FakeGoXlr, GoXlrApi, MicType};
use setup_coach_app::obs::mock::{FakeFilter, FakeObs};
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::types::*;
use setup_coach_app::vault::{VaultItem, ORIGINAL};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    obs: Arc<FakeObs>,
}

async fn world_in(data: &Path, tweak: impl FnOnce(&mut MockPlatform)) -> World {
    let mut mock = MockPlatform::demo();
    tweak(&mut mock);
    let core = Arc::new(AppCore::new(mock.platform(), data.to_path_buf(), Arc::new(CollectingEmitter::default())));
    let obs = Arc::new(FakeObs::typical());
    core.set_obs(obs.clone()).await;
    World { core, mock, obs }
}

fn mic_id(w: &World) -> String {
    w.mock.audio.inputs[0].id.clone()
}

fn filters(obs: &FakeObs, src: &str) -> Vec<(String, String, bool, serde_json::Value)> {
    obs.state.lock().unwrap().filters.get(src).cloned().unwrap_or_default().into_iter().map(|f: FakeFilter| (f.name, f.kind, f.enabled, f.settings)).collect()
}

fn scaled(mut t: MicTest, db: f64) -> MicTest {
    let g = dsp::from_db(db) as f32;
    for c in &mut t.audio.channels {
        for v in c.iter_mut() {
            *v *= g;
        }
    }
    t
}

/// Feed `test` through the capture path with its segment marks.
fn record(w: &World, test: &MicTest) {
    *w.mock.audio.recording.lock().unwrap() = Some(test.clone());
    w.core.start_mic_test(&mic_id(w)).unwrap();
    for seg in &test.segments {
        w.core.mark_step_at(seg.kind, Edge::Start, seg.start_s).unwrap();
        w.core.mark_step_at(seg.kind, Edge::End, seg.end_s).unwrap();
    }
}

fn original_items(core: &AppCore) -> Vec<VaultItem> {
    core.vault().load(ORIGINAL).unwrap().0.items
}

/// A minimal OBS config folder (closed OBS edits).
fn fake_obs_dir(root: &Path) -> PathBuf {
    let o = root.join("obs-studio");
    std::fs::create_dir_all(o.join("basic/profiles/Main")).unwrap();
    std::fs::create_dir_all(o.join("basic/scenes")).unwrap();
    std::fs::write(o.join("global.ini"), "[Basic]\nProfile=Main\nProfileDir=Main\nSceneCollection=Stream\nSceneCollectionFile=Stream\n").unwrap();
    std::fs::write(o.join("basic/profiles/Main/basic.ini"), "[Audio]\nSampleRate=44100\n").unwrap();
    std::fs::write(o.join("basic/scenes/Stream.json"), r#"{"sources":[{"name":"Mic/Aux","flags":0}]}"#).unwrap();
    o
}

#[tokio::test]
async fn quick_fixes_are_backed_up_first_and_restore_puts_everything_back() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_in(&dir.path().join("data"), |_| {}).await;
    let before_filters = filters(&w.obs, "Mic/Aux");
    let before_monitor = w.obs.state.lock().unwrap().inputs[0].monitor.clone();
    let q = w.core.run_quick_checks().await;
    let all: Vec<String> = q.actions.iter().map(|a| a.id.clone()).collect();
    let res = w.core.apply_actions(&all).await;
    assert!(!res.applied.is_empty());
    assert_eq!(w.mock.system.windows.lock().unwrap().ducking, DuckingPreference::DoNothing);
    assert_ne!(filters(&w.obs, "Mic/Aux"), before_filters);

    // Everything about to change was saved before the write.
    let list = w.core.list_backups();
    assert_eq!(list[0].id, ORIGINAL);
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[1].label, "Before quick fixes");
    let keys: Vec<String> = original_items(&w.core).iter().map(|i| i.key()).collect();
    for k in ["obs_filters:Mic/Aux", "obs_monitoring:Mic/Aux", "windows:ducking", "windows:usb_selective_suspend"] {
        assert!(keys.iter().any(|x| x == k), "{k} not in {keys:?}");
    }
    assert!(keys.iter().any(|k| k.starts_with("windows:enhancements:")), "{keys:?}");
    let saved = original_items(&w.core).into_iter().find(|i| i.key() == "obs_filters:Mic/Aux").unwrap();
    let VaultItem::ObsFilters { filters: f, .. } = saved else { panic!() };
    assert_eq!(f.iter().map(|f| f.name.clone()).collect::<Vec<_>>(), ["Noise Suppression", "Noise Gate"]);
    assert_eq!(f[0].settings["method"], "rnnoise");

    let r = w.core.restore_backup(ORIGINAL).await;
    assert!(r.failed.is_empty(), "{r:?}");
    assert_eq!(filters(&w.obs, "Mic/Aux"), before_filters);
    assert_eq!(w.obs.state.lock().unwrap().inputs[0].monitor, before_monitor);
    let win = w.mock.system.windows.lock().unwrap().clone();
    assert_eq!(win.ducking, DuckingPreference::Reduce80);
    assert!(win.usb_selective_suspend_enabled);
    assert!(win.endpoints[0].enhancements_enabled);
    // The restore itself was backed up (so it can be undone).
    assert!(w.core.list_backups()[1].label.starts_with("Before going back to"));
}

#[tokio::test]
async fn mic_chain_round_trip_and_original_survives_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let w = world_in(&data, |_| {}).await;
    let before = filters(&w.obs, "Mic/Aux");
    record(&w, &build(&Scenario::default()));
    let rep = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    let res = w.core.apply_mic_chain(rep.chain.clone(), "Mic/Aux").await;
    assert!(res.applied.contains(&"chain_json".to_string()), "{res:?}");
    assert!(data.join("chain.json").exists());
    let snap = &w.core.list_backups()[1];
    assert!(snap.label.starts_with("Before setting up your Yeti"), "{}", snap.label);
    // chain.json didn't exist before: saved as "none".
    assert!(original_items(&w.core).contains(&VaultItem::ChainJson { content: None }));

    // A later session changes things again: the original keeps the first values.
    let mut chain2 = rep.chain.clone();
    chain2.input_gain_db += 3.0;
    let w2 = World { core: Arc::new(AppCore::new(w.mock.platform(), data.clone(), Arc::new(CollectingEmitter::default()))), mock: MockPlatform::demo(), obs: w.obs.clone() };
    w2.core.set_obs(w.obs.clone()).await;
    w2.core.apply_mic_chain(chain2, "Mic/Aux").await;
    assert!(original_items(&w2.core).contains(&VaultItem::ChainJson { content: None }));
    let VaultItem::ObsFilters { filters: f, .. } = original_items(&w2.core).into_iter().find(|i| i.key() == "obs_filters:Mic/Aux").unwrap() else { panic!() };
    assert_eq!(f.len(), 2, "original OBS filters kept: {f:?}");

    let r = w2.core.restore_backup(ORIGINAL).await;
    assert!(r.failed.is_empty(), "{r:?}");
    assert_eq!(filters(&w.obs, "Mic/Aux"), before, "OBS filters equal to before Setup Coach");
    assert!(!data.join("chain.json").exists());

    // Restore the newest per-change snapshot (Setup Coach's first chain).
    let first = w2.core.list_backups().into_iter().find(|b| b.label.starts_with("Before setting up")).unwrap();
    assert!(w2.core.restore_backup(&first.id).await.failed.is_empty());
    assert!(w2.core.restore_backup("nope").await.failed.len() == 1);
}

#[tokio::test]
async fn obs_file_edits_are_copied_and_restored_with_obs_closed() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_in(&dir.path().join("data"), |_| {}).await;
    let obs_dir = fake_obs_dir(dir.path());
    *w.mock.system.obs_dir.lock().unwrap() = Some(obs_dir.clone());
    *w.mock.system.obs_running.lock().unwrap() = false;
    let scenes = obs_dir.join("basic/scenes/Stream.json");
    let orig = std::fs::read_to_string(&scenes).unwrap();
    record(&w, &build(&Scenario { left_only: true, ..Default::default() }));
    let rep = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    assert!(rep.chain.mono_downmix);
    let res = w.core.apply_mic_chain(rep.chain, "Mic/Aux").await;
    assert!(res.applied.contains(&"mono".to_string()), "{res:?}");
    assert_ne!(std::fs::read_to_string(&scenes).unwrap(), orig);
    // The saved path is whatever OBS's config resolves to (separators differ per OS).
    let key = original_items(&w.core)
        .iter()
        .find(|i| matches!(i, VaultItem::ObsFile { path, .. } if path.ends_with("Stream.json")))
        .map(|i| i.key())
        .expect("scene collection backed up");

    // With OBS open, files are refused with the close-OBS message.
    *w.mock.system.obs_running.lock().unwrap() = true;
    let r = w.core.restore_backup(ORIGINAL).await;
    assert!(r.failed.iter().any(|f| f.id == key && f.error.contains("Close OBS")), "{r:?}");
    *w.mock.system.obs_running.lock().unwrap() = false;
    let r = w.core.restore_backup(ORIGINAL).await;
    assert!(r.applied.contains(&key), "{r:?}");
    assert!(r.restart_obs_needed);
    assert_eq!(std::fs::read_to_string(&scenes).unwrap(), orig);
}

#[tokio::test]
async fn unreadable_obs_file_blocks_the_backup_instead_of_looking_missing() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_in(&dir.path().join("data"), |_| {}).await;
    // A directory where the file should be: reading fails with something other than NotFound.
    let e = w.core.backup_before("x", &[Target::ObsFile(dir.path().to_path_buf())]).await.unwrap_err();
    assert!(e.contains("nothing was changed"), "{e}");
    // A file that simply isn't there is fine: nothing to save.
    assert!(w.core.backup_before("x", &[Target::ObsFile(dir.path().join("absent.ini"))]).await.is_ok());
}

#[tokio::test]
async fn failed_filter_restore_never_removes_before_creating_and_points_at_the_undo() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_in(&dir.path().join("data"), |_| {}).await;
    let before = filters(&w.obs, "Mic/Aux");
    assert!(before.len() >= 2);
    let snap = w.core.vault().save("keep", &[w.core.capture(&Target::ObsFilters("Mic/Aux".into())).await.unwrap().unwrap()]).unwrap().unwrap();
    // Drop one filter, then make the re-create step fail.
    w.obs.state.lock().unwrap().filters.get_mut("Mic/Aux").unwrap().remove(0);
    w.obs.state.lock().unwrap().fail = vec!["CreateSourceFilter".into()];
    let r = w.core.restore_backup(&snap).await;
    assert!(r.failed.iter().any(|f| f.error.contains("Before going back")), "{r:?}");
    assert_eq!(filters(&w.obs, "Mic/Aux").len(), before.len() - 1, "nothing else was deleted");
}

#[tokio::test]
async fn camera_values_are_saved_and_missing_devices_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let w = world_in(&data, |_| {}).await;
    let before = w.mock.camera.state.lock().unwrap().clone();
    let webcam_before = w.obs.state.lock().unwrap().inputs[2].settings.clone();
    w.core.start_camera_test("mock-c920").await.unwrap();
    for step in [coach_core::video::TestStep::Neutral, coach_core::video::TestStep::HoldPaper] {
        w.core.capture_camera_step(step, 1.0).await.unwrap();
    }
    let rep = w.core.finish_camera_test(vec![]).unwrap();
    assert!(rep.settings.uvc.is_some());
    w.core.apply_camera_settings("mock-c920", rep.settings).await;
    assert_ne!(*w.mock.camera.state.lock().unwrap(), before);
    assert!(original_items(&w.core).iter().any(|i| matches!(i, VaultItem::Camera { state, .. } if **state == before)));
    let r = w.core.restore_backup(ORIGINAL).await;
    assert!(r.failed.is_empty(), "{r:?}");
    let after = w.mock.camera.state.lock().unwrap().clone();
    assert_eq!((after.auto_exposure, after.powerline_hz, after.auto_wb), (before.auto_exposure, before.powerline_hz, before.auto_wb));
    assert_eq!(w.obs.state.lock().unwrap().inputs[2].settings, webcam_before, "OBS webcam mode restored exactly");

    // Same vault, camera and mic unplugged: skipped with a plain note.
    let w2 = world_in(&data, |m| {
        Arc::get_mut(&mut m.camera).unwrap().devices.clear();
    })
    .await;
    let r = w2.core.restore_backup(ORIGINAL).await;
    assert!(r.skipped.iter().any(|s| s.id == "camera:mock-c920" && s.error.contains("isn't connected")), "{r:?}");
}

#[tokio::test]
async fn goxlr_values_are_saved_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_in(&dir.path().join("data"), |_| {}).await;
    let fake = Arc::new(FakeGoXlr::with_device("S1", MicType::Dynamic, 40.0));
    w.core.set_goxlr(Some(fake.clone() as Arc<dyn GoXlrApi>)).await;
    let id = w.core.backup_before("Before auto-adjusting your mic level", &[Target::GoXlr]).await.unwrap().unwrap();
    fake.command("S1", setup_coach_app::goxlr::cmd_set_mic_type(MicType::Condenser)).await.unwrap();
    fake.command("S1", setup_coach_app::goxlr::cmd_set_mic_gain(MicType::Condenser, 55.0)).await.unwrap();
    let r = w.core.restore_backup(&id).await;
    assert!(r.failed.is_empty(), "{r:?}");
    let d = fake.devices().await.unwrap().remove(0);
    assert_eq!((d.mic_type, d.mic_gain_db), (MicType::Dynamic, 40.0));
    // Without the Utility: skipped, not failed.
    w.core.set_goxlr(None).await;
    let r = w.core.restore_backup(&id).await;
    assert!(r.failed.is_empty() && r.skipped.len() == 1, "{r:?}");
}

#[tokio::test]
async fn ab_original_and_final_are_recorded_matched_and_exported() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let w = world_in(&data, |_| {}).await;
    let desktop = dir.path().join("Desktop");
    *w.mock.system.desktop_dir.lock().unwrap() = Some(desktop.clone());
    let id = mic_id(&w);
    assert!(w.core.get_ab(&id).is_err());

    // First test: kept as the original.
    let take1 = build(&Scenario { speech_level_dbfs: -32.0, ..Default::default() });
    record(&w, &take1);
    let rep = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    let rec_dir = data.join("recordings").join("microphone-yeti-stereo-microphone");
    let wav = setup_coach_app::wav::read(&rec_dir.join("original.wav")).unwrap();
    assert_eq!(wav.sample_rate, take1.audio.sample_rate);
    assert_eq!(wav.samples.len(), take1.audio.len(), "whole take saved");
    let meta: setup_coach_app::app::TakeMeta = serde_json::from_str(&std::fs::read_to_string(rec_dir.join("original.json")).unwrap()).unwrap();
    assert_eq!(meta.device_id, id);
    assert_eq!(meta.score, rep.score.value);
    assert!(meta.segments.iter().any(|s| s.kind == SegmentKind::Read));
    let ab = w.core.get_ab(&id).unwrap();
    assert!(ab.final_take.is_none());

    // A re-test in the same session doesn't replace it.
    record(&w, &scaled(take1.clone(), -10.0));
    w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    assert_eq!(setup_coach_app::wav::read(&rec_dir.join("original.wav")).unwrap(), wav);

    assert!(w.core.record_final(&id, Default::default(), vec![]).is_err(), "no recording running");
    w.core.apply_mic_chain(rep.chain.clone(), "Mic/Aux").await;

    // Final take: same passage, read again (only the read step).
    let read = take1.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
    record(&w, &MicTest { audio: take1.audio.clone(), segments: vec![*read] });
    let ab = w.core.record_final(&id, Default::default(), vec![]).unwrap();
    let fin = ab.final_take.clone().expect("final take");
    assert!(rec_dir.join("final-raw.wav").exists() && rec_dir.join("final.wav").exists());
    assert!((ab.matched_gain_db - (fin.lufs - ab.original.lufs)).abs() < 0.11, "{ab:?}");
    assert!(fin.lufs > ab.original.lufs, "processing brings the voice up: {ab:?}");

    // Playback and export are loudness-matched.
    w.core.play_preview(PreviewWhich::Original, true).unwrap();
    w.core.play_preview(PreviewWhich::Final, false).unwrap();
    let a = w.core.preview_buffer(PreviewWhich::Original, false).unwrap();
    let b = w.core.preview_buffer(PreviewWhich::Final, false).unwrap();
    let la = dsp::integrated_lufs(&a.channels[0], a.sample_rate);
    let lb = dsp::integrated_lufs(&b.channels[0], b.sample_rate);
    assert!((la - lb).abs() < 0.5, "{la} vs {lb}");
    let out = PathBuf::from(w.core.export_ab(&id).unwrap());
    assert!(out.starts_with(&desktop));
    assert!(out.file_name().unwrap().to_string_lossy().starts_with("Setup Coach A-B - Microphone (Yeti"), "{out:?}");
    let ea = setup_coach_app::wav::read(&out.join("A original.wav")).unwrap();
    let eb = setup_coach_app::wav::read(&out.join("B final.wav")).unwrap();
    assert!((dsp::integrated_lufs(&ea.samples, ea.sample_rate) - dsp::integrated_lufs(&eb.samples, eb.sample_rate)).abs() < 0.5);

    // Next session: a new first test becomes the original, the old one is kept.
    let w2 = world_in(&data, |_| {}).await;
    record(&w2, &take1);
    w2.core.finish_mic_test(Default::default(), vec![]).unwrap();
    let kept: Vec<String> = std::fs::read_dir(&rec_dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("original-") && n.ends_with(".wav")).collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(setup_coach_app::wav::read(&rec_dir.join(&kept[0])).unwrap(), wav);
}

#[tokio::test]
async fn a_minus_6_db_take_is_matched_within_half_a_db() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let w = world_in(&data, |_| {}).await;
    let id = mic_id(&w);
    let take = build(&Scenario::default());
    record(&w, &take);
    let rep = w.core.finish_mic_test(Default::default(), vec![]).unwrap();
    // A pass-through chain: final = the same voice 6 dB quieter.
    let mut chain = rep.chain.clone();
    chain.input_gain_db = 0.0;
    chain.compressor.ratio = 1.0;
    chain.compressor.makeup_db = 0.0;
    chain.expander.ratio = 1.0;
    chain.limiter.ceiling_db = 0.0;
    w.core.apply_mic_chain(chain, "Mic/Aux").await;
    let read = take.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
    record(&w, &MicTest { audio: scaled(take.clone(), -6.0).audio, segments: vec![*read] });
    let ab = w.core.record_final(&id, Default::default(), vec![]).unwrap();
    let a = w.core.preview_buffer(PreviewWhich::Original, false).unwrap();
    let b = w.core.preview_buffer(PreviewWhich::Final, false).unwrap();
    let diff = dsp::integrated_lufs(&a.channels[0], a.sample_rate) - dsp::integrated_lufs(&b.channels[0], b.sample_rate);
    assert!(diff.abs() < 0.5, "matched diff {diff}, ab {ab:?}");
    assert!(ab.matched_gain_db < -1.0, "{ab:?}");
}
