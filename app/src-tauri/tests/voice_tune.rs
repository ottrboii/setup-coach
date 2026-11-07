//! "Make it sound better" on the mock platform: compose + offline verify from
//! the last mic test's reading, loudness-matched Before / After, apply with a
//! vault backup, undo, "Gentler", the voiceover goal, and nothing applied when
//! the tune doesn't verify.

use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::tune::{FallbackReason, Verdict};
use coach_core::{SetupProfile, VoiceoverTarget};
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::obs::mock::FakeObs;
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::types::*;
use std::sync::Arc;
use std::time::Duration;

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

/// The mic test of the UI: context, recording with marks, finish.
fn mic_test(w: &World, s: &Scenario) {
    let ctx = w.core.mic_context(&mic_id(w), &MicAnswers::default()).unwrap();
    let test = build(s);
    *w.mock.audio.recording.lock().unwrap() = Some(test.clone());
    w.core.start_mic_test(&mic_id(w)).unwrap();
    for seg in &test.segments {
        w.core.mark_step_at(seg.kind, Edge::Start, seg.start_s).unwrap();
        w.core.mark_step_at(seg.kind, Edge::End, seg.end_s).unwrap();
    }
    w.core.finish_mic_test(ctx, vec![]).unwrap();
}

/// A slightly dark voice: the tune brightens it and verifies (verdict "close" on the mock mic).
fn dark() -> Scenario {
    Scenario { voice_tilt_db_oct: -2.0, ..Default::default() }
}

fn chain_json(w: &World) -> Option<String> {
    std::fs::read_to_string(w.core.data_dir.join("chain.json")).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn needs_a_fresh_reading_of_this_mic() {
    let w = world().await;
    let id = mic_id(&w);
    assert_eq!(w.core.start_voice_tune(&id).unwrap_err().code, VoiceTuneErrorCode::NeedsRead);
    assert_eq!(w.core.start_voice_tune("nope").unwrap_err().code, VoiceTuneErrorCode::MicMissing);
    mic_test(&w, &dark());
    assert!(w.core.start_voice_tune(&id).is_ok());
    // Switching the profile restamps the reading's context: the same voice is judged against the new goal.
    w.core.set_setup_profile(SetupProfile::Call).await.unwrap();
    let r = w.core.start_voice_tune(&id).unwrap();
    assert_eq!((r.profile, r.facts.goal.as_str()), (SetupProfile::Call, "Goal: Calls and meetings"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_reading_is_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    let mut core = AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default()));
    core.goxlr_autodetect.store(false, std::sync::atomic::Ordering::SeqCst);
    core.voice_tune_max_age = Duration::from_millis(1);
    let w = World { core: Arc::new(core), mock, _dir: dir };
    mic_test(&w, &dark());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(w.core.start_voice_tune(&mic_id(&w)).unwrap_err().code, VoiceTuneErrorCode::NeedsRead);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_and_undo_round_trip() {
    let w = world().await;
    let id = mic_id(&w);
    // Today's setup first, so there is something to go back to.
    mic_test(&w, &dark());
    let ctx = w.core.mic_context(&id, &MicAnswers::default()).unwrap();
    let mut base = w.core.rescan_mic(ctx, vec![]).unwrap().chain;
    base.eq.clear(); // an older setup without the tone moves
    w.core.apply_mic_chain(base.clone(), "Mic/Aux").await;
    let before_json = chain_json(&w).expect("chain.json written");

    let r = w.core.start_voice_tune(&id).unwrap();
    assert!(r.can_apply && r.verdict.applied(), "{:?} {}", r.verdict, r.headline);
    assert!(r.advanced.fallback.is_none());
    assert!(!r.applied && r.backup_id.is_none() && r.strength == 1.0 && r.can_be_gentler);
    assert!(r.facts.voice.starts_with("Your voice:") && r.facts.mic.starts_with("Your mic:") && r.facts.goal == "Goal: Streaming");
    assert!(!r.changes.is_empty() && !r.summary.is_empty());
    assert!(r.advanced.verification.is_some() && r.advanced.verification_note.contains("offline"));
    // Nothing applied by looking.
    assert_eq!(chain_json(&w).unwrap(), before_json);

    // Before / After files exist and are equally loud.
    let (bp, ap) = (r.before_path.clone().unwrap(), r.after_path.clone().unwrap());
    let (b, a) = (setup_coach_app::wav::read(std::path::Path::new(&bp)).unwrap(), setup_coach_app::wav::read(std::path::Path::new(&ap)).unwrap());
    let lufs = |w: &setup_coach_app::wav::Wav| coach_core::audio::dsp::integrated_lufs(&w.samples, w.sample_rate);
    assert!((lufs(&b) - lufs(&a)).abs() < 0.3, "{} vs {}", lufs(&b), lufs(&a));
    let diff = b.samples.iter().zip(&a.samples).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(diff > 1e-3, "before and after differ: {diff} (bands {:?})", r.advanced.bands);
    w.core.play_preview(PreviewWhich::TuneBefore, false).unwrap();
    w.core.play_preview(PreviewWhich::TuneAfter, true).unwrap();

    // Keep it.
    let applied = w.core.apply_voice_tune(&r.tune_id).await.unwrap();
    // The fake OBS can't load our VST3: that, and only that, is reported as not switched on.
    assert!(applied.applied.failed.iter().all(|f| f.id == "vst3"), "{:?}", applied.applied.failed);
    let backup = applied.backup_id.clone().expect("a backup id to undo with");
    let tuned_json = chain_json(&w).unwrap();
    assert_ne!(tuned_json, before_json);
    let tuned: coach_core::audio::ProcessingChain = serde_json::from_str(&tuned_json).unwrap();
    assert_eq!(tuned.eq, r.advanced.bands.iter().map(|b| b.band.clone()).collect::<Vec<_>>());
    // The stored setup follows (so switching profiles keeps the tune).
    assert_eq!(w.core.stored_mic_setup(SetupProfile::Streaming, &id).unwrap().mic_chain, tuned);
    assert!(w.core.vault().list().iter().any(|b| b.id == backup));

    // Undo puts back the chain and the stored setup.
    let back = w.core.undo_voice_tune(&backup).await;
    assert!(back.failed.is_empty(), "{:?}", back.failed);
    assert_eq!(chain_json(&w).unwrap(), before_json);
    assert_eq!(w.core.stored_mic_setup(SetupProfile::Streaming, &id).unwrap().mic_chain, base);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gentler_halves_the_moves_and_keeps_the_first_undo_point() {
    let w = world().await;
    let id = mic_id(&w);
    mic_test(&w, &dark());
    let full = w.core.start_voice_tune(&id).unwrap();
    assert!(full.can_apply);
    let half = w.core.gentler_voice_tune(&full.tune_id).unwrap();
    assert_eq!(half.strength, 0.5);
    assert_eq!(half.verdict, full.verdict);
    assert_eq!(half.advanced.bands.len(), full.advanced.bands.len());
    for (h, f) in half.advanced.bands.iter().zip(&full.advanced.bands) {
        assert!((h.band.gain_db - f.band.gain_db * 0.5).abs() <= 0.06, "{} vs {}", h.band.gain_db, f.band.gain_db);
    }
    let quarter = w.core.gentler_voice_tune(&full.tune_id).unwrap();
    assert_eq!(quarter.strength, 0.25);
    assert!(!quarter.can_be_gentler);
    assert_eq!(w.core.gentler_voice_tune(&full.tune_id).unwrap().strength, 0.25);

    // Apply the full one, then the gentler one: one undo point, the first.
    let again = w.core.gentler_voice_tune(&full.tune_id);
    assert!(again.is_ok());
    let a = w.core.apply_voice_tune(&full.tune_id).await.unwrap();
    let b = w.core.apply_voice_tune(&full.tune_id).await.unwrap();
    assert!(a.backup_id.is_some() && a.backup_id == b.backup_id);
    // Unknown / old tune ids are refused.
    assert_eq!(w.core.gentler_voice_tune("tune-999").unwrap_err().code, VoiceTuneErrorCode::Stale);
    assert_eq!(w.core.apply_voice_tune("tune-999").await.unwrap_err().code, VoiceTuneErrorCode::Stale);
    // A new tune replaces the old one.
    let newer = w.core.start_voice_tune(&id).unwrap();
    assert_ne!(newer.tune_id, full.tune_id);
    assert_eq!(w.core.apply_voice_tune(&full.tune_id).await.unwrap_err().code, VoiceTuneErrorCode::Stale);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_change_applies_nothing() {
    let w = world().await;
    let id = mic_id(&w);
    mic_test(&w, &Scenario::default());
    let r = w.core.start_voice_tune(&id).unwrap();
    assert_eq!(r.verdict, Verdict::NothingToChange);
    assert!(!r.can_apply && !r.can_be_gentler && r.before_path.is_none() && r.after_path.is_none() && r.changes.is_empty());
    assert!(r.headline.starts_with("Nothing to change"));
    let e = w.core.apply_voice_tune(&r.tune_id).await.unwrap_err();
    assert_eq!(e.code, VoiceTuneErrorCode::NothingToApply);
    assert_eq!(w.core.gentler_voice_tune(&r.tune_id).unwrap_err().code, VoiceTuneErrorCode::NothingToApply);
    assert!(chain_json(&w).is_none(), "no chain.json written");
    assert!(w.core.vault().list().is_empty(), "no backup, nothing changed");
    assert!(w.core.play_preview(PreviewWhich::TuneAfter, false).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_improved_is_never_applied() {
    // A sibilant voice: a de-esser alone raises the measured "s", so verification rejects it.
    let w = world().await;
    let id = mic_id(&w);
    mic_test(&w, &Scenario { sibilance_db: -2.0, sib_peak_hz: Some(7000.0), ..Default::default() });
    let r = w.core.start_voice_tune(&id).unwrap();
    if r.verdict.applied() {
        // The composer found a version that does verify: then it must have a verification that says so.
        let v = r.advanced.verification.as_ref().unwrap();
        assert!(v.improved && v.v2_no_harm && v.v4_cap);
        return;
    }
    assert_eq!(r.verdict, Verdict::NotImproved);
    assert!(!r.can_apply && r.headline.contains("settings stay as they are"));
    assert_eq!(w.core.apply_voice_tune(&r.tune_id).await.unwrap_err().code, VoiceTuneErrorCode::NothingToApply);
    assert!(chain_json(&w).is_none() && w.core.vault().list().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_or_noisy_reading_falls_back_and_says_so() {
    let w = world().await;
    let id = mic_id(&w);
    mic_test(&w, &Scenario { hiss_dbfs: -45.0, voice_tilt_db_oct: -3.0, ..dark() });
    let r = w.core.start_voice_tune(&id).unwrap();
    assert!(matches!(r.advanced.fallback, Some(FallbackReason::VoiceConfidence { .. })), "{:?}", r.advanced.fallback);
    // Not applied on a voice the app couldn't read: only the mic could be corrected, and there is no chart here.
    assert!(!r.can_apply, "{:?}", r.verdict);
    assert!(r.notes.iter().any(|n| n.contains("too short or unclear")), "{:?}", r.notes);
    assert!(r.facts.mic.contains("only listen to your voice") || r.facts.mic.contains("know how it colours"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voiceover_goal_is_saved_with_the_profile_and_reaches_the_context() {
    let w = world().await;
    let id = mic_id(&w);
    assert_eq!(w.core.voiceover_target(), VoiceoverTarget::Youtube);
    let info = w.core.voiceover_target_info();
    let ids: Vec<_> = info.options.iter().map(|o| o.id).collect();
    assert_eq!(ids, vec![VoiceoverTarget::Youtube, VoiceoverTarget::Podcast, VoiceoverTarget::Audiobook]);
    assert!(info.options.iter().all(|o| !o.label.is_empty() && !o.summary.is_empty()));
    assert!(!info.applies_now && !info.needs_rescan);
    // ProfileTuning is exposed per goal, with the speech-peak bound.
    let acx = info.options.iter().find(|o| o.id == VoiceoverTarget::Audiobook).unwrap();
    assert_eq!(acx.tuning.speech_peak_limit_db, acx.tuning.limiter_ceiling_db);
    assert!(serde_json::to_value(&acx.tuning).unwrap().get("speech_peak_limit_db").is_some());

    w.core.set_setup_profile(SetupProfile::Voiceover).await.unwrap();
    mic_test(&w, &dark());
    let podcast = w.core.set_voiceover_target(VoiceoverTarget::Podcast).unwrap();
    assert_eq!(podcast.selected, VoiceoverTarget::Podcast);
    assert!(podcast.applies_now && podcast.needs_rescan);
    assert_eq!(w.core.mic_context(&id, &MicAnswers::default()).unwrap().voiceover_target, VoiceoverTarget::Podcast);
    // The reading was scanned for the old goal: the result still comes from the saved goal, and says which.
    let r = w.core.start_voice_tune(&id).unwrap();
    assert_eq!(r.voiceover_target, Some(VoiceoverTarget::Podcast));
    assert_eq!(r.facts.goal, "Goal: Voiceover, Podcast");
    assert_eq!(r.advanced.tuning.loudness.value, -16.0);
    // Same value again: no rescan needed.
    assert!(!w.core.set_voiceover_target(VoiceoverTarget::Podcast).unwrap().needs_rescan);
    // Survives a restart, and old setups without the field read as YouTube.
    let again = AppCore::new(MockPlatform::demo().platform(), w.core.data_dir.clone(), Arc::new(CollectingEmitter::default()));
    assert_eq!(again.voiceover_target(), VoiceoverTarget::Podcast);
    // Keeps it when the setup is saved by name.
    w.core.save_setup("mine").unwrap();
    assert_eq!(w.core.voiceover_target(), VoiceoverTarget::Podcast);
    std::fs::write(w.core.data_dir.join("setup.json"), r#"{"name":"x","saved_at":"","mic_chain":null,"camera_settings":null,"camera_id":null,"obs_input":null}"#).unwrap();
    assert_eq!(w.core.voiceover_target(), VoiceoverTarget::Youtube);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changing_the_goal_drops_a_running_tune() {
    let w = world().await;
    let id = mic_id(&w);
    w.core.set_setup_profile(SetupProfile::Voiceover).await.unwrap();
    mic_test(&w, &dark());
    let r = w.core.start_voice_tune(&id).unwrap();
    w.core.set_voiceover_target(VoiceoverTarget::Audiobook).unwrap();
    assert_eq!(w.core.apply_voice_tune(&r.tune_id).await.unwrap_err().code, VoiceTuneErrorCode::Stale);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pass_says_checked() {
    let w = world().await;
    let id = mic_id(&w);
    mic_test(&w, &Scenario { tone: vec![(250.0, 5.0, 0.7), (315.0, 4.0, 0.7)], ..Default::default() });
    let r = w.core.start_voice_tune(&id).unwrap();
    assert_eq!(r.verdict, Verdict::Pass, "{}", r.headline);
    assert_eq!(r.headline, "Checked: it now sounds like we planned.");
    assert!(r.summary.iter().any(|l| l.id == "mud" && l.before == ToneState::TooMuch && l.after == ToneState::OnTarget), "{:?}", r.summary);
    let applied = w.core.apply_voice_tune(&r.tune_id).await.unwrap();
    assert!(applied.line.starts_with("Done.") || applied.line.starts_with("Your new settings are saved"), "{}", applied.line);
    assert!(applied.backup_id.is_some());
}