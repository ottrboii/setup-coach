//! Which findings a setup profile shows, advice that matches what the mic can
//! do about its own level, and the reflection finding's wording.

use coach_core::audio::{
    self,
    findings::{Connection, GainAccess, GainFacts, InterfaceGain, MicContext},
    synth::{build, Scenario},
};
use coach_core::{Area, Finding, FixKind, SetupProfile, Severity};

fn d() -> Scenario {
    Scenario::default()
}

fn scan(s: &Scenario, ctx: &MicContext) -> audio::MicReport {
    audio::scan(&build(s), ctx, &[])
}

fn fnd(id: &str, area: Area) -> Finding {
    Finding::new(id, area, Severity::Major, "t", "w", "f", FixKind::Manual)
}

// ---------------------------------------------------------------- relevance

#[test]
fn voiceover_hides_keyboard_streaming_and_call_keep_it() {
    let s = Scenario { typing_db: -8.0, ..d() };
    let has = |p| scan(&s, &MicContext { profile: p, ..Default::default() }).findings.iter().any(|f| f.id == "mic.keyboard");
    assert!(has(SetupProfile::Streaming));
    assert!(has(SetupProfile::Call), "typing is heard on a call");
    assert!(!has(SetupProfile::Voiceover));
}

#[test]
fn hidden_findings_cost_no_score() {
    let s = Scenario { typing_db: -8.0, ..d() };
    let v = scan(&s, &MicContext { profile: SetupProfile::Voiceover, ..Default::default() });
    let st = scan(&s, &MicContext { profile: SetupProfile::Streaming, ..Default::default() });
    assert!(v.score.value >= st.score.value);
}

#[test]
fn relevance_table() {
    use SetupProfile::*;
    let cases: &[(&str, Area, bool, bool, bool)] = &[
        // id, area, streaming, voiceover, call
        ("camera.backlit", Area::Camera, true, false, true),
        ("camera.studio_effects_on", Area::Camera, true, false, true),
        ("camera.hdmi_no_signal", Area::Camera, true, false, false),
        ("connection.usb3_on_usb2_port", Area::Camera, true, false, true),
        ("connection.cameras_share_controller", Area::Camera, true, false, true),
        ("connection.capture_card_behind_hub", Area::Camera, true, false, false),
        ("connection.behind_hub", Area::Mic, true, true, true),
        ("connection.device_dropouts", Area::Mic, true, true, true),
        ("connection.mic_held_by_app", Area::Mic, true, true, true),
        ("stream.cpu_maxed", Area::Stream, true, false, false),
        ("stream.bitrate_over_upload", Area::Stream, true, false, false),
        ("stream.recording_plain_mp4", Area::Stream, true, true, false),
        ("stream.on_wifi", Area::Stream, true, false, true),
        ("mic.keyboard", Area::Mic, true, false, true),
        ("mic.capture_card_audio_and_mic", Area::Mic, true, false, false),
        ("system.obs_websocket_off", Area::System, true, true, false),
        ("system.obs_outdated", Area::System, true, true, false),
    ];
    for (id, area, st, vo, ca) in cases {
        let f = fnd(id, *area);
        assert_eq!(Streaming.is_relevant(&f), *st, "{id} streaming");
        assert_eq!(Voiceover.is_relevant(&f), *vo, "{id} voiceover");
        assert_eq!(Call.is_relevant(&f), *ca, "{id} call");
    }
}

/// Anything that changes how the voice sounds, or whether the mic works at
/// all, is shown in every profile (the only mic.* exceptions are the two in the table).
#[test]
fn voice_quality_findings_are_never_hidden() {
    let voice = [
        "mic.no_signal", "mic.one_channel_only", "mic.stereo_mode", "mic.too_quiet", "mic.too_hot", "mic.clipping", "mic.near_clipping",
        "mic.not_enough_gain", "mic.background_noise", "mic.hum", "mic.phone_quality", "mic.too_far", "mic.roomy", "mic.reflection",
        "mic.plosives", "mic.sibilance", "mic.boomy", "mic.muddy", "mic.boxy", "mic.thin", "mic.harsh", "mic.dull", "mic.nasal",
        "mic.desk_thumps", "mic.cable_crackle", "mic.level_swings", "mic.auto_level_active", "mic.dc_offset", "mic.wrong_side",
        "mic.ai_noise_suppression", "mic.gain_stacked", "mic.left_channel_only", "mic.processing_in_two_places", "mic.obs_mic_muted",
        "system.audio_enhancements_on", "system.voice_clarity_on", "system.mic_privacy_blocked", "system.sample_rate_mismatch",
        "system.mic_boost_on", "system.mic_volume_below_100", "system.mic_level_changed_by_app", "system.bluetooth_handsfree",
        "system.windows_ducking_on", "system.listen_to_device_on", "system.exclusive_mode_allowed", "system.usb_selective_suspend_on",
        "system.goxlr_official_app", "connection.mic_held_by_app",
    ];
    for p in SetupProfile::ALL {
        for id in voice {
            let area = if id.starts_with("system.") { Area::System } else { Area::Mic };
            assert!(p.is_relevant(&fnd(id, area)), "{id} must stay visible for {}", p.id());
        }
    }
}

#[test]
fn filter_and_info_agree() {
    let ctx = MicContext::default();
    let info = SetupProfile::Voiceover.info(&ctx);
    assert!(info.irrelevant.areas.contains(&Area::Camera) && info.irrelevant.areas.contains(&Area::Stream));
    assert!(info.irrelevant.ids.iter().any(|i| i == "mic.keyboard"));
    assert_eq!(SetupProfile::Streaming.info(&ctx).irrelevant, Default::default(), "streaming hides nothing");
    let mut v = vec![fnd("camera.backlit", Area::Camera), fnd("mic.too_quiet", Area::Mic), fnd("stream.low_fps", Area::Stream), fnd("mic.keyboard", Area::Mic)];
    SetupProfile::Voiceover.filter_findings(&mut v);
    assert_eq!(v.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["mic.too_quiet"]);
    // Round trip for the UI contract.
    let j = serde_json::to_value(&info).unwrap();
    assert_eq!(j["irrelevant"]["areas"][0], "camera");
}

// ---------------------------------------------------------------- gain advice

fn hot() -> Scenario {
    Scenario { speech_level_dbfs: -10.0, ..d() }
}

fn ctx_gain(g: GainAccess, w: Option<&str>) -> MicContext {
    MicContext { mic_name: Some("Razer Seiren".into()), gain: g, gain_where: w.map(Into::into), ..Default::default() }
}

fn fix_of(r: &audio::MicReport, id: &str) -> String {
    let f = r.findings.iter().find(|f| f.id == id).unwrap_or_else(|| panic!("no {id}: {:?}", r.findings.iter().map(|f| &f.id).collect::<Vec<_>>()));
    f.fix.clone()
}

#[test]
fn no_gain_control_never_says_lower_your_gain() {
    let r = scan(&hot(), &ctx_gain(GainAccess::None, None));
    for id in ["mic.too_hot", "mic.clipping"] {
        let fix = fix_of(&r, id).to_lowercase();
        assert!(!fix.contains("turn the gain") && !fix.contains("lower the gain") && !fix.contains("gain knob"), "{id}: {fix}");
        assert!(fix.contains("no gain control"), "{id}: {fix}");
        assert!(fix.contains("farther") && fix.contains("off to the side"), "{id}: {fix}");
        assert!(fix.contains("pad") && fix.contains("mixer or interface") && fix.contains("software"), "{id}: {fix}");
    }
    // Every finding of a no-gain mic, noise ones included.
    let noisy = Scenario { hiss_dbfs: -40.0, ..d() };
    for g in [GainAccess::None] {
        let r = scan(&noisy, &ctx_gain(g, None));
        for f in &r.findings {
            let t = f.fix.to_lowercase();
            assert!(!t.contains("lower the gain") && !t.contains("turn the gain") && !t.contains("gain knob"), "{}: {t}", f.id);
        }
    }
    let r = scan(&Scenario { speech_level_dbfs: -42.0, ..d() }, &ctx_gain(GainAccess::None, None));
    let q = fix_of(&r, "mic.too_quiet").to_lowercase();
    assert!(!q.contains("turn the gain") && q.contains("software"), "{q}");
}

#[test]
fn advice_matches_where_the_gain_lives() {
    let knob = scan(&hot(), &ctx_gain(GainAccess::Knob, None));
    assert!(fix_of(&knob, "mic.too_hot").contains("Turn the gain knob down"));
    let sw = scan(&hot(), &ctx_gain(GainAccess::Software, Some("GoXLR Utility")));
    let t = fix_of(&sw, "mic.too_hot");
    assert!(t.contains("Lower the mic gain") && t.contains("GoXLR Utility"), "{t}");
    let unknown = scan(&hot(), &ctx_gain(GainAccess::Unknown, None));
    let t = fix_of(&unknown, "mic.too_hot");
    assert!(t.contains("If Razer Seiren has a gain control") && t.contains("if not"), "{t}");
    // Default context (old saved contexts) is the unknown wording.
    assert_eq!(MicContext::default().gain, GainAccess::Unknown);
}

#[test]
fn chain_trims_in_software_when_there_is_no_gain_to_turn() {
    let none = scan(&hot(), &ctx_gain(GainAccess::None, None));
    assert_eq!(none.chain.hardware_gain_change_db, 0.0, "no knob advice");
    assert!(none.chain.input_gain_db < -1.0, "cut goes into the software gain stage: {}", none.chain.input_gain_db);
    assert!(none.chain.notes.iter().any(|n| n.contains("no gain control")));
    let knob = scan(&hot(), &ctx_gain(GainAccess::Knob, None));
    assert!(knob.chain.hardware_gain_change_db < -1.0 && knob.chain.input_gain_db == 0.0);
    // Quiet with no gain: the missing level is added digitally, never "turn the knob up".
    let q = scan(&Scenario { speech_level_dbfs: -42.0, ..d() }, &ctx_gain(GainAccess::None, None));
    assert_eq!(q.chain.hardware_gain_change_db, 0.0);
    assert!(q.chain.input_gain_db > 5.0);
}

#[test]
fn gain_access_from_the_device_profile() {
    let none: &[String] = &[];
    let knob = vec!["Mute".to_string(), "Gain knob".to_string()];
    let r = |f: GainFacts| GainAccess::resolve(&f);
    // An interface decides everything.
    assert_eq!(r(GainFacts { interface: Some(InterfaceGain::Preamp), hardware_controls: none, ..Default::default() }).0, GainAccess::Knob);
    let (g, w) = r(GainFacts { interface: Some(InterfaceGain::VendorApp("the GoXLR app".into())), hardware_controls: none, ..Default::default() });
    assert_eq!((g, w.as_deref()), (GainAccess::Software, Some("the GoXLR app")));
    // A listed gain control, or a Windows slider that is the real gain.
    assert_eq!(r(GainFacts { hardware_controls: &knob, ..Default::default() }).0, GainAccess::Knob);
    let (g, w) = r(GainFacts { windows_slider_is_hw_gain: Some(true), hardware_controls: none, ..Default::default() });
    assert_eq!((g, w.as_deref()), (GainAccess::Software, Some("Windows sound settings")));
    // Headsets and built-ins have none. Unlisted controls on a USB mic are NOT proof.
    for c in [Connection::Headset, Connection::Bluetooth, Connection::BuiltIn] {
        assert_eq!(r(GainFacts { connection: Some(c), hardware_controls: none, ..Default::default() }).0, GainAccess::None);
    }
    assert_eq!(r(GainFacts { connection: Some(Connection::Usb), hardware_controls: none, ..Default::default() }).0, GainAccess::Unknown);
    assert_eq!(r(GainFacts { connection: Some(Connection::Usb), windows_slider_is_hw_gain: Some(false), hardware_controls: none, profile_verified: false, ..Default::default() }).0, GainAccess::Unknown);
    assert_eq!(r(GainFacts { connection: Some(Connection::Usb), windows_slider_is_hw_gain: Some(false), hardware_controls: none, profile_verified: true, ..Default::default() }).0, GainAccess::None);
}

// ---------------------------------------------------------------- reflection

#[test]
fn reflection_finding_is_plain_honest_and_testable() {
    let s = Scenario { reflection: Some((1.2, 0.6)), ..d() };
    let r = scan(&s, &MicContext::default());
    let f = r.findings.iter().find(|f| f.id == "mic.reflection").expect("reflection finding");
    assert!(!f.title.to_lowercase().contains("phasey"), "{}", f.title);
    // Measured distance, labelled as measured; the surface is labelled as a guess.
    assert!(f.why.contains("We measured this in your recording") && f.why.contains("is our guess"), "{}", f.why);
    let delay = f.evidence["delay_ms"];
    let extra = f.evidence["extra_path_cm"];
    assert!((extra - delay * 34.3).abs() < 1.0);
    assert_eq!(f.evidence["surface_distance_cm_estimate"], (extra / 2.0).round());
    // Rounded to 5 cm and said as "roughly".
    let said = ((extra / 2.0 / 5.0).round() * 5.0) as i64;
    assert!(f.why.contains(&format!("roughly {said} cm")), "{}", f.why);
    // A quick test, and the re-measure closes the loop.
    assert!(f.fix.contains("towel or mouse pad") && f.fix.contains("test again"), "{}", f.fix);
    assert_eq!(f.fix_kind, FixKind::Manual);
    // The towel test: the same room with the reflection gone no longer raises it.
    let after = scan(&Scenario { reflection: None, ..s }, &MicContext::default());
    assert!(after.findings.iter().all(|f| f.id != "mic.reflection"));
}
