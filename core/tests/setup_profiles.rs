//! Setup profiles (streaming / voiceover / call): pass bars and guidance move,
//! the audio processing never does. Evidence for every number:
//! `cargo run --release --example profile_sweep` and `core/src/setup_profile.rs`.

use coach_core::audio::{
    self,
    findings::{Capsule, Connection, MicContext},
    synth::{build, Scenario},
    targets::{check_targets, check_targets_for, Status},
};
use coach_core::{Constraint, SetupProfile, Severity};

fn d() -> Scenario {
    Scenario::default()
}

fn ctx(p: SetupProfile) -> MicContext {
    MicContext { profile: p, ..Default::default() }
}

fn report(s: &Scenario, p: SetupProfile) -> audio::MicReport {
    audio::scan(&build(s), &ctx(p), &[])
}

fn finding(r: &audio::MicReport, id: &str) -> Option<Severity> {
    r.findings.iter().find(|f| f.id == id).map(|f| f.severity)
}

fn status(r: &audio::MicReport, id: &str) -> Status {
    r.targets.iter().find(|t| t.id == id).unwrap().status
}

// Streaming is the default and is exactly the behaviour before profiles.
#[test]
fn streaming_is_the_default_and_unchanged() {
    assert_eq!(MicContext::default().profile, SetupProfile::Streaming);
    let t = SetupProfile::Streaming.targets();
    assert_eq!((t.noise_floor.met, t.noise_floor.close, t.noise_floor.flag, t.noise_floor.major), (-60.0, -50.0, -50.0, -45.0));
    assert_eq!((t.snr.met, t.snr.close, t.snr.major), (50.0, 40.0, 30.0));
    assert_eq!((t.room_decay.met, t.room_decay.close, t.room_decay.flag, t.room_decay.major), (0.35, 0.5, 0.45, 0.7));
    for s in [d(), Scenario { hiss_dbfs: -55.0, ..d() }, Scenario { rt60_s: 0.6, ..d() }, Scenario { speech_level_dbfs: -36.0, ..d() }] {
        let r = report(&s, SetupProfile::Streaming);
        assert_eq!(r.profile, SetupProfile::Streaming);
        assert_eq!(r.targets, check_targets(&r.metrics));
    }
    let texts: Vec<String> = check_targets(&report(&d(), SetupProfile::Streaming).metrics).into_iter().map(|t| t.target).collect();
    assert_eq!(texts[..4], ["−18 to −12 dBFS", "≤ −6 dBFS", "≤ −60 dBFS", "≥ 50 dB"]);
    assert_eq!(texts[6], "≤ 0.35 s");
}

#[test]
fn bars_are_ordered_and_profiles_lean_the_right_way() {
    let s = SetupProfile::Streaming.targets();
    for p in SetupProfile::ALL {
        let t = p.targets();
        for b in [t.noise_floor, t.room_decay, t.loud_peak] {
            assert!(b.met <= b.close && b.flag <= b.close && b.met <= b.flag && b.close <= b.major, "{p:?} {b:?}");
        }
        assert!(t.snr.met >= t.snr.close && t.snr.close >= t.snr.major, "{p:?}");
        // Level and headroom are the same everywhere (no evidence to move them).
        assert_eq!((t.speech_peak, t.speech_peak_close, t.loud_peak), (s.speech_peak, s.speech_peak_close, s.loud_peak));
    }
    let (v, c) = (SetupProfile::Voiceover.targets(), SetupProfile::Call.targets());
    // Voiceover is never looser than streaming; call is never stricter.
    assert!(v.noise_floor.close <= s.noise_floor.close && v.snr.close >= s.snr.close && v.room_decay.flag <= s.room_decay.flag);
    assert!(c.noise_floor.close >= s.noise_floor.close && c.snr.close <= s.snr.close && c.room_decay.flag >= s.room_decay.flag);
}

#[test]
fn clean_voice_passes_every_profile() {
    for p in SetupProfile::ALL {
        let r = report(&d(), p);
        let bad: Vec<_> = r.findings.iter().filter(|f| f.severity >= Severity::Minor).map(|f| &f.id).collect();
        assert!(bad.is_empty(), "{p:?} flagged {bad:?}");
        for id in ["noise_floor", "snr", "room", "speech_peaks"] {
            assert_eq!(status(&r, id), Status::Met, "{p:?} {id}");
        }
    }
}

// A light room (RT60 0.3 s, measured ~0.37 s) is fine on stream but not for a voiceover.
#[test]
fn voiceover_is_strict_on_room_echo() {
    let s = Scenario { rt60_s: 0.3, reverb_wet_db: -6.0, ..d() };
    let st = report(&s, SetupProfile::Streaming);
    assert_eq!(finding(&st, "mic.roomy"), None);
    assert_eq!(status(&st, "room"), Status::Close);
    let vo = report(&s, SetupProfile::Voiceover);
    assert_eq!(finding(&vo, "mic.roomy"), Some(Severity::Minor));
    let f = vo.findings.iter().find(|f| f.id == "mic.roomy").unwrap();
    assert!(f.title.contains("a little echoey"), "{}", f.title);
    assert_eq!(status(&vo, "room"), Status::Missed);
    // A really echoey room is the same "bathroom" card everywhere.
    let big = Scenario { rt60_s: 0.9, reverb_wet_db: -2.0, ..d() };
    let f = report(&big, SetupProfile::Voiceover).findings.into_iter().find(|f| f.id == "mic.roomy").unwrap();
    assert!(f.title.contains("bathroom") && f.severity == Severity::Major);
}

// Hiss at -65 dBFS (≈38 dB under speech): minor on stream, major for voiceover, fine on a call.
#[test]
fn noise_bars_follow_the_profile() {
    let s = Scenario { hiss_dbfs: -65.0, ..d() };
    assert_eq!(finding(&report(&s, SetupProfile::Streaming), "mic.background_noise"), Some(Severity::Minor));
    assert_eq!(finding(&report(&s, SetupProfile::Voiceover), "mic.background_noise"), Some(Severity::Major));
    let call = report(&s, SetupProfile::Call);
    assert_eq!(finding(&call, "mic.background_noise"), None);
    assert_eq!(status(&call, "noise_floor"), Status::Met);
    assert_eq!(status(&call, "snr"), Status::Met);
}

// A noisy room (floor -50, ≈25 dB under speech) is major on stream and still OK on a call.
#[test]
fn call_is_tolerant() {
    let s = Scenario { hiss_dbfs: -50.0, ..d() };
    assert_eq!(finding(&report(&s, SetupProfile::Streaming), "mic.background_noise"), Some(Severity::Major));
    let call = report(&s, SetupProfile::Call);
    assert_eq!(finding(&call, "mic.background_noise"), None);
    assert_eq!(status(&call, "noise_floor"), Status::Met);
    // Floor -42 is a problem even on a call; -35 (≈10 dB under speech, below
    // the 15 dB intelligibility minimum) is a major one.
    assert_eq!(finding(&report(&Scenario { hiss_dbfs: -42.0, ..d() }, SetupProfile::Call), "mic.background_noise"), Some(Severity::Minor));
    assert_eq!(finding(&report(&Scenario { hiss_dbfs: -35.0, ..d() }, SetupProfile::Call), "mic.background_noise"), Some(Severity::Major));
    // RT60 0.6 s (classroom limit): roomy on stream, close on a call.
    let room = Scenario { rt60_s: 0.6, ..d() };
    assert!(finding(&report(&room, SetupProfile::Streaming), "mic.roomy").is_some());
    let call = report(&room, SetupProfile::Call);
    assert_eq!(finding(&call, "mic.roomy"), None);
    assert_ne!(status(&call, "room"), Status::Missed);
}

#[test]
fn profiles_never_change_the_audio() {
    for s in [d(), Scenario { hiss_dbfs: -55.0, rt60_s: 0.5, ..d() }, Scenario { speech_level_dbfs: -38.0, ..d() }] {
        let base = report(&s, SetupProfile::Streaming).chain;
        for p in SetupProfile::ALL {
            assert_eq!(report(&s, p).chain, base, "{p:?}");
        }
    }
}

#[test]
fn i_cant_do_this_still_recalibrates() {
    let s = Scenario { rt60_s: 0.3, reverb_wet_db: -6.0, ..d() };
    let r = audio::scan(&build(&s), &ctx(SetupProfile::Voiceover), &[Constraint::CantTreatRoom]);
    let f = r.findings.iter().find(|f| f.id == "mic.roomy").unwrap();
    assert_eq!(f.severity, Severity::Info);
    assert!(!f.can_decline);
    let s = Scenario { hiss_dbfs: -65.0, ..d() };
    let r = audio::scan(&build(&s), &ctx(SetupProfile::Voiceover), &[Constraint::CantRemoveNoiseSource]);
    assert_eq!(finding(&r, "mic.background_noise"), Some(Severity::Info));
}

#[test]
fn placement_guidance_per_profile() {
    let dynamic = MicContext { capsule: Some(Capsule::Dynamic), ..Default::default() };
    let at = |p: SetupProfile, c: &MicContext| p.placement(&MicContext { profile: p, ..c.clone() });
    assert!(at(SetupProfile::Streaming, &dynamic).contains("out of the camera's view"));
    assert!(at(SetupProfile::Streaming, &dynamic).contains("3–10 cm"));
    // Voiceover: the closer half of the range.
    assert_eq!(SetupProfile::Voiceover.distance_cm(&dynamic), Some((3.0, 7.0)));
    assert!(at(SetupProfile::Voiceover, &dynamic).contains("3–7 cm"));
    let cond = MicContext { capsule: Some(Capsule::Condenser), distance_cm: Some((15.0, 25.0)), ..Default::default() };
    assert_eq!(SetupProfile::Voiceover.distance_cm(&cond), Some((15.0, 20.0)));
    let headset = MicContext { connection: Some(Connection::Headset), ..Default::default() };
    assert!(at(SetupProfile::Call, &headset).contains("corner of your mouth"));
    assert_eq!(SetupProfile::Call.distance_cm(&headset), Some((1.0, 3.0)));
    // Findings use the profile's distance words.
    let far = Scenario { rt60_s: 0.9, reverb_wet_db: 0.0, speech_level_dbfs: -36.0, ..d() };
    let r = audio::scan(&build(&far), &MicContext { profile: SetupProfile::Voiceover, ..dynamic.clone() }, &[]);
    assert!(r.findings.iter().find(|f| f.id == "mic.too_far").unwrap().fix.contains("3–7 cm"));
    for p in SetupProfile::ALL {
        let info = p.info(&dynamic);
        assert!(!info.label.is_empty() && !info.summary.is_empty() && !info.room_advice.is_empty());
        assert_eq!(info.targets, p.targets());
        // Plain language: no numbers outside the distance words.
        assert!(!info.summary.chars().any(|c| c.is_ascii_digit()) && !info.room_advice.chars().any(|c| c.is_ascii_digit()));
    }
    let v = serde_json::to_value(SetupProfile::Voiceover.info(&dynamic)).unwrap();
    assert_eq!(v["id"], "voiceover");
    assert_eq!(v["distance_cm"], serde_json::json!([3.0, 7.0]));
    assert_eq!(v["targets"]["room_decay"]["met"], 0.27);
}

#[test]
fn profile_serializes_and_old_contexts_load() {
    assert_eq!(serde_json::to_string(&SetupProfile::Voiceover).unwrap(), "\"voiceover\"");
    for p in SetupProfile::ALL {
        assert_eq!(serde_json::to_value(p).unwrap(), p.id());
    }
    let old: MicContext = serde_json::from_str(r#"{"mic_name":"RE20","capsule":"dynamic","address":null,"connection":null,"gain_needed_db":null,"interface_max_gain_db":null,"booster_db":null,"gain_at_max":null,"onboard_auto_level":null,"distance_cm":null}"#).unwrap();
    assert_eq!(old.profile, SetupProfile::Streaming);
    let r = report(&d(), SetupProfile::Call);
    let mut v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["profile"], "call");
    v.as_object_mut().unwrap().remove("profile");
    let back: audio::MicReport = serde_json::from_value(v).unwrap();
    assert_eq!(back.profile, SetupProfile::Streaming);
    assert_eq!(check_targets_for(&r.metrics, SetupProfile::Call), r.targets);
}
