//! Real A/B recordings: the user's ORIGINAL sound (first mic test of a
//! session, before we changed anything for that device) and their FINAL sound
//! (the same passage read again after the fixes, rendered through the applied
//! chain with core's offline renderer — static processing only, no AI).
//!
//! Files: `<data>\recordings\<device-slug>\`
//!   original.wav / original.json   – raw whole take (mono float WAV)
//!   original-<date>.wav/.json       – earlier originals (never deleted)
//!   final-raw.wav, final.wav, final.json

use super::{AppCore, R};
use crate::types::{AbError, AbErrorCode, AbInfo, AbTake};
use crate::wav;
use coach_core::audio::{self, dsp, findings::MicContext, process, AudioBuffer, MicReport, MicTest, ProcessingChain, Segment, SegmentKind};
use coach_core::Constraint;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What we keep next to each WAV.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeMeta {
    pub device_id: String,
    pub device_name: String,
    pub recorded_at: String,
    pub sample_rate: u32,
    /// Guided-step marks within the WAV (seconds).
    pub segments: Vec<Segment>,
    /// Analysis summary of the take (the final one: after processing).
    pub speech_lufs: f64,
    pub noise_floor_dbfs: f64,
    pub peak_dbfs: f64,
    pub score: u8,
    /// Newest settings backup when the take was recorded ("original" vault
    /// for the first take: nothing had been changed yet).
    pub backup_id: Option<String>,
    /// The chain `final.wav` was rendered with.
    #[serde(default)]
    pub chain: Option<ProcessingChain>,
    /// All findings of this take as found when it was recorded (the final
    /// take: after processing). `get_ab` shows the open ones.
    #[serde(default)]
    pub findings: Vec<coach_core::Finding>,
}

/// Highest playback level difference we'll correct (beyond this a take is
/// essentially silent and matching would just blow up noise).
const MAX_MATCH_DB: f64 = 30.0;

/// `Microphone (Yeti Stereo Microphone)` → `microphone-yeti-stereo-microphone`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').chars().take(60).collect::<String>();
    if out.is_empty() { "mic".into() } else { out }
}

/// Safe for a Windows folder name.
fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() { "Mic".into() } else { s.chars().take(60).collect() }
}

pub fn mono(buf: &AudioBuffer) -> Vec<f32> {
    let n = buf.len();
    let ch = buf.channels.len().max(1) as f32;
    (0..n).map(|i| buf.channels.iter().map(|c| c[i]).sum::<f32>() / ch).collect()
}

/// The reading part of a take (the passage that's the same in both takes);
/// the whole take if it has no reading mark.
pub fn read_clip(samples: &[f32], sr: u32, segments: &[Segment]) -> Vec<f32> {
    let mut out = Vec::new();
    for s in segments.iter().filter(|s| s.kind == SegmentKind::Read) {
        let a = ((s.start_s * sr as f64) as usize).min(samples.len());
        let b = ((s.end_s * sr as f64) as usize).clamp(a, samples.len());
        out.extend_from_slice(&samples[a..b]);
    }
    if out.is_empty() { samples.to_vec() } else { out }
}

/// Playback gains (dB, ≤ 0) that make two takes equally loud: the louder one
/// is turned down. Returns (gain_a, gain_b, b_minus_a).
pub fn match_gains(lufs_a: f64, lufs_b: f64) -> (f64, f64, f64) {
    let diff = (lufs_b - lufs_a).clamp(-MAX_MATCH_DB, MAX_MATCH_DB);
    if diff > 0.0 { (0.0, -diff, diff) } else { (diff, 0.0, diff) }
}

/// Shown with the result when the loudness difference is too big to match fully.
pub fn match_limit_note(lufs_a: f64, lufs_b: f64) -> Option<String> {
    if (lufs_b - lufs_a).abs() > MAX_MATCH_DB {
        Some(format!("These two takes differ by more than {MAX_MATCH_DB:.0} dB, so they could only be matched part of the way. Check that you recorded the same passage on the same mic."))
    } else {
        None
    }
}

fn gain(x: &[f32], db: f64) -> Vec<f32> {
    let g = dsp::from_db(db) as f32;
    x.iter().map(|v| v * g).collect()
}

/// (original, final, (gain original, gain final, final − original)).
type AbTakes = (Take, Option<Take>, (f64, f64, f64));

struct Take {
    meta: TakeMeta,
    path: PathBuf,
    clip: Vec<f32>,
    lufs: f64,
}

/// `missing` = the code when the take simply isn't there; a take that is there
/// but can't be read is `Unreadable`.
fn load_take(dir: &Path, stem: &str, missing: AbErrorCode) -> Result<Take, AbError> {
    let path = dir.join(format!("{stem}.wav"));
    let json = dir.join(format!("{stem}.json"));
    if !path.exists() || !json.exists() {
        return Err(AbError::new(missing, "We don't have that recording yet."));
    }
    let unreadable = || AbError::new(AbErrorCode::Unreadable, "A saved recording can't be read. Please run the mic test again.");
    let meta: TakeMeta = std::fs::read_to_string(&json)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(unreadable)?;
    let w = wav::read(&path).map_err(|_| unreadable())?;
    let clip = read_clip(&w.samples, w.sample_rate, &meta.segments);
    let lufs = dsp::integrated_lufs(&clip, w.sample_rate);
    Ok(Take { meta, path, clip, lufs })
}

fn write_take(dir: &Path, stem: &str, samples: &[f32], meta: &TakeMeta) -> R<()> {
    wav::write(&dir.join(format!("{stem}.wav")), meta.sample_rate, samples)?;
    wav::write_atomic(&dir.join(format!("{stem}.json")), serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?.as_bytes())
}

fn summary(report: &MicReport) -> (f64, f64, f64, u8) {
    let m = &report.metrics;
    (m.speech_lufs, m.noise_floor_dbfs, m.speech_peak_dbfs, report.score.value)
}

impl AppCore {
    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    /// Folder for a device: by its current name, or (when it's unplugged) the
    /// folder whose recordings carry its id.
    pub fn device_recordings_dir(&self, device_id: &str) -> PathBuf {
        if let Some(d) = self.platform.audio.list_inputs().into_iter().find(|d| d.id == device_id) {
            return self.recordings_dir().join(slug(&d.name));
        }
        let found = std::fs::read_dir(self.recordings_dir()).ok().and_then(|rd| {
            rd.flatten().map(|e| e.path()).find(|p| {
                ["original.json", "final.json"].iter().any(|f| {
                    std::fs::read_to_string(p.join(f)).ok().and_then(|t| serde_json::from_str::<TakeMeta>(&t).ok()).is_some_and(|m| m.device_id == device_id)
                })
            })
        });
        found.unwrap_or_else(|| self.recordings_dir().join(slug(device_id)))
    }

    fn take_meta(&self, device_id: &str, test: &MicTest, report: &MicReport, chain: Option<ProcessingChain>) -> TakeMeta {
        let (speech_lufs, noise_floor_dbfs, peak_dbfs, score) = summary(report);
        TakeMeta {
            device_id: device_id.to_string(),
            device_name: self.device_name(device_id).unwrap_or_else(|| device_id.to_string()),
            recorded_at: crate::paths::now_iso(),
            sample_rate: test.audio.sample_rate,
            segments: test.segments.clone(),
            speech_lufs,
            noise_floor_dbfs,
            peak_dbfs,
            score,
            backup_id: self.vault().latest_id(),
            chain,
            findings: report.findings.clone(),
        }
    }

    /// Called after every mic test: the first test of a device in this
    /// session, before anything was applied for it, becomes `original.wav`.
    /// An existing original is kept as `original-<date>.wav`.
    pub(super) fn maybe_save_original(&self, device_id: &str, test: &MicTest, report: &MicReport) -> R<bool> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.applied_devices.contains(device_id) || inner.originals_saved.contains(device_id) {
                return Ok(false);
            }
        }
        let dir = self.device_recordings_dir(device_id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't save your recording ({e})."))?;
        if dir.join("original.wav").exists() {
            let date: String = std::fs::read_to_string(dir.join("original.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<TakeMeta>(&t).ok())
                .map(|m| m.recorded_at)
                .unwrap_or_else(crate::paths::now_iso)
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            let mut n = 0;
            let stem = loop {
                let s = if n == 0 { format!("original-{date}") } else { format!("original-{date}-{n}") };
                if !dir.join(format!("{s}.wav")).exists() {
                    break s;
                }
                n += 1;
            };
            std::fs::rename(dir.join("original.wav"), dir.join(format!("{stem}.wav"))).map_err(|e| format!("Couldn't keep your earlier recording ({e})."))?;
            let _ = std::fs::rename(dir.join("original.json"), dir.join(format!("{stem}.json")));
        }
        let mut meta = self.take_meta(device_id, test, report, None);
        meta.backup_id = Some(crate::vault::ORIGINAL.into());
        write_take(&dir, "original", &mono(&test.audio), &meta)?;
        self.inner.lock().unwrap().originals_saved.insert(device_id.to_string());
        Ok(true)
    }

    /// Stop the capture started with `start_mic_test` and keep it as the final
    /// take: `final-raw.wav` (as captured) and `final.wav` (through the
    /// applied chain). The UI runs the same guided "read" step as the first
    /// test (same passage).
    pub fn record_final(&self, device_id: &str, ctx: MicContext, constraints: Vec<Constraint>) -> Result<AbInfo, AbError> {
        use AbErrorCode::*;
        self.stop_capture();
        let (rec, last) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.recording.take().ok_or(AbError::new(NoRecording, "Start the recording first."))?, inner.last_device.clone())
        };
        if last.as_deref() != Some(device_id) {
            return Err(AbError::new(WrongMic, "That recording was made with a different mic. Please record again."));
        }
        let test = {
            let r = rec.lock().unwrap();
            if r.len() < r.sample_rate as usize {
                return Err(AbError::new(TooShort, "The recording is too short. Please read the passage again."));
            }
            let segments = super::segments_from_marks(&r.marks, r.len() as f64 / r.sample_rate as f64);
            if !segments.iter().any(|s| s.kind == SegmentKind::Read) {
                return Err(AbError::new(NoReadStep, "We didn't get the reading step. Please read the passage again."));
            }
            MicTest { audio: AudioBuffer { sample_rate: r.sample_rate, channels: r.channels.clone() }, segments }
        };
        let chain = {
            let inner = self.inner.lock().unwrap();
            inner.applied_chain.clone()
        }
        .or_else(|| self.load_setup().and_then(|p| p.mic_chain))
        .ok_or(AbError::new(NoAppliedChain, "Apply your new settings first, then record the final take."))?;
        let dir = self.device_recordings_dir(device_id);
        if !dir.join("original.wav").exists() {
            return Err(AbError::new(NoFirstTest, "We don't have your first recording for this mic, so there's nothing to compare with yet."));
        }
        let rendered = process::render_test(&test, &chain);
        let report = audio::scan(&rendered, &ctx, &constraints);
        let meta = self.take_meta(device_id, &test, &report, Some(chain));
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't save your recording ({e})."))?;
        wav::write(&dir.join("final-raw.wav"), meta.sample_rate, &mono(&test.audio))?;
        write_take(&dir, "final", &mono(&rendered.audio), &meta)?;
        self.get_ab(device_id)
    }

    fn ab_takes(&self, device_id: &str) -> Result<AbTakes, AbError> {
        let dir = self.device_recordings_dir(device_id);
        let original = match load_take(&dir, "original", AbErrorCode::NoFirstTest) {
            Err(e) if e.code == AbErrorCode::NoFirstTest => {
                let unknown = !dir.exists() && self.platform.audio.list_inputs().iter().all(|d| d.id != device_id);
                return Err(if unknown {
                    AbError::new(AbErrorCode::MicMissing, "We can't find that microphone, and we have no recordings for it. Plug it in and run the mic test.")
                } else {
                    AbError::new(AbErrorCode::NoFirstTest, "Run the mic test first: we keep your first recording for the comparison.")
                });
            }
            other => other?,
        };
        let fin = if dir.join("final.json").exists() { Some(load_take(&dir, "final", AbErrorCode::NoFinalTake)?) } else { None };
        let gains = fin.as_ref().map(|f| match_gains(original.lufs, f.lufs)).unwrap_or((0.0, 0.0, 0.0));
        Ok((original, fin, gains))
    }

    pub fn get_ab(&self, device_id: &str) -> Result<AbInfo, AbError> {
        let (o, f, (_, _, diff)) = self.ab_takes(device_id)?;
        let note = f.as_ref().and_then(|f| match_limit_note(o.lufs, f.lufs));
        let findings = f.as_ref().map(|f| f.meta.findings.iter().filter(|x| x.severity >= coach_core::Severity::Minor).cloned().collect()).unwrap_or_default();
        let take = |t: &Take| AbTake { path: t.path.display().to_string(), lufs: (t.lufs * 10.0).round() / 10.0, score: t.meta.score };
        Ok(AbInfo { original: take(&o), final_take: f.as_ref().map(take), matched_gain_db: (diff * 10.0).round() / 10.0, note, findings })
    }

    /// The loudness-matched reading clips (original, final).
    fn matched_clips(&self, device_id: &str) -> R<(u32, Vec<f32>, Option<Vec<f32>>)> {
        let (o, f, (ga, gb, _)) = self.ab_takes(device_id)?;
        let sr = o.meta.sample_rate;
        Ok((sr, gain(&o.clip, ga), f.map(|f| gain(&f.clip, gb))))
    }

    pub(super) fn ab_preview(&self, final_take: bool) -> R<AudioBuffer> {
        let device = self.inner.lock().unwrap().last_device.clone().ok_or("Run the mic test first.")?;
        let (sr, o, f) = self.matched_clips(&device)?;
        let x = if final_take { f.ok_or("Record your final take first.")? } else { o };
        Ok(AudioBuffer::mono(sr, x))
    }

    /// Copy both loudness-matched takes to the Desktop:
    /// `Setup Coach A-B - <device>\A original.wav`, `B final.wav`.
    pub fn export_ab(&self, device_id: &str) -> Result<String, AbError> {
        let (sr, o, f) = self.matched_clips(device_id)?;
        let f = f.ok_or(AbError::new(AbErrorCode::NoFinalTake, "Record your final take first."))?;
        let name = self.device_name(device_id).unwrap_or_else(|| {
            std::fs::read_to_string(self.device_recordings_dir(device_id).join("original.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<TakeMeta>(&t).ok())
                .map(|m| m.device_name)
                .unwrap_or_else(|| "Mic".into())
        });
        let dir = crate::selftest::default_out_dir(self).join(format!("Setup Coach A-B - {}", file_safe(&name)));
        wav::write(&dir.join("A original.wav"), sr, &o)?;
        wav::write(&dir.join("B final.wav"), sr, &f)?;
        Ok(dir.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Microphone (Yeti Stereo Microphone)"), "microphone-yeti-stereo-microphone");
        assert_eq!(slug("  "), "mic");
        assert_eq!(file_safe("Mic: \"RE20\" / XLR"), "Mic RE20 XLR");
    }

    #[test]
    fn huge_loudness_gap_gets_a_note() {
        assert!(match_limit_note(-20.0, -60.0).is_some());
        assert!(match_limit_note(-20.0, -49.0).is_none());
    }

    #[test]
    fn minus_6_db_copy_is_matched() {
        let sr = 48_000;
        let a: Vec<f32> = (0..sr * 4).map(|i| ((i as f32) * 0.07).sin() * 0.3 * (1.0 + ((i as f32) * 0.0003).sin()) / 2.0).collect();
        let b = gain(&a, -6.0);
        let la = dsp::integrated_lufs(&a, sr as u32);
        let lb = dsp::integrated_lufs(&b, sr as u32);
        let (ga, gb, diff) = match_gains(la, lb);
        assert!((diff + 6.0).abs() < 0.5, "{diff}");
        assert_eq!(gb, 0.0);
        let la2 = dsp::integrated_lufs(&gain(&a, ga), sr as u32);
        assert!((la2 - lb).abs() < 0.5, "{la2} vs {lb}");
    }
}
