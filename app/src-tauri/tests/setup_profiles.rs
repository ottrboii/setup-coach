//! Setup profiles: the session's profile, one stored mic setup per profile per
//! mic, switching with a vault backup, and the profile reaching the analysis.

use coach_core::audio::synth::{build, Scenario};
use coach_core::{Constraint, SetupProfile};
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::obs::mock::FakeObs;
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::types::*;
use std::sync::Arc;

struct World {
    core: Arc<AppCore>,
    mock: MockPlatform,
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let core = AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default()));
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    let core = Arc::new(core);
    core.set_obs(Arc::new(FakeObs::typical())).await;
    World { core, mock, _dir: dir }
}

fn mic_id(w: &World) -> String {
    w.mock.audio.inputs[0].id.clone()
}

fn record(w: &World, s: &Scenario) {
    let test = build(s);
    *w.mock.audio.recording.lock().unwrap() = Some(test.clone());
    w.core.start_mic_test(&mic_id(w)).unwrap();
    for seg in &test.segments {
        w.core.mark_step_at(seg.kind, Edge::Start, seg.start_s).unwrap();
        w.core.mark_step_at(seg.kind, Edge::End, seg.end_s).unwrap();
    }
}

/// A full mic flow under the current profile: context, test, report, apply, verify.
async fn flow(w: &World, constraints: Vec<Constraint>) -> coach_core::audio::MicReport {
    let ctx = w.core.mic_context(&mic_id(w), &MicAnswers::default()).unwrap();
    record(w, &Scenario { rt60_s: 0.5, reverb_wet_db: -6.0, ..Default::default() });
    let r = w.core.finish_mic_test(ctx, constraints).unwrap();
    w.core.apply_mic_chain(r.chain.clone(), "Mic/Aux").await;
    w.core.verify_mic().unwrap();
    r
}

#[tokio::test]
async fn default_is_streaming_and_lists_all_three() {
    let w = world().await;
    assert_eq!(w.core.active_profile(), SetupProfile::Streaming);
    let list = w.core.list_setup_profiles(&mic_id(&w)).unwrap();
    let ids: Vec<_> = list.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![SetupProfile::Streaming, SetupProfile::Voiceover, SetupProfile::Call]);
    assert!(list.iter().all(|p| !p.placement.is_empty()));
    assert!(w.core.list_setup_profiles("nope").is_err());
    let ctx = w.core.mic_context(&mic_id(&w), &MicAnswers::default()).unwrap();
    assert_eq!(ctx.profile, SetupProfile::Streaming);
}

#[tokio::test]
async fn profile_reaches_context_and_report_and_survives_restart() {
    let w = world().await;
    let sw = w.core.set_setup_profile(SetupProfile::Voiceover).await.unwrap();
    assert!(sw.needs_scan && sw.applied.is_none());
    assert_eq!(sw.profile, SetupProfile::Voiceover);
    let ctx = w.core.mic_context(&mic_id(&w), &MicAnswers::default()).unwrap();
    assert_eq!(ctx.profile, SetupProfile::Voiceover);
    let r = flow(&w, vec![]).await;
    assert_eq!(r.profile, SetupProfile::Voiceover);
    // A new AppCore on the same data dir reads the same profile back.
    let again = AppCore::new(MockPlatform::demo().platform(), w.core.data_dir.clone(), Arc::new(CollectingEmitter::default()));
    assert_eq!(again.active_profile(), SetupProfile::Voiceover);
}

#[tokio::test]
async fn one_setup_per_profile_and_switching_backs_up_and_reapplies() {
    let w = world().await;
    let id = mic_id(&w);
    let streaming = flow(&w, vec![]).await;
    let s = w.core.stored_mic_setup(SetupProfile::Streaming, &id).expect("streaming stored");
    assert_eq!(s.mic_chain, streaming.chain);
    assert!(s.score > 0 && !s.targets.is_empty() && s.obs_input.as_deref() == Some("Mic/Aux"));

    // Call has nothing yet: scan needed, and streaming's "I can't" answers don't follow.
    let sw = w.core.set_setup_profile(SetupProfile::Call).await.unwrap();
    assert!(sw.needs_scan);
    assert!(w.core.stored_mic_setup(SetupProfile::Call, &id).is_none());
    flow(&w, vec![Constraint::CantMoveMicCloser]).await;
    let call = w.core.stored_mic_setup(SetupProfile::Call, &id).unwrap();
    assert_eq!(call.profile, SetupProfile::Call);
    assert_eq!(call.constraints, vec![Constraint::CantMoveMicCloser]);

    // Back to streaming: its stored setup is re-applied after a backup.
    let before = w.core.list_backups().len();
    let sw = w.core.set_setup_profile(SetupProfile::Streaming).await.unwrap();
    assert!(!sw.needs_scan);
    assert!(sw.applied.unwrap().applied.contains(&"chain_json".to_string()));
    let backups = w.core.list_backups();
    assert_eq!(backups.len(), before + 1);
    assert!(backups.iter().any(|b| b.label == "Before switching to Streaming"), "{:?}", backups.iter().map(|b| &b.label).collect::<Vec<_>>());
    let written: coach_core::audio::ProcessingChain = serde_json::from_str(&std::fs::read_to_string(w.core.data_dir.join("chain.json")).unwrap()).unwrap();
    assert_eq!(written, streaming.chain);
    // The call setup is untouched, and switching did not store anything new.
    assert_eq!(w.core.stored_mic_setup(SetupProfile::Call, &id).unwrap().saved_at, call.saved_at);
}

#[tokio::test]
async fn old_setup_json_is_the_streaming_setup() {
    let w = world().await;
    let id = mic_id(&w);
    let r = flow(&w, vec![]).await;
    w.core.save_setup("Mine").unwrap();
    // Strip the new fields, as an older app wrote the file.
    let path = w.core.data_dir.join("setup.json");
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("mic_setups");
    v.as_object_mut().unwrap().remove("active_profile");
    std::fs::write(&path, v.to_string()).unwrap();
    assert_eq!(w.core.active_profile(), SetupProfile::Streaming);
    assert_eq!(w.core.stored_mic_setup(SetupProfile::Streaming, &id).unwrap().mic_chain, r.chain);
    assert!(w.core.stored_mic_setup(SetupProfile::Voiceover, &id).is_none());
    // Startup re-apply uses it.
    let res = w.core.reapply_saved_setup().await;
    assert!(res.applied.contains(&"chain_json".to_string()), "{res:?}");
    // Under another profile the old chain is not applied.
    w.core.set_setup_profile(SetupProfile::Voiceover).await.unwrap();
    let res = w.core.reapply_saved_setup().await;
    assert!(!res.applied.contains(&"chain_json".to_string()));
    // save_setup keeps the profile data.
    w.core.save_setup("Mine").unwrap();
    assert_eq!(w.core.active_profile(), SetupProfile::Voiceover);
}
