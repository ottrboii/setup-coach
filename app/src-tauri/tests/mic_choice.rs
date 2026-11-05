//! "What mic is plugged into this?": Otto's GoXLR hides his RE20 behind the
//! Windows device "Chat Mic". The answer is kept per device, written through
//! the vault, and changes the profile used for the mic test.

use coach_core::audio::findings::{Capsule, Connection};
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::platform::mock::{MockAudio, MockPlatform};
use setup_coach_app::platform::AudioDevice;
use setup_coach_app::types::*;
use std::sync::{Arc, Mutex};

const CHAT: &str = "{0.0.1.00000000}.{mock-chat-mic}";

struct World {
    core: Arc<AppCore>,
    _dir: tempfile::TempDir,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let mut mock = MockPlatform::demo();
    let dev = |id: &str, name: &str, default: bool| AudioDevice {
        id: id.into(),
        name: name.into(),
        is_default: default,
        is_default_comms: default,
        channels: 2,
        sample_rate: 48_000,
        usb: None,
    };
    mock.audio = Arc::new(MockAudio {
        inputs: vec![
            dev(CHAT, "Chat Mic (TC-Helicon GoXLR Mini)", false),
            dev("{0.0.1.00000000}.{mock-yeti}", "Microphone (Yeti Stereo Microphone)", true),
            dev("{0.0.1.00000000}.{mock-headset}", "Headset Microphone (Bluetooth Hands-Free)", false),
        ],
        recording: Mutex::new(None),
        played: Mutex::new(0),
        live: Mutex::new(None),
    });
    let core = AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default()));
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    World { core: Arc::new(core), _dir: dir }
}

async fn chat(w: &World) -> AudioDeviceInfo {
    w.core.get_environment().await.mics.into_iter().find(|m| m.id == CHAT).unwrap()
}

#[test]
fn picker_lists_real_mics_without_generics() {
    let w = world();
    let list = w.core.list_mic_profiles();
    let re20 = list.iter().find(|m| m.id == "ev_re20").expect("RE20 in the picker");
    assert_eq!(re20.display_name, "Electro-Voice RE20");
    assert_eq!((re20.connection.as_str(), re20.capsule.as_str()), ("xlr", "dynamic"));
    assert!(list.len() > 40, "{}", list.len());
    assert!(list.iter().all(|m| !m.id.starts_with("generic")), "generic fallbacks must stay out");
    let names: Vec<_> = list.iter().map(|m| (m.brand.to_lowercase(), m.model.to_lowercase())).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "sorted by brand, model for the picker");
}

#[tokio::test]
async fn goxlr_input_asks_once_and_the_choice_changes_everything() {
    let w = world();
    let before = chat(&w).await;
    assert!(before.needs_mic_choice, "a GoXLR input hides the mic: ask");
    assert!(before.mic_choice.is_none());
    assert!(!before.profile.as_ref().unwrap().chosen);
    let ctx_before = w.core.mic_context(CHAT, &MicAnswers::default()).unwrap();
    assert!(ctx_before.mic_name.as_deref() != Some("Electro-Voice RE20"));

    // Named devices that already say what they are are not asked about.
    let env = w.core.get_environment().await;
    assert!(!env.mics.iter().find(|m| m.name.contains("Yeti")).unwrap().needs_mic_choice);
    assert!(!env.mics.iter().find(|m| m.name.contains("Hands-Free")).unwrap().needs_mic_choice);

    let r = w.core.set_mic_behind(CHAT, MicBehindInput::Profile { profile_id: "ev_re20".into() }).await.unwrap();
    assert!(!r.needs_mic_choice);
    let c = r.mic_choice.as_ref().unwrap();
    assert_eq!((c.status, c.profile_id.as_deref(), c.display_name.as_deref()), (MicBehindStatus::Chosen, Some("ev_re20"), Some("Electro-Voice RE20")));
    let p = r.profile.as_ref().unwrap();
    assert!(p.chosen && p.profile_id == "ev_re20");
    assert_eq!(p.display_name, "Electro-Voice RE20");
    assert!(p.questions.iter().any(|q| q.options.iter().any(|o| o.to_lowercase().contains("bass"))), "RE20's own switch question: {:?}", p.questions);
    assert!(p.interface.as_ref().is_some_and(|i| i.display_name.contains("GoXLR")), "still knows it sits on the GoXLR");

    // get_environment shows it; no "saved setup" appears just because of an answer.
    let after = chat(&w).await;
    assert_eq!(after.mic_choice, r.mic_choice);
    assert!(!after.needs_mic_choice);
    assert_eq!(after.profile.as_ref().unwrap().profile_id, "ev_re20");
    assert!(w.core.get_environment().await.saved_profile.is_none());

    // The chosen mic drives the mic test context.
    let ctx = w.core.mic_context(CHAT, &MicAnswers::default()).unwrap();
    assert_eq!(ctx.mic_name.as_deref(), Some("Electro-Voice RE20"));
    assert_eq!(ctx.capsule, Some(Capsule::Dynamic));
    assert_eq!(ctx.connection, Some(Connection::Xlr));
    assert_eq!(ctx.distance_cm, Some((3.0, 15.0)));
    assert_ne!(ctx.distance_cm, ctx_before.distance_cm);
    assert_eq!(ctx.interface_max_gain_db, ctx_before.interface_max_gain_db, "interface facts are kept");
    // Other devices are unaffected.
    let yeti = w.core.get_environment().await.mics.into_iter().find(|d| d.name.contains("Yeti")).unwrap();
    assert!(w.core.mic_context(&yeti.id, &MicAnswers::default()).unwrap().mic_name.unwrap().contains("Yeti"));

    // An explicit question answer (by label, like the picker questions send) still wins.
    let a = MicAnswers { mic_model_id: Some("Shure SM7B".into()), ..Default::default() };
    assert_eq!(w.core.mic_context(CHAT, &a).unwrap().mic_name.as_deref(), Some("Shure SM7B"));

    // Saving a setup later keeps the answer.
    let saved = w.core.save_setup("My setup").unwrap();
    assert_eq!(saved.mic_behind.get(CHAT).unwrap().profile_id.as_deref(), Some("ev_re20"));
    assert_eq!(w.core.get_environment().await.saved_profile.unwrap().name, "My setup");
    assert_eq!(w.core.mic_choice(CHAT), r.mic_choice);
}

#[tokio::test]
async fn answers_are_in_the_vault_and_can_be_undone() {
    let w = world();
    w.core.set_mic_behind(CHAT, MicBehindInput::Profile { profile_id: "ev_re20".into() }).await.unwrap();
    w.core.set_mic_behind(CHAT, MicBehindInput::Skipped).await.unwrap();
    let list = w.core.list_backups();
    assert_eq!(list[0].id, "original");
    assert!(list[0].items.iter().any(|i| i.kind == "mic_choice"), "{:?}", list[0].items);
    assert!(list.iter().any(|b| b.label.contains("which mic is plugged into")), "{:?}", list.iter().map(|b| &b.label).collect::<Vec<_>>());
    assert_eq!(w.core.mic_choice(CHAT).unwrap().status, MicBehindStatus::Skipped);
    // Skipped: no profile change, and we don't ask again.
    let d = chat(&w).await;
    assert!(!d.needs_mic_choice && !d.profile.unwrap().chosen);

    // "Go back to how it was before Setup Coach" forgets the answer.
    let res = w.core.restore_backup("original").await;
    assert!(res.failed.is_empty(), "{res:?}");
    assert!(w.core.mic_choice(CHAT).is_none());
    assert!(chat(&w).await.needs_mic_choice);
}

#[tokio::test]
async fn other_clear_and_bad_input() {
    let w = world();
    let r = w.core.set_mic_behind(CHAT, MicBehindInput::Other).await.unwrap();
    assert_eq!(r.mic_choice.unwrap().status, MicBehindStatus::Other);
    assert!(!r.needs_mic_choice);
    let r = w.core.set_mic_behind(CHAT, MicBehindInput::Clear).await.unwrap();
    assert!(r.mic_choice.is_none() && r.needs_mic_choice);

    let e = w.core.set_mic_behind(CHAT, MicBehindInput::Profile { profile_id: "nope".into() }).await.unwrap_err();
    assert!(e.contains("don't know"), "{e}");
    let e = w.core.set_mic_behind(CHAT, MicBehindInput::Profile { profile_id: "generic.xlr_dynamic_low_output".into() }).await.unwrap_err();
    assert!(e.contains("don't know"), "generic fallbacks aren't choosable: {e}");
    let e = w.core.set_mic_behind("unplugged", MicBehindInput::Skipped).await.unwrap_err();
    assert!(e.contains("isn't connected"), "{e}");
    assert!(w.core.mic_choice("unplugged").is_none());
}

#[tokio::test]
async fn a_damaged_setup_file_is_kept_aside_not_lost() {
    let w = world();
    std::fs::create_dir_all(w.core.data_dir.clone()).unwrap();
    std::fs::write(w.core.data_dir.join("setup.json"), "{ not json").unwrap();
    w.core.set_mic_behind(CHAT, MicBehindInput::Skipped).await.unwrap();
    assert_eq!(w.core.mic_choice(CHAT).unwrap().status, MicBehindStatus::Skipped);
    assert!(w.core.data_dir.join("setup.json.unreadable").exists());
}

#[test]
fn old_setup_files_without_the_field_still_load() {
    let old = r#"{"name":"A","saved_at":"2026-11-01T00:00:00Z","mic_chain":null,"camera_settings":null,"camera_id":null,"obs_input":null}"#;
    let p: SavedProfile = serde_json::from_str(old).unwrap();
    assert!(p.mic_behind.is_empty());
}
