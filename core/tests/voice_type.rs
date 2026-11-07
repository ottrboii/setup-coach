//! Voice-type measurement (P1 of docs/voice-tuning-design.md) on synthetic voices.

use coach_core::audio::synth::{Scenario, build};
use coach_core::audio::voice::{self, Confidence, ConfidenceReason, PitchClass, PlosiveClass, SibClass, TiltClass};
use coach_core::audio::{self, MicContext, MicTest, SegmentKind, analyze};

fn voice_of(s: &Scenario) -> voice::VoiceType {
    analyze(&build(s)).voice
}

/// The read segment cut to `secs` (the rest of the buffer is left alone).
fn short_read(s: &Scenario, secs: f64) -> MicTest {
    let mut t = build(s);
    for seg in t.segments.iter_mut().filter(|g| g.kind == SegmentKind::Read) {
        seg.end_s = seg.start_s + secs;
    }
    t
}

#[test]
fn pitch_classes_follow_f0_over_seeds() {
    for (f0, class) in [(90.0, PitchClass::Deep), (130.0, PitchClass::LowMid), (170.0, PitchClass::HighMid), (230.0, PitchClass::High)] {
        for seed in 1..=5u64 {
            let v = voice_of(&Scenario { f0_hz: f0, seed, ..Default::default() });
            let med = v.f0_median_hz.unwrap_or_else(|| panic!("f0 {f0} seed {seed}: no pitch ({v:?})"));
            assert!((med / f0 - 1.0).abs() < 0.12, "f0 {f0} seed {seed}: median {med}");
            assert_eq!(v.pitch, class, "f0 {f0} seed {seed}: median {med}");
            let (p10, p90) = (v.f0_p10_hz.unwrap(), v.f0_p90_hz.unwrap());
            assert!(p10 <= med && med <= p90 && p10 < p90, "f0 {f0} seed {seed}: {p10} {med} {p90}");
            assert_ne!(v.confidence, Confidence::Insufficient, "f0 {f0} seed {seed}");
        }
    }
}

#[test]
fn pitch_spread_is_measured() {
    let flat = voice_of(&Scenario { f0_spread: 0.0, ..Default::default() });
    let wide = voice_of(&Scenario { f0_spread: 2.0, ..Default::default() });
    let range = |v: &voice::VoiceType| v.f0_p90_hz.unwrap() / v.f0_p10_hz.unwrap();
    assert!(range(&wide) > range(&flat) + 0.1, "flat {} wide {}", range(&flat), range(&wide));
}

#[test]
fn clean_voice_is_neutral_and_confident() {
    for seed in 1..=3u64 {
        let v = voice_of(&Scenario { seed, ..Default::default() });
        assert_eq!(v.confidence, Confidence::Good, "{v:?}");
        assert_eq!(v.confidence_reason, None);
        assert_eq!(v.tilt, TiltClass::Neutral, "seed {seed}: {v:?}");
        assert_eq!(v.sibilance, SibClass::Normal, "seed {seed}: {v:?}");
        assert_eq!(v.plosives, PlosiveClass::None, "seed {seed}: {v:?}");
    }
}

#[test]
fn dark_and_bright_voices_have_opposite_tilt() {
    let base = voice_of(&Scenario::default()).tilt_db_per_oct.unwrap();
    for seed in 1..=3u64 {
        let dark = voice_of(&Scenario { voice_tilt_db_oct: -3.0, seed, ..Default::default() });
        let bright = voice_of(&Scenario { voice_tilt_db_oct: 3.0, seed, ..Default::default() });
        assert_eq!(dark.tilt, TiltClass::Dark, "seed {seed}: {dark:?}");
        assert_eq!(bright.tilt, TiltClass::Bright, "seed {seed}: {bright:?}");
        // Both land near the injected slope (the synth overshoots a little: neighbouring filters add).
        let (d, b) = (dark.tilt_db_per_oct.unwrap(), bright.tilt_db_per_oct.unwrap());
        assert!(d < base - 2.0 && d > base - 6.0, "dark {d} base {base}");
        assert!(b > base + 2.0 && b < base + 6.5, "bright {b} base {base}");
    }
}

#[test]
fn sibilant_peak_is_recovered() {
    for seed in [7u64, 11] {
        for hz in [5000.0, 7000.0, 8500.0] {
            let v = voice_of(&Scenario { sib_peak_hz: Some(hz), sibilance_db: -10.0, seed, ..Default::default() });
            let peak = v.sib_peak_hz.unwrap_or_else(|| panic!("peak {hz} seed {seed}: none ({v:?})"));
            assert!((peak - hz).abs() <= 500.0, "peak {hz} seed {seed}: got {peak}");
        }
    }
}

#[test]
fn sibilant_vs_not() {
    let soft = voice_of(&Scenario { sibilance_db: -40.0, ..Default::default() });
    // Nothing reaches the gate: no peak is claimed (the tilt of this synthetic voice is
    // not asserted: with no "s" it has no energy above 4.5 kHz at all).
    assert_eq!(soft.sib_peak_hz, None, "{soft:?}");
    assert_eq!(soft.sibilance, SibClass::Soft);
    let strong = voice_of(&Scenario { sibilance_db: 2.0, sib_peak_hz: Some(7000.0), ..Default::default() });
    assert_eq!(strong.sibilance, SibClass::Strong, "{strong:?}");
    assert!(strong.sib_level_db.unwrap() > soft.sib_level_db.unwrap() + 20.0);
    assert!(strong.sib_peak_hz.is_some());
}

#[test]
fn plosive_rate() {
    let calm = voice_of(&Scenario::default());
    let popper = voice_of(&Scenario { plosive_db: 4.0, ..Default::default() });
    assert_eq!(calm.plosives, PlosiveClass::None);
    assert_eq!(popper.plosives, PlosiveClass::Many, "{popper:?}");
    assert!(popper.plosives_per_min.unwrap() > calm.plosives_per_min.unwrap());
}

#[test]
fn too_short_clips_are_insufficient_not_guessed() {
    for secs in [0.3, 1.0, 2.0] {
        let v = voice::measure(&short_read(&Scenario::default(), secs), None);
        assert_eq!(v.confidence, Confidence::Insufficient, "{secs} s: {v:?}");
        assert_eq!(v.confidence_reason, Some(ConfidenceReason::TooLittleSpeech));
        assert_eq!((v.f0_median_hz, v.tilt_db_per_oct, v.sib_peak_hz, v.plosives_per_min), (None, None, None, None), "{secs} s");
        assert_eq!((v.pitch, v.tilt, v.sibilance, v.plosives), (PitchClass::Unknown, TiltClass::Unknown, SibClass::Unknown, PlosiveClass::Unknown));
    }
    // Longer but still short: never "Good".
    let v = voice::measure(&short_read(&Scenario::default(), 4.0), None);
    assert_ne!(v.confidence, Confidence::Good, "{v:?}");
}

#[test]
fn silence_is_insufficient() {
    let v = voice_of(&Scenario { silent: true, ..Default::default() });
    assert_eq!(v.confidence, Confidence::Insufficient);
    assert_eq!(v.f0_median_hz, None);
}

#[test]
fn noisy_recordings_are_never_confident() {
    // Speech drowned in hiss and fan noise.
    let drowned = voice_of(&Scenario { hiss_dbfs: -35.0, fan_dbfs: Some(-40.0), ..Default::default() });
    assert_eq!(drowned.confidence, Confidence::Insufficient, "{drowned:?}");
    assert_eq!(drowned.confidence_reason, Some(ConfidenceReason::Noisy));
    assert_eq!(drowned.f0_median_hz, None);
    // Noisy but audible: anything we report is flagged, and the pitch is not wild.
    for hiss in [-36.0, -45.0] {
        let v = voice_of(&Scenario { hiss_dbfs: hiss, ..Default::default() });
        assert_ne!(v.confidence, Confidence::Good, "hiss {hiss}: {v:?}");
        assert_eq!(v.confidence_reason, Some(ConfidenceReason::Noisy), "hiss {hiss}");
        if let Some(f0) = v.f0_median_hz {
            assert!((f0 / 120.0 - 1.0).abs() < 0.2, "hiss {hiss}: f0 {f0}");
        }
    }
}

#[test]
fn bandlimited_recordings_get_no_tilt() {
    let v = voice_of(&Scenario { bandlimit_hz: Some(3400.0), ..Default::default() });
    assert_eq!(v.confidence, Confidence::Low, "{v:?}");
    assert_eq!(v.confidence_reason, Some(ConfidenceReason::BandLimited));
    assert_eq!((v.tilt_db_per_oct, v.tilt, v.sib_peak_hz), (None, TiltClass::Unknown, None));
    assert_eq!(v.pitch, PitchClass::LowMid, "pitch survives a phone-quality mic: {v:?}");
}

#[test]
fn mic_correction_is_removed_from_the_tilt() {
    // "This voice through a bright mic": +3 dB/oct-ish presence lift, published as points.
    let mic = vec![(250.0, -5.0), (500.0, -3.0), (1000.0, 0.0), (2000.0, 3.0), (4000.0, 6.0), (8000.0, 9.0)];
    let clean = build(&Scenario::default());
    let through = build(&Scenario { mic_response: Some(mic.clone()), ..Default::default() });
    let base = analyze(&clean).voice.tilt_db_per_oct.unwrap();
    let raw = voice::measure(&through, None).tilt_db_per_oct.unwrap();
    let corrected = voice::measure(&through, Some(&mic)).tilt_db_per_oct.unwrap();
    assert!(raw > base + 1.5, "mic should brighten: base {base} raw {raw}");
    assert!((corrected - base).abs() < (raw - base).abs() * 0.4, "base {base} raw {raw} corrected {corrected}");
    // The default (no correction) is what analyze stores.
    assert_eq!(voice::measure(&through, None), analyze(&through).voice);
}

#[test]
fn tilt_math_is_exact_on_a_straight_line() {
    // Deviation that is +2 dB per octave around 1 kHz, on the 22 bands.
    let dev: Vec<f64> = coach_core::audio::dsp::BAND_CENTERS.iter().map(|f| 2.0 * (f / 1000.0).log2()).collect();
    assert!((voice::tilt_db_per_oct(&dev, None).unwrap() - 2.0).abs() < 1e-9);
    // A correction of the same slope cancels it, whatever level it is published at.
    let corr: Vec<(f64, f64)> = [100.0f64, 1000.0, 10000.0].iter().map(|f| (*f, 7.0 + 2.0 * (*f / 1000.0).log2())).collect();
    assert!(voice::tilt_db_per_oct(&dev, Some(&corr)).unwrap().abs() < 1e-9);
    assert_eq!(voice::tilt_db_per_oct(&dev[..5], None), None);
}

#[test]
fn scan_carries_the_voice_and_old_reports_still_load() {
    let r = audio::scan(&build(&Scenario::default()), &MicContext::default(), &[]);
    assert_eq!(r.metrics.voice.confidence, Confidence::Good);
    // A saved report from before this field existed loads with the "insufficient" default.
    let mut json = serde_json::to_value(&r).unwrap();
    json["metrics"].as_object_mut().unwrap().remove("voice").expect("voice is serialised");
    let old: audio::MicReport = serde_json::from_value(json).unwrap();
    assert_eq!(old.metrics.voice.confidence, Confidence::Insufficient);
    assert_eq!(old.chain, r.chain);
}

#[test]
fn measuring_the_voice_leaves_the_chain_alone() {
    // P1 adds measurements only: a clean voice still needs no EQ and no de-esser.
    let r = audio::scan(&build(&Scenario::default()), &MicContext::default(), &[]);
    assert!(r.chain.eq.is_empty(), "{:?}", r.chain.eq);
    assert!(r.chain.deesser.is_none());
}
