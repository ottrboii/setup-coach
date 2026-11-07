//! Voice tuning composer (docs/voice-tuning-design.md P3): D = M + P + R,
//! net cap, verification. Synthetic voices only; thresholds are the design's
//! starting values. `cargo run --release --example voice_tune` prints the numbers.

use coach_core::audio::chain::{EqBand, EqKind};
use coach_core::audio::dsp::Rng;
use coach_core::audio::findings::MicContext;
use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::tune::{self, BandOrigin, Candidate, FallbackReason, TuneInput, Verdict, VoiceTune, CAP_FREQS};
use coach_core::profiles::{MicResponse, Proximity};
use coach_core::{SetupProfile, VoiceoverTarget};

fn d() -> Scenario {
    Scenario::default()
}

fn mic(points: &[(f64, f64)]) -> MicResponse {
    MicResponse {
        points_hz_db: points.iter().map(|(f, g)| [*f, *g]).collect(),
        reference: "dB re 1 kHz (synthetic)".into(),
        measured_at_cm: None,
        pattern_b: Some(0.5),
        proximity: Some(Proximity::Gradient),
        switch_curves: Default::default(),
        source: "synthetic".into(),
        digitised_precision_db: Some(1.0),
        verified: false,
    }
}

/// Bass roll-off and a broad +5 dB presence lift, like many vocal mics.
fn presence_mic() -> MicResponse {
    mic(&[
        (100.0, -6.0), (125.0, -4.5), (160.0, -3.0), (200.0, -2.0), (250.0, -1.0), (315.0, 0.0), (500.0, 0.0), (1000.0, 0.0),
        (1600.0, 0.5), (2000.0, 1.5), (2500.0, 3.0), (3150.0, 4.5), (4000.0, 5.0), (5000.0, 5.0), (6300.0, 3.5), (8000.0, 1.0),
        (10000.0, -2.0),
    ])
}

fn compose(s: &Scenario, p: SetupProfile, m: Option<&MicResponse>) -> VoiceTune {
    let test = build(s);
    let ctx = MicContext { profile: p, ..Default::default() };
    tune::compose(&TuneInput { test: &test, ctx: &ctx, constraints: &[], mic: m, switch: None })
}

/// Log sweep 80 Hz-16 kHz, 24 points per octave.
fn dense_sweep() -> impl Iterator<Item = f64> {
    (0..=(24.0 * (16000f64 / 80.0).log2()) as usize).map(|k| 80.0 * 2f64.powf(k as f64 / 24.0))
}

fn net_within_cap(t: &VoiceTune) {
    for (f, g) in CAP_FREQS.iter().zip(&t.curves.net_db) {
        assert!(*g >= -6.0 - 1e-9 && *g <= 3.0 + 1e-9, "net {g:+.2} dB at {f} Hz");
    }
    // Between the third-octave points too, on the bands actually proposed.
    let sr = d().sample_rate;
    for f in dense_sweep() {
        let g = tune::net_response(&t.proposed.eq, sr, f);
        assert!((-6.0..=3.0).contains(&g), "net {g:+.2} dB at {f:.0} Hz: {:?}", t.proposed.eq);
        let mic: Vec<EqBand> = t.proposed.eq.iter().zip(&t.origins).filter(|x| *x.1 == BandOrigin::Mic).map(|x| x.0.clone()).collect();
        let gm = tune::net_response(&mic, sr, f);
        assert!(gm.abs() <= tune::MIC_LIMIT_DB + 0.1, "mic sum {gm:+.2} dB at {f:.0} Hz: {mic:?}");
    }
}

// Overlapping mic bands can add past +-3 dB each; the sum is held to +-3 dB.
#[test]
fn mic_sum_is_limited() {
    let sr = 48000;
    let peak = |f: f64, g: f64| EqBand { kind: EqKind::Peak, freq_hz: f, gain_db: g, q: 0.7, reason: String::new() };
    let mut bands = vec![peak(3000.0, -3.0), peak(4500.0, -3.0), EqBand { kind: EqKind::HighShelf, freq_hz: 6000.0, gain_db: -2.0, q: 0.7, reason: String::new() }];
    let worst = CAP_FREQS.iter().map(|f| tune::net_response(&bands, sr, *f)).fold(0.0f64, f64::min);
    assert!(worst < -4.0, "setup overlaps: {worst}");
    assert!(tune::limit_mic_sum(&mut bands, sr));
    for f in CAP_FREQS {
        let g = tune::net_response(&bands, sr, f);
        assert!(g.abs() <= tune::MIC_LIMIT_DB, "{g} at {f}");
    }
    let worst = CAP_FREQS.iter().map(|f| tune::net_response(&bands, sr, *f)).fold(0.0f64, f64::min);
    assert!(worst < -2.8, "scaled to the limit, not past it: {worst}");
    // Already inside: untouched.
    let mut one = vec![peak(3000.0, -2.0)];
    assert!(!tune::limit_mic_sum(&mut one, sr));
    assert_eq!(one[0].gain_db, -2.0);
}

/// Whatever the verdict: applied only when verified, otherwise today's chain.
fn applied_only_when_verified(t: &VoiceTune) {
    if t.verdict.applied() {
        let v = t.verification.as_ref().unwrap();
        assert!(v.v2_no_harm && v.v3_level && v.v4_cap && v.improved, "{v:?}");
        assert_eq!(t.chain.eq, t.proposed.eq.iter().filter(|b| !v.dropped.contains(b)).cloned().collect::<Vec<_>>());
    } else {
        assert_eq!(t.chain, t.base);
    }
}

const AIR: usize = 6;

// Section 9.2 do no harm: a clean average voice gets nothing.
#[test]
fn clean_voice_nothing_to_change() {
    let t = compose(&d(), SetupProfile::Streaming, None);
    assert_eq!(t.fallback, None);
    assert!(t.proposed.eq.is_empty() && t.proposed.deesser.is_none(), "{:?}", t.proposed.eq);
    assert_eq!(t.verdict, Verdict::NothingToChange);
    assert_eq!(t.chain, t.base);
    assert_eq!(t.curves.mic_weight, 0.0);
}

// Dark voice: brighter after, but still darker than average (character kept, 9.4).
#[test]
fn dark_voice_brightened_gently_and_stays_dark() {
    let t = compose(&Scenario { voice_tilt_db_oct: -3.0, ..d() }, SetupProfile::Streaming, None);
    assert_eq!(t.fallback, None);
    assert!(t.proposed.eq.iter().any(|b| b.freq_hz >= 2000.0 && b.gain_db > 0.0), "{:?}", t.proposed.eq);
    assert!(t.origins.iter().all(|o| *o == BandOrigin::Voice));
    net_within_cap(&t);
    applied_only_when_verified(&t);
    // The scan already brightens this voice the same way: no news (see
    // `no_mic_chart_tune_equal_to_current_is_nothing_to_change`). The character
    // check moves to the mic-chart case below.
    assert_eq!(t.verdict, Verdict::NothingToChange);
    assert!(t.chain.eq.iter().any(|b| b.freq_hz >= 2000.0 && b.gain_db > 0.0), "today's chain keeps the lift");
}

// The synth's `voice_tilt_db_oct` is built from narrow (Q 4.3) third-octave
// peaks; on a bright tilt that staircase ripples like a comb and the analyser
// reports `mic.reflection`. Then the voice rules must not run: mic-only bands.
#[test]
fn bright_tilt_with_reflection_is_mic_only() {
    let m = presence_mic();
    for tilt in [2.0, 3.0] {
        let s = Scenario { voice_tilt_db_oct: tilt, ..d() };
        let t = compose(&s, SetupProfile::Streaming, None);
        assert_eq!(t.fallback, Some(FallbackReason::Reflection), "{tilt}");
        assert!(t.proposed.eq.is_empty(), "{tilt}: {:?}", t.proposed.eq);
        assert!(t.notes.iter().any(|n| n.contains("only corrected the mic")));
        let t = compose(&s, SetupProfile::Streaming, Some(&m));
        assert_eq!(t.fallback, Some(FallbackReason::Reflection));
        assert!(!t.origins.is_empty() && t.origins.iter().all(|o| *o == BandOrigin::Mic), "{tilt}: {:?}", t.origins);
        net_within_cap(&t);
        applied_only_when_verified(&t);
    }
}

// Bright voice the voice rules do judge (broad brightness, no comb): never
// boosted up top, a cut is proposed, and what is applied keeps it brighter than average.
#[test]
fn bright_voice_never_boosted_up_top() {
    let s = Scenario { tone: vec![(4000.0, 4.0, 0.7), (8000.0, 6.0, 0.7)], ..d() };
    let t = compose(&s, SetupProfile::Streaming, None);
    assert_eq!(t.fallback, None, "voice rules must run");
    assert!(t.voice.tilt_db_per_oct.unwrap() > 1.0, "{:?}", t.voice.tilt_db_per_oct);
    assert!(!t.proposed.eq.iter().any(|b| b.freq_hz >= 2000.0 && b.gain_db > 0.0), "{:?}", t.proposed.eq);
    assert!(t.proposed.eq.iter().zip(&t.origins).any(|(b, o)| *o == BandOrigin::Voice && b.freq_hz >= 2000.0 && b.gain_db < 0.0), "{:?}", t.proposed.eq);
    net_within_cap(&t);
    applied_only_when_verified(&t);
    // "Before" is today's chain. With no mic chart the cap-limited tune (-4.8 dB
    // at 8 kHz) is gentler than what the scan already applies (-5.7 dB), so it
    // does not beat it: nothing is applied, today's chain stays.
    assert_eq!(t.verdict, Verdict::NotImproved, "{:?}", t.verification);
    let v = t.verification.as_ref().unwrap();
    assert!(!v.improved && v.after.regions_db[AIR] > v.before.regions_db[AIR], "gentler than today, not better");
    assert!(v.before.regions_db[AIR] > 0.0, "character kept");
}

// The tune is judged against the chain the user has now, EQ and de-esser included.
// No mic chart: R equals the scan's own EQ, so there is nothing new to offer.
#[test]
fn no_mic_chart_tune_equal_to_current_is_nothing_to_change() {
    for p in [SetupProfile::Streaming, SetupProfile::Call] {
        let t = compose(&Scenario { voice_tilt_db_oct: -3.0, ..d() }, p, None);
        assert!(!t.base.eq.is_empty(), "the scan already corrects this voice");
        assert_eq!(t.proposed.eq, t.base.eq, "R equals today's EQ");
        assert_eq!(t.verdict, Verdict::NothingToChange);
        assert!(t.verification.is_none() && !t.verdict.applied());
        assert_eq!(t.chain, t.base);
    }
}

// A mic chart that adds bands: judged against today's chain, not a bare one.
#[test]
fn mic_chart_tune_is_verified_against_current_chain() {
    let m = presence_mic();
    let s = Scenario { voice_tilt_db_oct: -3.0, mic_response: Some(m.points_hz_db.iter().map(|p| (p[0], p[1])).collect()), ..d() };
    let t = compose(&s, SetupProfile::Streaming, Some(&m));
    assert!(t.origins.contains(&BandOrigin::Mic));
    assert!(!t.base.eq.is_empty());
    applied_only_when_verified(&t);
    assert!(t.verdict.applied(), "{:?}", t.verification);
    let v = t.verification.as_ref().unwrap();
    assert!(v.improved && v.after.tone_rms_db < v.before.tone_rms_db - 0.05, "{} -> {}", v.before.tone_rms_db, v.after.tone_rms_db);
    // "Before" is today's chain: it renders like `base`, not like a bare chain.
    let test = build(&s);
    let ctx = MicContext::default();
    let bare = { let mut c = t.base.clone(); c.eq.clear(); c.deesser = None; coach_core::audio::process::calibrate_makeup(&test, &mut c); c };
    let b = tune::check_render(&test, &ctx, &t.base, &SetupProfile::Streaming.tuning());
    let r = tune::check_render(&test, &ctx, &bare, &SetupProfile::Streaming.tuning());
    assert_eq!(v.before, b);
    assert_ne!(v.before, r);
}

// A tune that is not better than what the user has now is never applied, however
// the chart reads (flat voice through a chart that says the mic is coloured).
#[test]
fn tune_never_worse_than_current_chain() {
    let m = presence_mic();
    for s in [d(), Scenario { voice_tilt_db_oct: -3.0, ..d() }, Scenario { sibilance_db: -2.0, sib_peak_hz: Some(7000.0), ..d() }] {
        let t = compose(&s, SetupProfile::Streaming, Some(&m));
        applied_only_when_verified(&t);
        if let Some(v) = &t.verification {
            if t.verdict.applied() {
                assert!(v.after.tone_rms_db < v.before.tone_rms_db || v.after.sibilance_db < v.before.sibilance_db, "{v:?}");
            } else {
                assert_eq!(t.chain, t.base);
            }
        }
    }
}

// Sibilant voice: de-esser centred on the measured "s" peak (clamped 4.5-9 kHz),
// and an applied tune never raises the "s" by more than 1 dB (9.6).
#[test]
fn sibilant_voice_deesser_follows_the_s_peak() {
    let t = compose(&Scenario { sibilance_db: -2.0, sib_peak_hz: Some(7000.0), ..d() }, SetupProfile::Streaming, None);
    assert_eq!(t.fallback, None);
    let de = t.proposed.deesser.as_ref().expect("strong s gets a de-esser");
    assert!((tune::DEESS_MIN_HZ..=tune::DEESS_MAX_HZ).contains(&de.freq_hz));
    assert!((de.freq_hz - 7000.0).abs() <= 1000.0, "centre {}", de.freq_hz);
    // Today's chain uses a fixed 6.5 kHz; the tune uses the measured peak.
    assert_eq!(t.base.deesser.as_ref().map(|d| d.freq_hz), Some(6500.0));
    net_within_cap(&t);
    applied_only_when_verified(&t);
    // The verified candidate lowers the "s" (checked whatever the verdict: on
    // this synthetic voice the tune is held back by V3, the level).
    let v = t.verification.as_ref().expect("a strong s is verified");
    let (before, after) = (v.before.sibilance_db.unwrap(), v.after.sibilance_db.unwrap());
    // Today's 6.5 kHz de-esser is part of "before" now, so the gain is smaller.
    assert!(after < before - tune::DEESS_MIN_GAIN_DB, "s {before:.1} -> {after:.1}");
    assert!(v.v2_no_harm, "{v:?}");
}

// A de-esser on its own counts as better only when the "s" measurably went down.
#[test]
fn deesser_only_needs_the_s_to_drop() {
    use coach_core::audio::chain::DeEsser;
    let run = |s: &Scenario, de: DeEsser| {
        let test = build(s);
        let ctx = MicContext::default();
        let m = coach_core::audio::analyze(&test);
        let raw = coach_core::audio::compute_chain(&m, &ctx, &[]);
        // The user's current chain has no EQ and no de-esser here, so the
        // candidate is judged against a bare one (with today's 6.5 kHz de-esser
        // already in the chain, the 5 kHz candidate leaves the "s" higher than today's: -2.1 -> -1.2 dB).
        let mut base = raw.clone();
        base.eq.clear();
        base.deesser = None;
        coach_core::audio::process::calibrate_makeup(&test, &mut base);
        let cand = Candidate { bands: vec![], deesser: Some(de) };
        let (verdict, ver, chain) = tune::verify_and_decide(&test, &ctx, &raw, &base, cand, &SetupProfile::Streaming.tuning());
        (verdict, ver.unwrap(), chain, base)
    };
    let de = |f: f64, t: f64| DeEsser { freq_hz: f, threshold_db: t, max_reduction_db: 8.0 };
    let sibilant = Scenario { sibilance_db: -2.0, sib_peak_hz: Some(7000.0), ..d() };
    // This one lowers the "s" (synthetic: 1.4 -> -1.2 dB): it counts as better.
    let (_, v, _, _) = run(&sibilant, de(5000.0, -12.0));
    let (b, a) = (v.before.sibilance_db.unwrap(), v.after.sibilance_db.unwrap());
    assert!(a < b - tune::DEESS_MIN_GAIN_DB && v.improved, "s {b:.1} -> {a:.1}");
    // This one raises it (1.4 -> 2.7 dB) although its high cut lowers the tone
    // RMS: not better, so not applied.
    let (verdict, v, _, _) = run(&sibilant, de(7000.0, -8.0));
    let (b, a) = (v.before.sibilance_db.unwrap(), v.after.sibilance_db.unwrap());
    assert!(a >= b && v.after.tone_rms_db < v.before.tone_rms_db, "s {b:.1} -> {a:.1}");
    assert!(!v.improved && !verdict.applied(), "{verdict:?}");
    // Soft "s": nothing for the de-esser to take down, so not applied.
    let (verdict, v, chain, base) = run(&Scenario { sibilance_db: -25.0, ..d() }, de(7000.0, -8.0));
    let (b, a) = (v.before.sibilance_db, v.after.sibilance_db);
    assert!(!v.improved, "s {b:?} -> {a:?}");
    assert_eq!(verdict, Verdict::NotImproved);
    assert_eq!(chain, base);
}

// Low confidence (noisy read): no voice-based moves, mic correction only, reason reported.
#[test]
fn low_confidence_falls_back_to_mic_only() {
    let s = Scenario { hiss_dbfs: -45.0, voice_tilt_db_oct: -3.0, ..d() };
    let t = compose(&s, SetupProfile::Streaming, None);
    assert!(matches!(t.fallback, Some(FallbackReason::VoiceConfidence { .. })), "{:?}", t.fallback);
    assert!(t.proposed.eq.is_empty());
    assert_eq!(t.verdict, Verdict::NothingToChange);
    assert_eq!(t.chain, t.base);
    assert!(t.notes.iter().any(|n| n.contains("only corrected the mic")));
    let m = presence_mic();
    let t = compose(&s, SetupProfile::Streaming, Some(&m));
    assert!(!t.proposed.eq.is_empty());
    assert!(t.origins.iter().all(|o| *o == BandOrigin::Mic), "{:?}", t.origins);
    assert!(t.hpf.proposed_hz.is_none(), "no voice-based HPF proposal on a low-confidence read");
    applied_only_when_verified(&t);
}

// M: half weight, clamped to +-3 dB, broad features only, nothing without a chart.
#[test]
fn mic_correction_is_half_weight_clamped_and_broad_only() {
    let extreme = mic(&[(100.0, -15.0), (200.0, -10.0), (400.0, 0.0), (1000.0, 0.0), (2500.0, 10.0), (4000.0, 12.0), (6300.0, 10.0), (10000.0, 0.0)]);
    let m = tune::mic_correction(&extreme.points_hz_db, tune::MIC_WEIGHT_UNVERIFIED);
    assert!(m.iter().all(|g| g.abs() <= 3.0 + 1e-9), "{m:?}");
    assert!(m.iter().any(|g| *g <= -2.9) && m.iter().any(|g| *g >= 2.9), "big features use the full +-3 dB");
    // Half weight: a 4 dB broad lift becomes about -2 dB.
    let lift = mic(&[(100.0, 0.0), (1000.0, 0.0), (2000.0, 4.0), (5000.0, 4.0), (8000.0, 0.0), (10000.0, 0.0)]);
    let m = tune::mic_correction(&lift.points_hz_db, tune::MIC_WEIGHT_UNVERIFIED);
    let at3k = m[16];
    assert!((at3k + 2.0).abs() < 0.3, "{at3k}");
    // A narrow wiggle (one third-octave) is not corrected.
    let narrow = mic(&[(100.0, 0.0), (2500.0, 0.0), (3150.0, 6.0), (4000.0, 0.0), (10000.0, 0.0)]);
    assert!(tune::mic_correction(&narrow.points_hz_db, 0.5).iter().all(|g| *g == 0.0));
    // Through compose: every evaluated mic band within +-3 dB, weight 0.5 (unverified).
    let t = compose(&d(), SetupProfile::Streaming, Some(&extreme));
    assert_eq!(t.curves.mic_weight, 0.5);
    assert!(t.curves.mic_db.iter().all(|g| g.abs() <= 3.0 + 1e-9));
    for (b, o) in t.proposed.eq.iter().zip(&t.origins) {
        if *o == BandOrigin::Mic {
            assert!(b.gain_db.abs() <= 3.0, "{b:?}");
        }
    }
    net_within_cap(&t);
    // Proximity: a cardioid coached at 3-10 cm swings > 4 dB at 100 Hz.
    let swing = tune::proximity_swing(&extreme, Some((3.0, 10.0))).unwrap();
    assert!(swing > tune::PROXIMITY_SWING_DB, "{swing}");
}

// Section 9.5: random curves never leave -6 / +3 dB on the evaluated response;
// the mic share is never cut when it fits on its own.
#[test]
fn cap_holds_on_the_evaluated_response() {
    let mut rng = Rng::new(42);
    let sr = 48000;
    for _ in 0..200 {
        let n = 2 + (rng.uniform() * 4.0) as usize;
        let mut bands: Vec<(EqBand, BandOrigin)> = (0..n)
            .map(|k| {
                let origin = if k == 0 { BandOrigin::Mic } else { BandOrigin::Voice };
                let lim = if origin == BandOrigin::Mic { 3.0 } else { 6.0 };
                let kind = [EqKind::Peak, EqKind::Peak, EqKind::LowShelf, EqKind::HighShelf][(rng.uniform() * 4.0) as usize % 4];
                let band = EqBand { kind, freq_hz: 2f64.powf(rng.range(100f64.log2(), 10000f64.log2())), gain_db: rng.range(-lim, lim), q: rng.range(0.7, 1.4), reason: String::new() };
                (band, origin)
            })
            .collect();
        let mic_before = bands[0].0.gain_db;
        tune::cap_bands(&mut bands, sr);
        let eq: Vec<EqBand> = bands.iter().map(|x| x.0.clone()).collect();
        for f in CAP_FREQS {
            let g = tune::net_response(&eq, sr, f);
            assert!((-6.0..=3.0).contains(&g), "{g} at {f}: {eq:?}");
        }
        let mic_alone = tune::net_response(&[bands[0].0.clone()], sr, 1000.0);
        let mic_fits = CAP_FREQS.iter().all(|f| (-5.9..=2.9).contains(&tune::net_response(&[EqBand { gain_db: mic_before, ..bands[0].0.clone() }], sr, *f)));
        if mic_fits {
            assert_eq!(bands[0].0.gain_db, mic_before, "mic share kept ({mic_alone})");
        }
    }
}

// Verify loop: a candidate that does harm is not applied; today's chain stays.
#[test]
fn verify_fails_not_applied() {
    let test = build(&d());
    let ctx = MicContext::default();
    let m = coach_core::audio::analyze(&test);
    let raw = coach_core::audio::compute_chain(&m, &ctx, &[]);
    let mut base = raw.clone();
    coach_core::audio::process::calibrate_makeup(&test, &mut base);
    let cut = |f: f64| (EqBand { kind: EqKind::Peak, freq_hz: f, gain_db: -6.0, q: 1.4, reason: "harm".into() }, BandOrigin::Voice);
    let cand = Candidate { bands: vec![cut(500.0), cut(1000.0), cut(3150.0)], deesser: None };
    let (verdict, ver, chain) = tune::verify_and_decide(&test, &ctx, &raw, &base, cand, &SetupProfile::Streaming.tuning());
    assert_eq!(verdict, Verdict::NotImproved);
    assert_eq!(chain, base);
    let ver = ver.unwrap();
    assert!(!ver.v2_no_harm);
    assert_eq!(ver.dropped.len(), 2, "two do-no-harm rounds, then give up");
}

// Calls never spend EQ above 7 kHz, in today's chain or the tune.
#[test]
fn call_has_no_boost_above_7k() {
    let s = Scenario { voice_tilt_db_oct: -3.0, ..d() };
    let t = compose(&s, SetupProfile::Call, None);
    for c in [&t.base, &t.proposed, &t.chain] {
        assert!(!c.eq.iter().any(|b| b.gain_db > 0.0 && (b.freq_hz > 7000.0 || b.kind == EqKind::HighShelf && b.freq_hz > 5000.0)), "{:?}", c.eq);
    }
    // Streaming does use the air shelf on the same voice.
    let st = compose(&s, SetupProfile::Streaming, None);
    assert!(st.proposed.eq.iter().any(|b| b.gain_db > 0.0 && b.freq_hz >= 7000.0));
}

/// Loudness (LUFS and plain RMS) and sample peak of the read rendered through `chain`.
fn read_levels(test: &coach_core::audio::MicTest, chain: &coach_core::audio::chain::ProcessingChain) -> (f64, f64, f64) {
    use coach_core::audio::{dsp, process, AudioBuffer, SegmentKind};
    let seg = test.segments.iter().find(|s| s.kind == SegmentKind::Read).unwrap();
    let r = test.range(seg);
    let read = AudioBuffer { sample_rate: test.audio.sample_rate, channels: test.audio.channels.iter().map(|c| c[r.clone()].to_vec()).collect() };
    let out = process::render(&read, chain);
    let x = &out.channels[0];
    (dsp::integrated_lufs(x, out.sample_rate), dsp::level_db(x), dsp::db_amp(dsp::peak_abs(x)))
}

fn compose_vo(s: &Scenario, vo: VoiceoverTarget) -> (coach_core::audio::MicTest, VoiceTune) {
    let test = build(s);
    let ctx = MicContext { profile: SetupProfile::Voiceover, voiceover_target: vo, ..Default::default() };
    let t = tune::compose(&TuneInput { test: &test, ctx: &ctx, constraints: &[], mic: None, switch: None });
    (test, t)
}

// Voiceover / YouTube end to end: the chain lands on -14 LUFS (+-1), the
// compressor stays at most 3:1 and nothing passes the -1.5 dBFS ceiling.
#[test]
fn voiceover_youtube_reaches_minus_14() {
    for s in [d(), Scenario { voice_tilt_db_oct: -3.0, ..d() }] {
        let (test, t) = compose_vo(&s, VoiceoverTarget::Youtube);
        for c in [&t.base, &t.chain] {
            let (lufs, _, peak) = read_levels(&test, c);
            eprintln!("YouTube: {lufs:.1} LUFS, peak {peak:.2} dBFS, {:?}", c.compressor);
            assert!((-15.0..=-13.0).contains(&lufs), "YouTube read at {lufs:.1} LUFS: {:?}", c.compressor);
            assert!(peak <= -1.5 + 1e-6, "peak {peak:.2} dBFS");
            assert!(c.compressor.ratio <= 3.0, "{:?}", c.compressor);
            assert_eq!(c.limiter.ceiling_db, -1.5);
        }
    }
}

// Voiceover / Audiobook (ACX) end to end: RMS -23...-18 dB, peaks <= -3 dBFS.
#[test]
fn voiceover_acx_meets_rms_and_peaks() {
    for s in [d(), Scenario { voice_tilt_db_oct: -3.0, ..d() }] {
        let (test, t) = compose_vo(&s, VoiceoverTarget::Audiobook);
        for c in [&t.base, &t.chain] {
            let (_, rms, peak) = read_levels(&test, c);
            eprintln!("ACX: {rms:.1} dB RMS, peak {peak:.2} dBFS, {:?}", c.compressor);
            assert!((-23.0..=-18.0).contains(&rms), "ACX RMS {rms:.1} dB");
            assert!(peak <= -3.0, "ACX peak {peak:.2} dBFS");
            assert!(c.compressor.ratio <= 3.0, "{:?}", c.compressor);
        }
    }
}

// HPF rule f0_p10 / 1.5 is reported (not switched): it keeps the fundamental within 0.8 dB.
#[test]
fn hpf_proposal_is_reported_not_switched() {
    let t = compose(&Scenario { f0_hz: 95.0, ..d() }, SetupProfile::Streaming, None);
    let h = t.hpf;
    let p10 = h.f0_p10_hz.unwrap();
    assert!((h.proposed_hz.unwrap() - p10 / 1.5).abs() <= 0.5);
    assert!(h.proposed_loss_db.unwrap() >= -0.8, "{h:?}");
    assert_eq!(h.current_hz, t.base.highpass_hz);
    assert_eq!(t.proposed.highpass_hz, t.base.highpass_hz);
}
