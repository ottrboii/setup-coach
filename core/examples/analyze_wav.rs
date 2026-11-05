//! Analyse a real guided-test recording: `cargo run --release --example analyze_wav -- <rec.wav> <rec.json>`
//! (the pair the app saves under %APPDATA%\SetupCoach\recordings\<mic>\original.*).
//! Prints the metrics and findings, plus the low-frequency / desk-vibration
//! numbers per step, so thresholds can be checked against real recordings.

use coach_core::audio::{self, AudioBuffer, MicContext, MicTest, Segment};
use serde::Deserialize;

#[derive(Deserialize)]
struct Meta {
    sample_rate: u32,
    segments: Vec<Segment>,
}

/// Minimal RIFF reader: 16-bit PCM or 32-bit float, any channel count.
fn read_wav(path: &str) -> AudioBuffer {
    let b = std::fs::read(path).expect("read wav");
    let mut pos = 12;
    let (mut fmt, mut ch, mut sr, mut bits) = (0u16, 1usize, 48000u32, 16u16);
    let mut data: &[u8] = &[];
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        match id {
            b"fmt " => {
                fmt = u16::from_le_bytes(body[0..2].try_into().unwrap());
                ch = u16::from_le_bytes(body[2..4].try_into().unwrap()) as usize;
                sr = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = body,
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    let samples: Vec<f32> = match (fmt, bits) {
        (3, 32) => data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        (1, 16) => data.chunks_exact(2).map(|c| i16::from_le_bytes(c.try_into().unwrap()) as f32 / 32768.0).collect(),
        _ => panic!("unsupported wav format {fmt}/{bits}"),
    };
    let mut channels = vec![Vec::new(); ch];
    for (i, s) in samples.iter().enumerate() {
        channels[i % ch].push(*s);
    }
    AudioBuffer { sample_rate: sr, channels }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: analyze_wav <rec.wav> <rec.json>");
        return;
    }
    let audio_buf = read_wav(&args[0]);
    let meta: Meta = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    assert_eq!(meta.sample_rate, audio_buf.sample_rate);
    let test = MicTest { audio: audio_buf, segments: meta.segments };
    let r = audio::scan(&test, &MicContext::default(), &[]);
    let m = &r.metrics;
    println!("score {} | floor {:.1} speech rms {:.1} pk {:.1} typing {:?} tap {:?}", r.score.value, m.noise_floor_dbfs, m.speech_rms_dbfs, m.speech_peak_dbfs, m.typing_rel_db, m.tap_rel_db);
    println!("vibration: {:?}", m.vibration);
    for f in &r.findings {
        println!("  - {:?} {} | {} | {:?}", f.severity, f.id, f.title, f.evidence);
    }
    if std::env::var("TIMELINE").is_ok() {
        // Per step, every 250 ms: low-frequency (20-120 Hz) peak / full-band peak in dBFS.
        use coach_core::audio::{dsp, vibration::lf_band};
        let sr = test.audio.sample_rate;
        let x = &test.audio.channels[0];
        let lf = lf_band(x, sr);
        for seg in &test.segments {
            let r = test.range(seg);
            let step = (sr as usize) / 4;
            let mut line = String::new();
            let mut i = r.start;
            while i + step <= r.end {
                let l = dsp::db_amp(dsp::peak_abs(&lf[i..i + step]));
                let f = dsp::db_amp(dsp::peak_abs(&x[i..i + step]));
                line.push_str(&format!("{:.0}/{:.0} ", l, f));
                i += step;
            }
            println!("{:?}: {line}", seg.kind);
        }
    }
    for n in &r.chain.notes {
        println!("  note: {n}");
    }
}
