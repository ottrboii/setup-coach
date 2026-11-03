//! Minimal WAV I/O for the A/B recordings: we write 32-bit float mono (the
//! capture is float already, so nothing is lost) and read float or 16/24/32-bit
//! PCM. Writes are atomic (temp file + rename).

use std::path::Path;

/// Mono samples + rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

/// WAVE_FORMAT_IEEE_FLOAT, mono, with a `fact` chunk.
pub fn encode_f32(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 4) as u32;
    let mut b = Vec::with_capacity(58 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(4 + 26 + 12 + 8 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&18u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&sample_rate.to_le_bytes());
    b.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes()); // cbSize
    b.extend_from_slice(b"fact");
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// Decode a PCM (16/24/32-bit) or float (32/64-bit) WAV; channels are averaged to mono.
pub fn decode(bytes: &[u8]) -> Result<Wav, String> {
    let bad = || "That recording file is damaged.".to_string();
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad());
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32_at(pos + 4) as usize;
        let body = pos + 8;
        let end = body.checked_add(len).filter(|e| *e <= bytes.len()).ok_or_else(bad)?;
        if id == b"fmt " && len >= 16 {
            let mut tag = u16_at(body);
            if tag == 0xFFFE && len >= 26 {
                tag = u16_at(body + 24); // WAVE_FORMAT_EXTENSIBLE sub-format
            }
            fmt = Some((tag, u16_at(body + 2).max(1), u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            let (tag, ch, sr, bits) = fmt.ok_or_else(bad)?;
            let data = &bytes[body..end];
            let bps = (bits as usize).div_ceil(8);
            let frame = bps * ch as usize;
            if frame == 0 {
                return Err(bad());
            }
            let sample = |o: usize| -> f32 {
                let s = &data[o..o + bps];
                match (tag, bits) {
                    (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    (3, 64) => f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32,
                    (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32_768.0,
                    (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                    (1, 32) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                    _ => 0.0,
                }
            };
            if !matches!((tag, bits), (3, 32) | (3, 64) | (1, 16) | (1, 24) | (1, 32)) {
                return Err("That recording uses a WAV format we can't read.".into());
            }
            let n = data.len() / frame;
            let samples = (0..n).map(|i| (0..ch as usize).map(|c| sample(i * frame + c * bps)).sum::<f32>() / ch as f32).collect();
            return Ok(Wav { sample_rate: sr, samples });
        }
        pos = end + (len & 1);
    }
    Err(bad())
}

/// Write bytes atomically: `<path>.tmp` then rename over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("Couldn't save ({e})."))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("Couldn't save ({e})."))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Couldn't save ({e})."))
}

pub fn write(path: &Path, sample_rate: u32, samples: &[f32]) -> Result<(), String> {
    write_atomic(path, &encode_f32(sample_rate, samples))
}

pub fn read(path: &Path) -> Result<Wav, String> {
    decode(&std::fs::read(path).map_err(|_| "That recording is missing.".to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_round_trip() {
        let s: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let w = decode(&encode_f32(48_000, &s)).unwrap();
        assert_eq!(w.sample_rate, 48_000);
        assert_eq!(w.samples, s);
    }

    #[test]
    fn reads_pcm24_stereo() {
        // 2 frames, stereo, 24-bit: (0.5, -0.5), (0, 0.25)
        let enc = |x: f32| {
            let v = (x * 8_388_608.0) as i32;
            [v as u8, (v >> 8) as u8, (v >> 16) as u8]
        };
        let mut data = Vec::new();
        for x in [0.5f32, -0.5, 0.0, 0.25] {
            data.extend_from_slice(&enc(x));
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(4 + 24 + 8 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 2] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&(48_000u32 * 6).to_le_bytes());
        b.extend_from_slice(&6u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        let w = decode(&b).unwrap();
        assert_eq!(w.samples.len(), 2);
        assert!(w.samples[0].abs() < 1e-6);
        assert!((w.samples[1] - 0.125).abs() < 1e-5);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not a wav").is_err());
    }
}
