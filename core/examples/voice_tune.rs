//! Print the voice-tuning composer's work on synthetic voices:
//! `cargo run --release --example voice_tune [scenario]`.
//! Scenarios: clean, bright, dark, sibilant, short, mic, mic_dark, boomy, harsh, call_bright, all (default).

use coach_core::audio::findings::MicContext;
use coach_core::audio::synth::{build, Scenario};
use coach_core::audio::tune::{compose, TuneInput, REGIONS};
use coach_core::profiles::{MicResponse, Proximity};
use coach_core::SetupProfile;

fn presence_mic() -> MicResponse {
    let pts: Vec<[f64; 2]> = [
        (100.0, -6.0), (125.0, -4.5), (160.0, -3.0), (200.0, -2.0), (250.0, -1.0), (315.0, 0.0), (400.0, 0.0), (500.0, 0.0),
        (630.0, 0.0), (800.0, 0.0), (1000.0, 0.0), (1250.0, 0.0), (1600.0, 0.5), (2000.0, 1.5), (2500.0, 3.0), (3150.0, 4.5),
        (4000.0, 5.0), (5000.0, 5.0), (6300.0, 3.5), (8000.0, 1.0), (10000.0, -2.0),
    ]
    .iter()
    .map(|(f, d)| [*f, *d])
    .collect();
    MicResponse {
        points_hz_db: pts,
        reference: "dB re 1 kHz (synthetic)".into(),
        measured_at_cm: None,
        pattern_b: Some(0.5),
        proximity: Some(Proximity::Gradient),
        switch_curves: Default::default(),
        source: "synthetic test curve".into(),
        digitised_precision_db: Some(1.0),
        verified: false,
    }
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
   let d = Scenario::default;
    let mic = presence_mic();
    let mic_pts: Vec<(f64, f64)> = mic.points_hz_db.iter().map(|p| (p[0], p[1])).collect();
    let cases: Vec<(&str, Scenario, SetupProfile, bool)> = vec![
        ("clean", d(), SetupProfile::Streaming, false),
        ("bright", Scenario { voice_tilt_db_oct: 3.0, ..d() }, SetupProfile::Streaming, false),
        ("dark", Scenario { voice_tilt_db_oct: -3.0, ..d() }, SetupProfile::Streaming, false),
        ("sibilant", Scenario { sibilance_db: -2.0, sib_peak_hz: Some(7000.0), ..d() }, SetupProfile::Streaming, false),
        ("short", Scenario { f0_spread: 1.0, hiss_dbfs: -45.0, ..d() }, SetupProfile::Streaming, false),
        ("mic", Scenario { mic_response: Some(mic_pts.clone()), ..d() }, SetupProfile::Streaming, true),
        ("mic_dark", Scenario { voice_tilt_db_oct: -3.0, mic_response: Some(mic_pts.clone()), ..d() }, SetupProfile::Streaming, true),
        ("boomy", Scenario { tone: vec![(120.0, 9.0, 1.0)], ..d() }, SetupProfile::Streaming, false),
        ("harsh", Scenario { tone: vec![(3500.0, 8.0, 1.2)], ..d() }, SetupProfile::Streaming, false),
        ("call_bright", Scenario { voice_tilt_db_oct: 3.0, ..d() }, SetupProfile::Call, false),
        ("vo_dark", Scenario { voice_tilt_db_oct: -3.0, ..d() }, SetupProfile::Voiceover, false),
    ];
    for (name, s, p, with_mic) in cases {
        if which != "all" && which != name {
            continue;
        }
        let test = build(&s);
        let ctx = MicContext { profile: p, ..Default::default() };
        let t = compose(&TuneInput { test: &test, ctx: &ctx, constraints: &[], mic: with_mic.then_some(&mic), switch: None });
        println!("== {name} ({p:?}) verdict {:?} fallback {:?}", t.verdict, t.fallback);
        println!(
            "   voice: conf {:?} f0 {:?} p10 {:?} tilt {:?} sib {:?} @ {:?}",
            t.voice.confidence, t.voice.f0_median_hz.map(|x| x.round()), t.voice.f0_p10_hz.map(|x| x.round()), t.voice.tilt_db_per_oct.map(|x| (x * 10.0).round() / 10.0),
            t.voice.sib_level_db.map(|x| x.round()), t.voice.sib_peak_hz.map(|x| x.round())
        );
        for (b, o) in t.proposed.eq.iter().zip(&t.origins) {
            println!("   band {o:?} {:?} {:.0} Hz {:+.1} dB q {:.2} ({})", b.kind, b.freq_hz, b.gain_db, b.q, b.reason);
        }
        println!("   de-esser {:?}", t.proposed.deesser.as_ref().map(|d| (d.freq_hz, d.max_reduction_db)));
        let mx = t.curves.net_db.iter().cloned().fold(f64::MIN, f64::max);
        let mn = t.curves.net_db.iter().cloned().fold(f64::MAX, f64::min);
        println!("   net {mn:+.1}..{mx:+.1} dB, fit error {:.2} dB, swing {:?}", t.curves.fit_error_db, t.curves.proximity_swing_db.map(|x| x.round()));
        println!("   hpf now {} Hz (loss {:?}) proposed {:?} (loss {:?})", t.hpf.current_hz, t.hpf.current_loss_db.map(|x| (x * 10.0).round() / 10.0), t.hpf.proposed_hz, t.hpf.proposed_loss_db.map(|x| (x * 10.0).round() / 10.0));
        if let Some(v) = &t.verification {
            let fmt = |r: &[f64]| r.iter().zip(REGIONS).map(|(x, (n, _, _))| format!("{n} {x:+.1}")).collect::<Vec<_>>().join(", ");
            println!("   before: {} | rms {:.2} sib {:?} lvl {:.1} pk {:.1} {:?}", fmt(&v.before.regions_db), v.before.tone_rms_db, v.before.sibilance_db.map(|x| x.round()), v.before.level, v.before.peak_dbfs, v.before.tonal_findings);
            println!("   after:  {} | rms {:.2} sib {:?} lvl {:.1} pk {:.1} {:?}", fmt(&v.after.regions_db), v.after.tone_rms_db, v.after.sibilance_db.map(|x| x.round()), v.after.level, v.after.peak_dbfs, v.after.tonal_findings);
            println!("   V1 {} V2 {} V3 {} V4 {} improved {} dropped {}", v.v1_tone, v.v2_no_harm, v.v3_level, v.v4_cap, v.improved, v.dropped.len());
        }
        for n in &t.notes {
            println!("   note: {n}");
        }
    }
}
