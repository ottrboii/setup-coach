//! End-to-end mic scans on synthetic recordings with known faults.

use coach_core::audio::{
    self, analyze, compute_chain,
    findings::{Address, Capsule, Connection, MicContext},
    process::render_test,
    synth::{build, Scenario},
};
use coach_core::{Constraint, Severity};

fn ids(s: &Scenario, ctx: &MicContext) -> Vec<String> {
    let r = audio::scan(&build(s), ctx, &[]);
    r.findings.iter().map(|f| f.id.clone()).collect()
}

fn has(s: Scenario, id: &str) {
    let found = ids(&s, &MicContext::default());
    assert!(found.iter().any(|f| f == id), "expected {id}, got {found:?}");
}

fn d() -> Scenario {
    Scenario::default()
}

#[test]
fn clean_voice_has_no_problems() {
    let r = audio::scan(&build(&d()), &MicContext::default(), &[]);
    let bad: Vec<_> = r.findings.iter().filter(|f| f.severity >= Severity::Minor).map(|f| &f.id).collect();
    assert!(bad.is_empty(), "clean scenario flagged {bad:?}");
    assert!(r.score.value >= 95);
    assert!((r.metrics.f0_hz.unwrap() - 120.0).abs() < 10.0);
    assert!(r.metrics.reverb_decay_s.unwrap() < 0.3);
    assert!(r.metrics.dead_channel.is_none());
}

#[test]
fn clean_high_voice_has_no_major_problems() {
    let r = audio::scan(&build(&Scenario { f0_hz: 210.0, ..d() }), &MicContext::default(), &[]);
    assert!(r.findings.iter().all(|f| f.severity < Severity::Major), "{:?}", r.findings.iter().map(|f| &f.id).collect::<Vec<_>>());
    assert!((r.metrics.f0_hz.unwrap() - 210.0).abs() < 20.0);
}

#[test]
fn silent_input() {
    has(Scenario { silent: true, ..d() }, "mic.no_signal");
}

#[test]
fn left_channel_only() {
    let t = build(&Scenario { left_only: true, ..d() });
    let r = audio::scan(&t, &MicContext::default(), &[]);
    assert_eq!(r.findings[0].id, "mic.one_channel_only");
    assert!(r.chain.mono_downmix);
}

#[test]
fn levels() {
    has(Scenario { speech_level_dbfs: -42.0, ..d() }, "mic.too_quiet");
    let hot = ids(&Scenario { speech_level_dbfs: -10.0, ..d() }, &MicContext::default());
    assert!(hot.contains(&"mic.clipping".to_string()) && hot.contains(&"mic.too_hot".to_string()), "{hot:?}");
}

#[test]
fn noise_hum_and_bandwidth() {
    has(Scenario { hiss_dbfs: -50.0, ..d() }, "mic.background_noise");
    has(Scenario { fan_dbfs: Some(-52.0), ..d() }, "mic.background_noise");
    let m = analyze(&build(&Scenario { hum: Some((60.0, -62.0)), ..d() }));
    assert_eq!(m.hum.unwrap().mains_hz, 60.0);
    let m = analyze(&build(&Scenario { hum: Some((50.0, -62.0)), ..d() }));
    assert_eq!(m.hum.unwrap().mains_hz, 50.0);
    has(Scenario { bandlimit_hz: Some(3400.0), ..d() }, "mic.phone_quality");
}

#[test]
fn noise_character() {
    use coach_core::audio::analyze::NoiseCharacter;
    assert_eq!(analyze(&build(&Scenario { hiss_dbfs: -50.0, ..d() })).noise_character, NoiseCharacter::Hiss);
    assert_eq!(analyze(&build(&Scenario { fan_dbfs: Some(-52.0), ..d() })).noise_character, NoiseCharacter::Rumble);
}

#[test]
fn room_and_reflections() {
    has(Scenario { rt60_s: 0.9, reverb_wet_db: -2.0, ..d() }, "mic.roomy");
    let m = analyze(&build(&Scenario { reflection: Some((1.2, 0.6)), ..d() }));
    let r = m.reflection.expect("reflection");
    assert!((r.delay_ms - 1.2).abs() < 0.15, "{r:?}");
    assert!(analyze(&build(&d())).reflection.is_none());
}

#[test]
fn far_from_mic_is_one_finding() {
    let s = Scenario { rt60_s: 0.9, reverb_wet_db: 0.0, speech_level_dbfs: -36.0, ..d() };
    let found = ids(&s, &MicContext::default());
    assert!(found.contains(&"mic.too_far".to_string()), "{found:?}");
    assert!(!found.contains(&"mic.roomy".to_string()));
}

#[test]
fn consonants() {
    has(Scenario { plosive_db: 4.0, ..d() }, "mic.plosives");
    has(Scenario { sibilance_db: 2.0, ..d() }, "mic.sibilance");
    assert_eq!(analyze(&build(&d())).plosive_events, 0);
}

#[test]
fn tone() {
    has(Scenario { tone: vec![(110.0, 11.0, 0.9)], speech_level_dbfs: -30.0, ..d() }, "mic.boomy");
    has(Scenario { tone: vec![(300.0, 10.0, 1.0)], speech_level_dbfs: -30.0, ..d() }, "mic.muddy");
    has(Scenario { tone: vec![(3500.0, -10.0, 0.8), (8000.0, -10.0, 0.8)], ..d() }, "mic.dull");
    has(Scenario { tone: vec![(3500.0, 9.0, 1.0)], speech_level_dbfs: -30.0, ..d() }, "mic.harsh");
}

#[test]
fn other_steps() {
    has(Scenario { typing_db: -8.0, ..d() }, "mic.keyboard");
    has(Scenario { tap_db: 0.0, ..d() }, "mic.desk_thumps");
    has(Scenario { dropouts: true, ..d() }, "mic.cable_crackle");
    has(Scenario { moving_drop_db: 9.0, ..d() }, "mic.level_swings");
    has(Scenario { auto_level: true, ..d() }, "mic.auto_level_active");
}

#[test]
fn sm7b_on_small_interface_suggests_booster() {
    let ctx = MicContext {
        mic_name: Some("Shure SM7B".into()),
        capsule: Some(Capsule::Dynamic),
        address: Some(Address::End),
        connection: Some(Connection::Xlr),
        gain_needed_db: Some(60.0),
        interface_max_gain_db: Some(56.0),
        gain_at_max: Some(true),
        ..Default::default()
    };
    let r = audio::scan(&build(&Scenario { speech_level_dbfs: -36.0, hiss_dbfs: -62.0, ..d() }), &ctx, &[]);
    let f = r.findings.iter().find(|f| f.id == "mic.not_enough_gain").expect("gain finding");
    assert_eq!(f.suggest_product.as_deref(), Some("inline_booster"));
    // Gain is maxed, so the shortfall is made up digitally, not by the knob.
    assert_eq!(r.chain.hardware_gain_change_db, 0.0);
    assert!(r.chain.input_gain_db > 10.0);
}

#[test]
fn side_address_mic_spoken_into_from_top() {
    let ctx = MicContext { mic_name: Some("Blue Yeti".into()), capsule: Some(Capsule::Condenser), address: Some(Address::Side), ..Default::default() };
    let s = Scenario { tone: vec![(3500.0, -10.0, 0.8), (8000.0, -12.0, 0.8)], speech_level_dbfs: -34.0, ..d() };
    let found = ids(&s, &ctx);
    assert!(found.contains(&"mic.wrong_side".to_string()), "{found:?}");
    assert!(!found.contains(&"mic.dull".to_string()));
}

#[test]
fn declined_fix_becomes_workaround() {
    let s = Scenario { rt60_s: 0.9, reverb_wet_db: 0.0, speech_level_dbfs: -36.0, ..d() };
    let t = build(&s);
    let r = audio::scan(&t, &MicContext::default(), &[Constraint::CantMoveMicCloser]);
    let f = r.findings.iter().find(|f| f.id == "mic.too_far").unwrap();
    assert_eq!(f.severity, Severity::Info);
    assert!(f.fix.starts_with("Got it"));
    // No bass boost and gentler compression when the mic must stay far.
    assert!(r.chain.eq.iter().all(|b| !(b.freq_hz < 300.0 && b.gain_db > 0.0)));
    assert!(r.chain.compressor.ratio <= 3.0);
    assert!(r.chain.expander.ratio >= 4.0);
}

#[test]
fn eq_never_exceeds_limit() {
    for s in [
        Scenario { tone: vec![(300.0, 14.0, 1.0)], speech_level_dbfs: -32.0, ..d() },
        Scenario { tone: vec![(3500.0, 14.0, 1.0)], speech_level_dbfs: -32.0, ..d() },
        Scenario { tone: vec![(3500.0, -14.0, 0.8)], ..d() },
    ] {
        let c = compute_chain(&analyze(&build(&s)), &MicContext::default(), &[]);
        assert!(c.eq.len() <= 5);
        assert!(c.eq.iter().all(|b| b.gain_db.abs() <= 6.0), "{:?}", c.eq);
    }
}

#[test]
fn clean_voice_gets_a_light_touch() {
    let c = compute_chain(&analyze(&build(&d())), &MicContext::default(), &[]);
    assert!(c.eq.is_empty(), "{:?}", c.eq);
    assert!(c.hardware_gain_change_db.abs() <= 3.0);
    assert!((60.0..=100.0).contains(&c.highpass_hz));
    assert!(c.deesser.is_none());
}

/// The verify loop: apply the chain to a flawed recording, re-measure, and
/// check the problems the chain is responsible for are fixed.
#[test]
fn applying_the_chain_fixes_what_settings_can_fix() {
    let s = Scenario {
        speech_level_dbfs: -40.0,
        tone: vec![(300.0, 10.0, 1.0)],
        left_only: true,
        ..d()
    };
    let before_test = build(&s);
    let before = analyze(&before_test);
    let mut chain = compute_chain(&before, &MicContext::default(), &[]);
    audio::process::calibrate_makeup(&before_test, &mut chain);
    let after = analyze(&render_test(&before_test, &chain));
    assert!(after.dead_channel.is_none() && after.channels == 1);
    // Processed output targets: voice ≈ −16 LUFS with speech peaks below the limiter.
    assert!((-12.0..=-2.5).contains(&after.speech_peak_dbfs), "peak {}", after.speech_peak_dbfs);
    assert!(after.tone.mud_db < before.tone.mud_db - 2.0, "mud {} -> {}", before.tone.mud_db, after.tone.mud_db);
    // Loudness lands on target, unless the chain chose headroom over loudness
    // (the synthetic voice is very peaky) and says so in its notes.
    if (after.speech_lufs - chain.target_lufs).abs() >= 2.0 {
        assert!(chain.notes.iter().any(|n| n.contains("under the loudness target")), "lufs {} without note", after.speech_lufs);
        assert!(after.speech_lufs > chain.target_lufs - 6.0, "lufs {}", after.speech_lufs);
    }
    assert!(after.clipped_fraction == 0.0);
    assert!(after.loud_peak_dbfs.unwrap() <= -1.4);
    let after_ids: Vec<_> = audio::mic_findings(&after, &MicContext::default()).into_iter().map(|f| f.id).collect();
    for fixed in ["mic.one_channel_only", "mic.too_quiet", "mic.muddy"] {
        assert!(!after_ids.contains(&fixed.to_string()), "{fixed} still present: {after_ids:?}");
    }
}

#[test]
fn report_serializes_for_the_app() {
    let r = audio::scan(&build(&Scenario { hiss_dbfs: -55.0, ..d() }), &MicContext::default(), &[]);
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"area\":\"mic\""));
    let back: coach_core::audio::MicReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back.findings.len(), r.findings.len());
}

#[test]
fn deesser_reduces_sharp_s_sounds() {
    let t = build(&Scenario { sibilance_db: 4.0, ..d() });
    let r = audio::scan(&t, &MicContext::default(), &[]);
    let de = r.chain.deesser.clone().expect("de-esser for sharp S");
    let mut without = r.chain.clone();
    without.deesser = None;
    let with_m = analyze(&render_test(&t, &r.chain));
    let without_m = analyze(&render_test(&t, &without));
    assert!(with_m.sibilance_db.unwrap() < without_m.sibilance_db.unwrap() - 1.5, "{:?} vs {:?} ({de:?})", with_m.sibilance_db, without_m.sibilance_db);
}

fn thumps(s: Scenario) -> Option<coach_core::Finding> {
    audio::scan(&build(&s), &MicContext::default(), &[]).findings.into_iter().find(|f| f.id == "mic.desk_thumps")
}

/// Same recording as the user would get if they skipped the optional tap step.
fn thumps_without_tap(s: Scenario) -> Option<coach_core::Finding> {
    let mut t = build(&s);
    t.segments.retain(|g| g.kind != audio::SegmentKind::DeskTap);
    audio::scan(&t, &MicContext::default(), &[]).findings.into_iter().find(|f| f.id == "mic.desk_thumps")
}

fn coupled() -> Scenario {
    Scenario { tap_db: -4.0, tap_ring_s: 0.12, desk_coupling_db: Some(-14.0), ..d() }
}

#[test]
fn desk_vibration_is_found_when_the_arm_touches_the_desk() {
    let f = thumps(coupled()).expect("coupled mic must be flagged");
    assert_eq!(f.severity, Severity::Major);
    assert_eq!(f.suggest_product.as_deref(), Some("shock_mount"));
    // The symptom is explained, the physical fix comes first, gain is not the fix.
    assert!(f.why.contains("louder"), "{}", f.why);
    assert!(f.fix.starts_with("Fix the touch first"), "{}", f.fix);
    assert!(f.fix.contains("do not turn the gain down"), "{}", f.fix);
    assert!(f.fix.find("foam or rubber").unwrap() < f.fix.find("low-cut").unwrap());
    // A plain tap that shows as loud as the voice is enough on its own (the old rule).
    assert!(thumps(Scenario { tap_db: 0.0, ..d() }).is_some());
}

#[test]
fn desk_vibration_is_found_without_the_tap_step() {
    // User skipped the optional tap: typing thumps and bumps in the quiet carry it.
    assert!(thumps_without_tap(coupled()).is_some());
    assert!(thumps_without_tap(d()).is_none());
}

#[test]
fn desk_vibration_is_not_confused_with_other_low_frequency_things() {
    assert!(thumps(d()).is_none(), "clean");
    // Steady room rumble / HVAC, also loud.
    for fan in [-52.0, -45.0, -40.0] {
        assert!(thumps(Scenario { fan_dbfs: Some(fan), ..d() }).is_none(), "fan {fan}");
        assert!(thumps_without_tap(Scenario { fan_dbfs: Some(fan), ..d() }).is_none(), "fan {fan} no tap");
    }
    // Mains hum.
    assert!(thumps(Scenario { hum: Some((60.0, -55.0)), ..d() }).is_none(), "hum");
    // Plosives (breath pops) in the read.
    assert!(thumps(Scenario { plosive_db: 4.0, ..d() }).is_none(), "plosives");
    assert!(thumps_without_tap(Scenario { plosive_db: 4.0, ..d() }).is_none(), "plosives no tap");
    // Loud keyboard through the air is the keyboard finding, not desk vibration.
    assert!(thumps(Scenario { typing_db: -8.0, ..d() }).is_none(), "airborne typing");
    // A decoupled mic: small tap, nothing in the quiet.
    assert!(thumps(Scenario { tap_db: -24.0, ..d() }).is_none(), "one weak clue alone");
    // Coupling is still found through a loud fan.
    assert!(thumps(Scenario { fan_dbfs: Some(-45.0), ..coupled() }).is_some(), "coupled + fan");
}

#[test]
fn desk_vibration_replaces_keyboard_advice_and_never_blames_the_gain() {
    let r = audio::scan(&build(&coupled()), &MicContext::default(), &[]);
    let k = r.findings.iter().find(|f| f.id == "mic.keyboard");
    if let Some(k) = k {
        assert!(k.fix.starts_with("Fix the desk touch first"), "{}", k.fix);
    }
    assert!(r.chain.notes.iter().any(|n| n.contains("desk shaking") && n.contains("gain")), "{:?}", r.chain.notes);
    // No gain-down advice caused by the thumps: the chain's gain comes from the voice only.
    let calm = audio::scan(&build(&Scenario { tap_db: -35.0, ..d() }), &MicContext::default(), &[]);
    assert_eq!(r.chain.hardware_gain_change_db, calm.chain.hardware_gain_change_db);
}

#[test]
fn cant_fix_desk_vibration_falls_back_to_a_steeper_low_cut() {
    let t = build(&coupled());
    let before = audio::scan(&t, &MicContext::default(), &[]);
    let after = audio::scan(&t, &MicContext::default(), &[Constraint::CantFixDeskVibration]);
    let f = after.findings.iter().find(|f| f.id == "mic.desk_thumps").unwrap();
    assert_eq!(f.severity, Severity::Info);
    assert!(f.fix.starts_with("Got it"));
    assert!(after.chain.highpass_hz >= 120.0 && after.chain.highpass_slope_db_oct == 24, "{:?}", after.chain.highpass_hz);
    assert!(before.chain.highpass_hz < after.chain.highpass_hz);
}

#[test]
fn old_saved_metrics_without_vibration_still_load() {
    let m = analyze(&build(&d()));
    let mut v = serde_json::to_value(&m).unwrap();
    v.as_object_mut().unwrap().remove("vibration");
    let back: coach_core::audio::AudioMetrics = serde_json::from_value(v).unwrap();
    assert_eq!(back.vibration.thump_events, 0);
}