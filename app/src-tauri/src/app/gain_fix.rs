//! "Fix my level for me": move the mic's real gain (GoXLR through GoXLR
//! Utility, or a Windows level that is the hardware gain) in small steps and
//! measure after every step. Hardware first, then verify: nothing is ever
//! reported as fixed without a measurement that shows it.

use super::coaching::GainSource;
use super::vault_ops::Target;
use super::{AppCore, R};
use crate::goxlr;
use crate::types::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

/// Speech peaks should sit here (dBFS); same zone as the live gain coach.
const ZONE: (f64, f64) = (-18.0, -12.0);
const TARGET_DBFS: f64 = -15.0;
const MAX_STEPS: u32 = 8;
const MAX_STEP_DB: f64 = 4.0;
/// Quieter than this = we did not hear speech.
const HEARD_DBFS: f64 = -55.0;

enum Measured {
    Peak(f64),
    Silent,
}

impl AppCore {
    fn level_progress(&self, text: &str) {
        self.emitter.emit(events::LEVEL_FIX_PROGRESS, json!({ "text": text }));
    }

    /// Listen for `level_fix_window` and report where the speech peaks sit
    /// (95th percentile of 100 ms block peaks, so one cough doesn't count).
    async fn measure_speech_peak(&self, device: &str) -> R<Measured> {
        let buf: Arc<Mutex<(u32, Vec<f32>)>> = Arc::new(Mutex::new((0, Vec::new())));
        let sink_buf = buf.clone();
        let sink = Box::new(move |data: &[f32], ch: u16, sr: u32| {
            let ch = ch.max(1) as usize;
            let mut b = sink_buf.lock().unwrap();
            b.0 = sr;
            for frame in data.chunks(ch) {
                b.1.push(frame.iter().fold(0.0f32, |m, x| m.max(x.abs())));
            }
        });
        let handle = self.platform.audio.start_capture(device, sink)?;
        tokio::time::sleep(self.level_fix_window).await;
        handle.stop();
        let (sr, samples) = std::mem::take(&mut *buf.lock().unwrap());
        let block = (sr / 10).max(1) as usize;
        let mut peaks: Vec<f32> = samples.chunks(block).filter(|c| c.len() >= block / 2).map(|c| c.iter().fold(0.0f32, |m, x| m.max(*x))).collect();
        if peaks.is_empty() {
            return Ok(Measured::Silent);
        }
        peaks.sort_by(|a, b| a.total_cmp(b));
        let p = peaks[((peaks.len() as f64 * 0.95) as usize).min(peaks.len() - 1)] as f64;
        let db = 20.0 * p.max(1e-9).log10();
        Ok(if db > HEARD_DBFS { Measured::Peak(db) } else { Measured::Silent })
    }

    async fn gain_write(&self, src: &GainSource, db: f64) -> R<()> {
        match src {
            GainSource::Windows { endpoint } => self.platform.system.set_endpoint_gain_db(endpoint, db),
            GainSource::GoXlr { api, serial, mic_type } => api.command(serial, goxlr::cmd_set_mic_gain(*mic_type, db)).await,
        }
    }

    /// Change the gain of the mic on `device_id` in small steps until the
    /// speech peaks sit in the safe zone, measuring after every step.
    /// Always answers with a plain-language `line`; failures are results, not errors.
    pub async fn fix_mic_level(&self, device_id: &str) -> R<LevelFixResult> {
        if self.device_name(device_id).is_none() {
            return Err("That microphone isn't connected any more.".into());
        }
        let res = |status, line: &str| LevelFixResult {
            status,
            line: line.to_string(),
            changed: None,
            gain_before_db: None,
            gain_after_db: None,
            peak_before_dbfs: None,
            peak_after_dbfs: None,
            steps: 0,
            backup_id: None,
        };
        let goxlr_mic = self.is_goxlr_device(device_id);
        let Some(src) = self.gain_source(device_id).await else {
            return Ok(res(
                LevelFixStatus::NeedsYou,
                &if goxlr_mic {
                    "GoXLR Utility isn't running, so we can't change your GoXLR for you. Open GoXLR Utility (its icon is by the clock, or start it from the Start menu) and press this again. Or lower the mic gain yourself in the GoXLR app, and we'll check your level again afterwards.".to_string()
                } else {
                    "We can't change this mic's level for you. If it has a gain knob or a level in its own app, adjust it there; if not, move the mic a bit closer or farther from your mouth. We'll check again afterwards.".to_string()
                },
            ));
        };
        let place = match &src {
            GainSource::GoXlr { .. } => "GoXLR mic gain",
            GainSource::Windows { .. } => "mic level in Windows",
        };
        let Some(start) = self.gain_get(&src).await else {
            return Ok(res(LevelFixStatus::Couldnt, "We couldn't read your mic's current gain, so we left everything as it was."));
        };

        self.stop_capture();
        self.level_progress("Talk like you do on stream for a few seconds. We're listening to your level.");
        let peak0 = match self.measure_speech_peak(device_id).await? {
            Measured::Peak(p) => p,
            Measured::Silent => {
                return Ok(res(LevelFixStatus::NeedsYou, "We couldn't hear you, so nothing was changed. Check that your mic isn't muted, then talk like you do on stream and try again."));
            }
        };
        let mut out = LevelFixResult { gain_before_db: Some(start.current_db), gain_after_db: Some(start.current_db), peak_before_dbfs: Some(peak0), peak_after_dbfs: Some(peak0), ..res(LevelFixStatus::Fixed, "") };
        if (ZONE.0..=ZONE.1).contains(&peak0) {
            out.line = "Your level is already in the safe zone, so nothing needed changing.".into();
            return Ok(out);
        }

        // Back up the previous gain first; if that fails nothing is touched.
        let target = match &src {
            GainSource::GoXlr { .. } => Target::GoXlr,
            GainSource::Windows { endpoint } => Target::EndpointGain(endpoint.clone()),
        };
        let label = if peak0 > ZONE.1 { "Before lowering your mic level" } else { "Before raising your mic level" };
        out.backup_id = self.backup_before(label, &[target]).await?;
        out.changed = Some(place.to_string());
        if let Some(d) = self.inner.lock().unwrap().last_device.clone() {
            self.inner.lock().unwrap().applied_devices.insert(d);
        }

        let mut gain = start.current_db;
        let mut peak = peak0;
        for _ in 0..MAX_STEPS {
            let need = if peak > -0.5 { -6.0 } else { TARGET_DBFS - peak };
            let next = (gain + need.clamp(-MAX_STEP_DB, MAX_STEP_DB)).clamp(start.min_db, start.max_db);
            if (next - gain).abs() < 0.5 {
                break; // at the end of the gain range
            }
            self.level_progress(if need < 0.0 { "Turning the gain down a little and listening again..." } else { "Turning the gain up a little and listening again..." });
            if let Err(e) = self.gain_write(&src, next).await {
                let back = self.gain_write(&src, start.current_db).await.is_ok();
                out.status = LevelFixStatus::Couldnt;
                out.gain_after_db = Some(if back { start.current_db } else { gain });
                out.line = format!("We couldn't change your {place} ({e}). {}", if back { "It is back how it was." } else { "Use Undo to put it back how it was." });
                return Ok(out);
            }
            out.steps += 1;
            tokio::time::sleep(self.level_fix_window / 6).await;
            let actual = self.gain_get(&src).await.map(|g| g.current_db).unwrap_or(next);
            match self.measure_speech_peak(device_id).await? {
                Measured::Silent => {
                    let back = self.gain_write(&src, start.current_db).await.is_ok();
                    out.status = LevelFixStatus::NeedsYou;
                    out.gain_after_db = Some(if back { start.current_db } else { actual });
                    out.line = format!("We lost your voice after changing the {place}, so we {} Talk like you do on stream and try again.", if back { "put it back how it was." } else { "couldn't verify it. Use Undo to put it back how it was." });
                    return Ok(out);
                }
                Measured::Peak(p) => {
                    let asked = actual - gain;
                    let moved = p - peak;
                    // It must move the right way; a clipped start only has to come down.
                    let worked = if peak > -0.5 { p < peak } else { moved * asked.signum() >= asked.abs() * 0.25 };
                    if !worked {
                        let back = self.gain_write(&src, start.current_db).await.is_ok();
                        out.status = LevelFixStatus::Couldnt;
                        out.gain_after_db = Some(if back { start.current_db } else { actual });
                        out.peak_after_dbfs = Some(p);
                        out.line = format!(
                            "Changing the {place} didn't change how loud you are, so it probably isn't what your stream hears. {} Check which input OBS uses.",
                            if back { "We put it back how it was." } else { "Use Undo to put it back how it was." }
                        );
                        return Ok(out);
                    }
                    gain = actual;
                    peak = p;
                    out.gain_after_db = Some(gain);
                    out.peak_after_dbfs = Some(peak);
                }
            }
            if (ZONE.0..=ZONE.1).contains(&peak) {
                break;
            }
        }

        let lowered = gain < start.current_db;
        if (ZONE.0..=ZONE.1).contains(&peak) {
            out.status = LevelFixStatus::Fixed;
            out.line = format!(
                "Your voice was {}, so we turned the {place} {} and listened again: it sits in the safe zone now. You can undo this.",
                if lowered { "too loud" } else { "too quiet" },
                if lowered { "down" } else { "up" }
            );
            return Ok(out);
        }
        // Not there. Keep the change only if the measurement shows it helped.
        if (peak - TARGET_DBFS).abs() < (peak0 - TARGET_DBFS).abs() {
            out.status = LevelFixStatus::NeedsYou;
            out.line = format!(
                "We moved the {place} {} and it's better, but your voice is still {}. {}",
                if lowered { "down" } else { "up" },
                if peak > ZONE.1 { "a bit loud" } else { "a bit quiet" },
                if peak > ZONE.1 {
                    "Move the mic a little farther from your mouth or angle it to the side, then we'll check again."
                } else {
                    "Move the mic a little closer to your mouth, then we'll check again."
                }
            );
        } else {
            let back = self.gain_write(&src, start.current_db).await.is_ok();
            out.status = LevelFixStatus::Couldnt;
            out.gain_after_db = Some(if back { start.current_db } else { gain });
            out.line = format!("We couldn't get your level into the safe zone with the {place}. {}", if back { "We put it back how it was." } else { "Use Undo to put it back how it was." });
        }
        Ok(out)
    }
}
