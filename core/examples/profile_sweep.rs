//! Evidence for the setup-profile pass bars (`coach_core::setup_profile`):
//! sweeps synthetic rooms, noise and levels and prints what the analyser
//! measures plus each profile's verdict.
//!
//! `cargo run --release --example profile_sweep`

use coach_core::SetupProfile;
use coach_core::audio::{analyze, synth::{build, Scenario}, targets::{check_targets_for, Status}, AudioMetrics};

/// One letter per profile (streaming / voiceover / call): M met, c close, X missed.
fn verdicts(m: &AudioMetrics, ids: &[&str]) -> String {
    SetupProfile::ALL
        .iter()
        .map(|p| {
            let t = check_targets_for(m, *p);
            ids.iter()
                .map(|id| match t.iter().find(|c| c.id == *id).map(|c| c.status) {
                    Some(Status::Met) => 'M',
                    Some(Status::Close) => 'c',
                    Some(Status::Missed) => 'X',
                    _ => '-',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let d = Scenario::default;

    println!("== Level: speech RMS -> measured speech peak / rms / LUFS (crest) ==");
    for lvl in [-34.0, -30.0, -26.0, -22.0] {
        let m = analyze(&build(&Scenario { speech_level_dbfs: lvl, ..d() }));
        println!("speech {lvl:>5.0}  peak {:>6.1}  rms {:>6.1}  lufs {:>6.1}  crest {:>4.1}", m.speech_peak_dbfs, m.speech_rms_dbfs, m.speech_lufs, m.speech_peak_dbfs - m.speech_rms_dbfs);
    }

    println!("\n== Room: rt60 x wet -> measured decay (s) ==");
    print!("rt60 \\ wet");
    let wets = [-18.0, -12.0, -6.0, -2.0];
    for w in wets {
        print!("  {w:>6.0}");
    }
    println!();
    for rt in [0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8, 1.0] {
        print!("{rt:>9.1}");
        for w in wets {
            let m = analyze(&build(&Scenario { rt60_s: rt, reverb_wet_db: w, ..d() }));
            print!("  {:>6}", m.reverb_decay_s.map(|v| format!("{v:.2}")).unwrap_or("-".into()));
        }
        println!();
    }

    println!("\n== Room verdicts at wet -12 (streaming / voiceover / call) ==");
    for rt in [0.0, 0.2, 0.3, 0.4, 0.5, 0.6, 0.8] {
        let m = analyze(&build(&Scenario { rt60_s: rt, ..d() }));
        println!("rt60 {rt:.1}  decay {:>5}  room S/V/C: {}", m.reverb_decay_s.map(|v| format!("{v:.2}")).unwrap_or("-".into()), verdicts(&m, &["room"]));
    }

    println!("\n== Noise: hiss / fan level -> floor, SNR (speech -26) ==");
    for h in [-82.0, -70.0, -65.0, -60.0, -55.0, -50.0, -45.0] {
        let m = analyze(&build(&Scenario { hiss_dbfs: h, ..d() }));
        println!("hiss {h:>5.0}  floor {:>6.1}  snr {:>5.1}   floor+snr S/V/C: {}", m.noise_floor_dbfs, m.snr_db, verdicts(&m, &["noise_floor", "snr"]));
    }
    for fan in [-70.0, -60.0, -52.0, -45.0] {
        let m = analyze(&build(&Scenario { fan_dbfs: Some(fan), ..d() }));
        println!("fan  {fan:>5.0}  floor {:>6.1}  snr {:>5.1}   floor+snr S/V/C: {}", m.noise_floor_dbfs, m.snr_db, verdicts(&m, &["noise_floor", "snr"]));
    }
}
