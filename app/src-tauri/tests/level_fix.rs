//! "Fix my level" through GoXLR Utility on the mock platform: small steps,
//! a measurement after each, a backup to undo, and honest plain-language results.

use coach_core::audio::findings::GainAccess;
use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::SegmentKind;
use setup_coach_app::app::{AppCore, CollectingEmitter};
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
    id: String,
    _dir: tempfile::TempDir,
}

async fn world(mic_name: &str) -> World {
    let dir = tempfile::tempdir().unwrap();
    let id = "{0.0.1.00000000}.{mock-goxlr}".to_string();
    let dev = AudioDevice { id: id.clone(), name: mic_name.into(), is_default: true, is_default_comms: true, channels: 1, sample_rate: 48_000, usb: None };
    let mut mock = MockPlatform::demo();
    mock.audio = Arc::new(MockAudio { inputs: vec![dev], recording: Mutex::new(None), played: Mutex::new(0), live: Mutex::new(None) });
    let events = Arc::new(CollectingEmitter::default());
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), events.clone());
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    core.level_fix_window = Duration::from_millis(150);
    let core = Arc::new(core);
    core.set_obs(Arc::new(FakeObs::typical())).await;
    World { core, mock, events, id, _dir: dir }
}

fn speech_source() -> Arc<Vec<f32>> {
    let t = build(&Scenario::default());
    let seg = t.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
    let src = &t.audio.channels[0][t.range(seg)];
    let peak = src.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    Arc::new(src.iter().map(|v| v / peak * 0.5).collect())
}

/// A GoXLR whose gain really changes what the mic delivers (when `linked`).
async fn goxlr_world(start_gain: f64, linked: bool) -> (World, Arc<FakeGoXlr>) {
    let w = world("Chat Mic (TC-Helicon GoXLR)").await;
    let link = Arc::new(Mutex::new(start_gain));
    let mut fake = FakeGoXlr::with_device("S1", MicType::Dynamic, start_gain);
    if linked {
        fake.gain_link = Some(link.clone());
    }
    let fake = Arc::new(fake);
    w.core.set_goxlr(Some(fake.clone() as Arc<dyn GoXlrApi>)).await;
    *w.mock.audio.live.lock().unwrap() = Some(LiveMock { source: speech_source(), sample_rate: 48_000, gain_db: link, offset_db: -54.0, speed: 20.0 });
    (w, fake)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_loud_goxlr_is_lowered_in_steps_and_measured() {
    let (w, fake) = goxlr_world(58.0, true).await;
    let r = w.core.fix_mic_level(&w.id).await.unwrap();
    assert_eq!(r.status, LevelFixStatus::Fixed, "{}", r.line);
    assert!(r.line.contains("too loud") && r.line.contains("GoXLR"), "{}", r.line);
    assert!(r.steps >= 2, "small steps, not one jump: {}", r.steps);
    let g = fake.devices().await.unwrap()[0].mic_gain_db;
    assert!(g < 58.0 && g > 30.0, "gain {g}");
    assert!(r.peak_before_dbfs.unwrap() > -12.0 && r.peak_after_dbfs.unwrap() <= -12.0, "{r:?}");
    // Every gain write was one small step.
    let cmds = fake.commands.lock().unwrap().clone();
    assert!(cmds.len() as u32 >= r.steps);
    assert!(w.events.events.lock().unwrap().iter().any(|e| e.0 == events::LEVEL_FIX_PROGRESS));

    // Undo puts the previous gain back.
    let id = r.backup_id.clone().expect("backup before changing");
    let back = w.core.restore_backup(&id).await;
    assert!(back.failed.is_empty(), "{:?}", back.failed);
    assert_eq!(fake.devices().await.unwrap()[0].mic_gain_db, 58.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn already_fine_changes_nothing() {
    let (w, fake) = goxlr_world(45.0, true).await;
    let r = w.core.fix_mic_level(&w.id).await.unwrap();
    assert_eq!(r.status, LevelFixStatus::Fixed, "{}", r.line);
    assert_eq!(r.steps, 0);
    assert!(r.backup_id.is_none() && r.changed.is_none());
    assert!(fake.commands.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gain_that_changes_nothing_is_put_back_and_not_called_fixed() {
    // The GoXLR gain is not what the mic path hears (fake not linked to the signal).
    let (w, fake) = goxlr_world(58.0, false).await;
    let r = w.core.fix_mic_level(&w.id).await.unwrap();
    assert_eq!(r.status, LevelFixStatus::Couldnt, "{}", r.line);
    assert!(r.line.contains("put it back"), "{}", r.line);
    assert_eq!(fake.devices().await.unwrap()[0].mic_gain_db, 58.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silence_means_needs_you_and_no_change() {
    let (w, fake) = goxlr_world(58.0, true).await;
    *w.mock.audio.live.lock().unwrap() = Some(LiveMock { source: Arc::new(vec![0.0; 48_000]), sample_rate: 48_000, gain_db: Arc::new(Mutex::new(58.0)), offset_db: 0.0, speed: 20.0 });
    let r = w.core.fix_mic_level(&w.id).await.unwrap();
    assert_eq!(r.status, LevelFixStatus::NeedsYou);
    assert!(r.line.contains("couldn't hear you"), "{}", r.line);
    assert!(fake.commands.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn utility_not_running_says_how_to_open_it_and_changes_nothing() {
    let w = world("Chat Mic (TC-Helicon GoXLR)").await;
    let r = w.core.fix_mic_level(&w.id).await.unwrap();
    assert_eq!(r.status, LevelFixStatus::NeedsYou);
    assert!(r.line.contains("GoXLR Utility isn't running") && r.line.contains("GoXLR app"), "{}", r.line);
    assert!(r.backup_id.is_none() && r.changed.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goxlr_gain_is_software_even_when_utility_is_off() {
    for name in ["Chat Mic (TC-Helicon GoXLR)", "Sample (GoXLR Mini Thing)"] {
        let w = world(name).await;
        let ctx = w.core.mic_context(&w.id, &MicAnswers::default()).unwrap();
        assert_eq!(ctx.gain, GainAccess::Software, "{name}");
        assert_eq!(ctx.gain_where.as_deref(), Some("the GoXLR app"), "{name}");
    }
    // The system audit words its tips for the GoXLR too.
    let w = world("Chat Mic (TC-Helicon GoXLR)").await;
    w.mock.system.windows.lock().unwrap().endpoints.iter_mut().for_each(|e| {
        if e.is_capture && e.is_default {
            e.mic_boost_db = Some(10.0);
            e.name = "Chat Mic (TC-Helicon GoXLR)".into();
            e.is_onboard = false;
        }
    });
    let q = w.core.run_quick_checks().await;
    let f = q.findings.iter().find(|f| f.id == "system.mic_boost_on").expect("boost finding");
    assert!(f.fix.contains("GoXLR app"), "{}", f.fix);
}
