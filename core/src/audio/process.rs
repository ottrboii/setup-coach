//! Render a [`ProcessingChain`] offline, in the live order: gain → our VST3
//! (mono, high-pass, EQ, de-esser) → OBS expander → compressor → limiter.
//! Used for the before/after preview and
//! to verify a chain by re-analysing its output. Mirrors what OBS + our VST3
//! will do live, closely enough to judge the result.

use super::chain::{EqKind, ProcessingChain};
use super::dsp::{Biquad, from_db};
use super::{AudioBuffer, MicTest};

fn coeff(ms: f64, sr: f64) -> f64 {
    (-1.0 / (ms.max(0.01) * 0.001 * sr)).exp()
}

/// Process a buffer through the chain. Output is mono.
pub fn render(buf: &AudioBuffer, chain: &ProcessingChain) -> AudioBuffer {
    let sr = buf.sample_rate;
    let srf = sr as f64;
    let n = buf.len();
    // Mono: if downmixing a left-only input, take the louder channel; otherwise average.
    let mono: Vec<f64> = if chain.mono_downmix && buf.channels.len() > 1 {
        let e: Vec<f64> = buf.channels.iter().map(|c| c.iter().map(|v| (*v as f64).powi(2)).sum()).collect();
        let best = e.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|x| x.0).unwrap_or(0);
        buf.channels[best].iter().map(|v| *v as f64).collect()
    } else {
        (0..n).map(|i| buf.channels.iter().map(|c| c[i] as f64).sum::<f64>() / buf.channels.len() as f64).collect()
    };

    let mut filters: Vec<Biquad> = Vec::new();
    filters.push(Biquad::highpass(sr, chain.highpass_hz, std::f64::consts::FRAC_1_SQRT_2));
    if chain.highpass_slope_db_oct >= 24 {
        filters.push(Biquad::highpass(sr, chain.highpass_hz, std::f64::consts::FRAC_1_SQRT_2));
    }
    for b in &chain.eq {
        filters.push(match b.kind {
            EqKind::Peak => Biquad::peaking(sr, b.freq_hz, b.q, b.gain_db),
            EqKind::LowShelf => Biquad::low_shelf(sr, b.freq_hz, b.gain_db),
            EqKind::HighShelf => Biquad::high_shelf(sr, b.freq_hz, b.gain_db),
        });
    }

    let gain = from_db(chain.hardware_gain_change_db + chain.input_gain_db);
    // Envelope followers.
    let (ea, er) = (coeff(chain.expander.attack_ms, srf), coeff(chain.expander.release_ms, srf));
    let (ca, cr) = (coeff(chain.compressor.attack_ms, srf), coeff(chain.compressor.release_ms, srf));
    let lr = coeff(chain.limiter.release_ms, srf);
    let mut env_e = 0.0f64;
    let mut env_c = 0.0f64;
    let mut env_s = 0.0f64;
    let mut env_full = 0.0f64;
    let mut lim_gain = 1.0f64;
    // Complementary split (x = low + high exactly), so turning the highs down
    // never smears phase or leaks.
    let mut de_lp = chain.deesser.as_ref().map(|d| Biquad::lowpass(sr, d.freq_hz * 0.7, 0.707));
    let mut de_side = chain.deesser.as_ref().map(|d| Biquad::bandpass(sr, d.freq_hz, 1.0));
    let ceiling = from_db(chain.limiter.ceiling_db);
    let makeup = from_db(chain.compressor.makeup_db);

    let mut out = Vec::with_capacity(n);
    for &x0 in &mono {
        let mut x = x0 * gain;
        for f in filters.iter_mut() {
            x = f.process(x);
        }
        // De-esser (in our VST3, before OBS's dynamics; split-band, relative): when the 'ess' band carries more than
        // `threshold_db` of the total energy, turn only the highs down.
        if let (Some(d), Some(lp), Some(side)) = (chain.deesser.as_ref(), de_lp.as_mut(), de_side.as_mut()) {
            let sb = side.process(x);
            env_s = 0.98 * env_s + 0.02 * sb * sb;
            env_full = 0.995 * env_full + 0.005 * x * x;
            let share = 10.0 * (env_s.max(1e-18) / env_full.max(1e-18)).log10();
            // ~1 ms look at the band (fast) vs ~4 ms full-band average.
            let hf = x - lp.process(x);
            if share > d.threshold_db {
                let g = from_db(-((share - d.threshold_db) * 2.0).min(d.max_reduction_db));
                x += (g - 1.0) * hf;
            }
        }
        // Expander (downward, max 30 dB of attenuation).
        let a = x.abs();
        env_e = if a > env_e { ea * env_e + (1.0 - ea) * a } else { er * env_e + (1.0 - er) * a };
        let lvl = 20.0 * env_e.max(1e-9).log10();
        if lvl < chain.expander.threshold_db {
            let att = ((lvl - chain.expander.threshold_db) * (chain.expander.ratio - 1.0)).max(-30.0);
            x *= from_db(att);
        }
        // Compressor (soft knee 6 dB, RMS-ish detector).
        let p = x * x;
        env_c = if p > env_c { ca * env_c + (1.0 - ca) * p } else { cr * env_c + (1.0 - cr) * p };
        let lvl = 10.0 * env_c.max(1e-18).log10();
        let over = lvl - chain.compressor.threshold_db;
        let knee = 6.0;
        let red = if over <= -knee / 2.0 {
            0.0
        } else if over >= knee / 2.0 {
            over * (1.0 - 1.0 / chain.compressor.ratio)
        } else {
            (1.0 - 1.0 / chain.compressor.ratio) * (over + knee / 2.0).powi(2) / (2.0 * knee)
        };
        x *= from_db(-red) * makeup;
        // Peak limiter (instant attack, smooth release) + hard safety clamp.
        let a = x.abs();
        let need = if a > ceiling { ceiling / a } else { 1.0 };
        lim_gain = if need < lim_gain { need } else { lr * lim_gain + (1.0 - lr) * need.min(1.0) };
        x *= lim_gain;
        x = x.clamp(-ceiling, ceiling);
        out.push(x as f32);
    }
    AudioBuffer::mono(sr, out)
}

/// Render a whole test so it can be re-analysed (verification step).
pub fn render_test(test: &MicTest, chain: &ProcessingChain) -> MicTest {
    MicTest { audio: render(&test.audio, chain), segments: test.segments.clone() }
}

/// Fine-tune the compressor make-up gain by rendering the read and measuring
/// its loudness, so the processed voice actually lands on the LUFS target
/// (EQ cuts, compression and limiting all eat level in ways a formula misses).
pub fn calibrate_makeup(test: &MicTest, chain: &mut ProcessingChain) {
    let Some(seg) = test.segments.iter().find(|s| s.kind == super::SegmentKind::Read) else { return };
    let r = test.range(seg);
    if r.len() < test.audio.sample_rate as usize {
        return;
    }
    let read = AudioBuffer {
        sample_rate: test.audio.sample_rate,
        channels: test.audio.channels.iter().map(|c| c[r.clone()].to_vec()).collect(),
    };
    // Measure (loudness, 95th-percentile speech peak) of the processed read.
    let measure = |chain: &ProcessingChain| {
        let out = render(&read, chain);
        let x = &out.channels[0];
        let lufs = super::dsp::integrated_lufs(x, out.sample_rate);
        let f = super::dsp::frames(x, out.sample_rate, 20.0);
        let peaks: Vec<f64> = f.rms_db.iter().zip(&f.peak_db).filter(|(r, _)| **r > lufs - 20.0).map(|(_, p)| *p).collect();
        (lufs, super::dsp::percentile(&peaks, 95.0))
    };
    let set = |chain: &mut ProcessingChain, v: f64| chain.compressor.makeup_db = ((v.clamp(-12.0, 30.0)) * 10.0).round() / 10.0;
    // Normal speech peaks stay 1.5 dB under the limiter ceiling (-3 dBFS for
    // the usual -1.5 ceiling; lower for an audiobook's -3.5).
    let peak_limit = (-3.0f64).min(chain.limiter.ceiling_db - 1.5);
    // Gentle chains (distant mic, untreated room) stay gentle.
    let max_ratio = if chain.compressor.ratio <= 2.5 { 3.0 } else { 4.0 };
    for tighten in 0..=3 {
        // 1) Hit the loudness target.
        for _ in 0..4 {
            let (lufs, _) = measure(chain);
            let diff = chain.target_lufs - lufs;
            if diff.abs() < 0.5 {
                break;
            }
            let v = chain.compressor.makeup_db + diff.clamp(-12.0, 12.0);
            set(chain, v);
        }
        // 2) Back off until normal speech peaks stay clear of the limiter.
        let (mut lufs, mut p95) = measure(chain);
        let mut steps = 0;
        while p95 > peak_limit && steps < 8 {
            let v = chain.compressor.makeup_db - (p95 - peak_limit).max(1.0);
            set(chain, v);
            (lufs, p95) = measure(chain);
            steps += 1;
        }
        if chain.target_lufs - lufs <= 1.5 {
            break;
        }
        // 3) Too quiet at the peak limit: compress a little more and retry.
        if tighten < 3 {
            chain.compressor.threshold_db -= 2.0;
            chain.compressor.ratio = (chain.compressor.ratio + 0.5).min(max_ratio);
        } else {
            chain.notes.push("Kept your voice slightly under the loudness target so normal talking isn't squashed by the limiter.".into());
        }
    }
}

/// "How viewers hear you on a phone": small-speaker simulation (no deep bass,
/// rolled-off top, mono). Used for the before/after preview toggle.
pub fn phone_speaker(buf: &AudioBuffer) -> AudioBuffer {
    let sr = buf.sample_rate;
    let n = buf.len();
    let mut x: Vec<f32> = (0..n).map(|i| buf.channels.iter().map(|c| c[i]).sum::<f32>() / buf.channels.len().max(1) as f32).collect();
    for mut f in [
        Biquad::highpass(sr, 450.0, 0.707),
        Biquad::highpass(sr, 450.0, 0.707),
        Biquad::peaking(sr, 2500.0, 1.0, 3.0),
        Biquad::lowpass(sr, 9000.0, 0.707),
    ] {
        f.run(&mut x);
    }
    AudioBuffer::mono(sr, x)
}
