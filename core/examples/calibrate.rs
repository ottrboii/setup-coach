use coach_core::audio::{self, synth::{build, Scenario}, MicContext};

fn show(name: &str, s: Scenario) {
    let t = build(&s);
    let r = audio::scan(&t, &MicContext::default(), &[]);
    let m = &r.metrics;
    println!("== {name}: score {} | floor {:.1} rms {:.1} pk {:.1} quiet {:.1} lufs {:.1} snr {:.1} loud {:?} clip {:.5} f0 {:?} bw {} rev {:?} refl {:?} plos {} {:?} sib {:?} typ {:?} tap {:?} drop {} crk {} mv {:?}/{:?} lvr {:?} pause {:?} noise {:?}",
        r.score.value, m.noise_floor_dbfs, m.speech_rms_dbfs, m.speech_peak_dbfs, m.quiet_speech_dbfs, m.speech_lufs, m.snr_db, m.loud_peak_dbfs.map(|v| (v*10.0).round()/10.0), m.clipped_fraction, m.f0_hz.map(|v| v.round()), m.upper_bandwidth_hz, m.reverb_decay_s.map(|v| (v*100.0).round()/100.0), m.reflection.map(|r| ((r.delay_ms*100.0).round()/100.0, r.ripple_db.round())), m.plosive_events, m.plosive_worst_db.map(|v| v.round()), m.sibilance_db.map(|v| v.round()), m.typing_rel_db.map(|v| v.round()), m.tap_rel_db.map(|v| v.round()), m.dropouts, m.crackles, m.moving_drop_db.map(|v| v.round()), m.moving_spread_db.map(|v| v.round()), m.loud_vs_read_db.map(|v| v.round()), m.pause_floor_rise_db.map(|v| v.round()), m.noise_character);
    println!("   tone {:?}", m.tone);
    println!("   dev {:?}", m.band_deviation_db.iter().map(|d| d.round() as i32).collect::<Vec<_>>());
    for f in &r.findings { println!("   - {:?} {} — {}", f.severity, f.id, f.title); }
}

fn main() {
    let d = Scenario::default;
    let which: Vec<String> = std::env::args().skip(1).collect();
    let all = which.is_empty();
    let want = |n: &str| all || which.iter().any(|w| w == n);
    if want("clean") { show("clean", d()); }
    if want("female") { show("female", Scenario { f0_hz: 210.0, ..d() }); }
    if want("quiet") { show("quiet", Scenario { speech_level_dbfs: -42.0, ..d() }); }
    if want("hot") { show("hot", Scenario { speech_level_dbfs: -10.0, ..d() }); }
    if want("left") { show("left", Scenario { left_only: true, ..d() }); }
    if want("hiss") { show("hiss", Scenario { hiss_dbfs: -50.0, ..d() }); }
    if want("fan") { show("fan", Scenario { fan_dbfs: Some(-52.0), ..d() }); }
    if want("hum") { show("hum", Scenario { hum: Some((60.0, -62.0)), ..d() }); }
    if want("bt") { show("bt", Scenario { bandlimit_hz: Some(3400.0), ..d() }); }
    if want("room") { show("room", Scenario { rt60_s: 0.9, reverb_wet_db: -2.0, ..d() }); }
    if want("refl") { show("refl", Scenario { reflection: Some((1.2, 0.6)), ..d() }); }
    if want("plos") { show("plos", Scenario { plosive_db: 4.0, ..d() }); }
    if want("sib") { show("sib", Scenario { sibilance_db: 2.0, ..d() }); }
    if want("boomy") { show("boomy", Scenario { tone: vec![(110.0, 11.0, 0.9)], ..d() }); }
    if want("muddy") { show("muddy", Scenario { tone: vec![(300.0, 8.0, 1.0)], ..d() }); }
    if want("dull") { show("dull", Scenario { tone: vec![(3500.0, -10.0, 0.8), (8000.0, -10.0, 0.8)], ..d() }); }
    if want("harsh") { show("harsh", Scenario { tone: vec![(3500.0, 9.0, 1.0)], ..d() }); }
    if want("typing") { show("typing", Scenario { typing_db: -8.0, ..d() }); }
    if want("tap") { show("tap", Scenario { tap_db: 0.0, ..d() }); }
    if want("drop") { show("drop", Scenario { dropouts: true, ..d() }); }
    if want("moving") { show("moving", Scenario { moving_drop_db: 9.0, ..d() }); }
    if want("auto") { show("auto", Scenario { auto_level: true, ..d() }); }
    if want("silent") { show("silent", Scenario { silent: true, ..d() }); }
}
