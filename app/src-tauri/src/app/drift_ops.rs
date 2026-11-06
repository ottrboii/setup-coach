//! Drift check: run the clap test at the start and again after a wait, on
//! one continuous mic recording, and say whether the offset holds.
//!
//! The mic stays open for the whole check, so the mic's own sample clock is
//! the audio timeline (as in OBS, which times each device by its samples).
//! Pictures are stamped with the PC clock. Each clap check measures the
//! offset between the two; a steady offset is fixable with OBS' Sync Offset,
//! a growing one means two clocks disagree and only a single clock fixes it.
//! Between checks only a sample counter runs (nothing is kept).

use super::avsync_ops::{build_report, SyncRec, SyncReport};
use super::*;
use coach_core::avsync::{self, DriftUnclear, DriftVerdict, OffsetSample};
use coach_core::finding::Finding;
use coach_core::video::Frame;
use serde::{Deserialize, Serialize};

/// Shortest and longest wait between checks (seconds). Short ones are for tests.
const MIN_INTERVAL_S: f64 = 1.0;
const MAX_INTERVAL_S: f64 = 3.0 * 3600.0;
/// One clap window keeps at most this much mic audio (seconds).
const MAX_WINDOW_S: u64 = 60;

/// Mic side of a drift check: total samples so far and the open clap window.
#[derive(Default)]
struct DriftRec {
    rate: u32,
    total: u64,
    /// Sample index the window starts at, and its recording.
    window: Option<(u64, SyncRec)>,
}

pub(super) struct DriftSession {
    rec: Arc<Mutex<DriftRec>>,
    frames: Arc<Mutex<Vec<Frame>>>,
    /// Frame grabber of the open clap window.
    grabbing: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    capture: Option<Box<dyn StreamHandle>>,
    camera_id: String,
    source: Option<String>,
    started: Instant,
    interval_s: f64,
    points: Vec<OffsetSample>,
    last_point_at: Option<f64>,
    last: Option<SyncReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftVerdictView {
    Steady,
    Drifting,
    Unclear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftPoint {
    pub t_s: f64,
    pub offset_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    pub verdict: DriftVerdictView,
    /// "too_few" | "noisy" | "too_short" when unclear.
    pub reason: Option<String>,
    /// Plain-language result.
    pub message: String,
    /// What to do (drifting) or how to get a clear answer (unclear).
    pub steps: Vec<String>,
    /// A fixed Sync Offset / Render Delay will hold.
    pub fixed_offset_ok: bool,
    /// Steady over a short check (under 20 min): a longer one is surer.
    pub short: bool,
    /// id "sync.av_drift" when drifting.
    pub finding: Option<Finding>,
    // Advanced only.
    pub slope_ms_per_10min: f64,
    pub change_ms: f64,
    pub span_min: f64,
    pub points: usize,
    pub spread_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftStatus {
    pub interval_s: f64,
    pub elapsed_s: f64,
    /// Seconds until the next clap check is due; `None` while one is open or before the first.
    pub next_due_in_s: Option<f64>,
    pub clap_open: bool,
    pub points: Vec<DriftPoint>,
    /// Result of the latest clap check (hint when it was unclear).
    pub last: Option<SyncReport>,
    pub report: DriftReport,
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Pure: verdict and plain words for a series of clap checks.
pub(super) fn drift_report(points: &[OffsetSample], capture_card: bool) -> DriftReport {
    let e = avsync::estimate_drift(points);
    let finding = avsync::drift_finding(&e, capture_card);
    let (verdict, reason, mut message, steps) = match e.verdict {
        DriftVerdict::Steady => (
            DriftVerdictView::Steady,
            None,
            "Your sound and picture stay the same distance apart, so a fixed Sync Offset will hold.".to_string(),
            vec![],
        ),
        DriftVerdict::Drifting => {
            let mut s = vec![
                "Bring your mic and your game sound into one device (a mixer or audio interface), so they share one clock.".to_string(),
                "Set every audio device and OBS to 48 kHz: Windows Sound settings > your device > Properties > Advanced, and OBS Settings > Audio.".to_string(),
            ];
            if capture_card {
                s.push("If your game sound comes in through the capture card and also through another cable or app, turn one of them off in OBS.".into());
            }
            s.push("After you change something, press Check again to measure it again.".into());
            (
                DriftVerdictView::Drifting,
                None,
                "The gap between your sound and picture keeps changing, so a fixed Sync Offset won't hold: it would be right now and wrong later in the stream.".to_string(),
                s,
            )
        }
        DriftVerdict::Unclear(u) => {
            let (code, msg, step) = match u {
                DriftUnclear::TooFewPoints => ("too_few", "One more clap check after the wait tells us if the gap holds.", "Leave everything as it is and clap again when we ask."),
                DriftUnclear::Noisy => ("noisy", "The clap checks disagree too much to tell.", "Do one more check: four clear claps with your hands in the middle of the picture."),
                DriftUnclear::TooShort => ("too_short", "It moved a little, but not enough to be sure yet.", "Wait a bit longer and clap again."),
            };
            (DriftVerdictView::Unclear, Some(code.to_string()), msg.to_string(), vec![step.to_string()])
        }
    };
    if e.short {
        message.push_str(" This was a short check; one over 30 minutes or more is surer.");
    }
    DriftReport {
        fixed_offset_ok: verdict == DriftVerdictView::Steady,
        verdict,
        reason,
        message,
        steps,
        short: e.short,
        finding,
        slope_ms_per_10min: r1(e.slope_ms_per_10min),
        change_ms: e.change_ms.round(),
        span_min: r1(e.span_min),
        points: e.points,
        spread_ms: e.spread_ms.map(f64::round),
    }
}

/// Pictures of one clap window on the window's own time: PC-clock stamps
/// minus where the window starts on the mic's clock. If the two clocks
/// agree this is just "time since the window opened"; if not, the
/// difference is the drift and it shows up in the measured offset.
pub(super) fn window_frames(frames: &[Frame], audio_start_s: f64) -> Vec<Frame> {
    frames
        .iter()
        .filter_map(|f| {
            let t = f.timestamp_s - audio_start_s;
            (t >= 0.0).then(|| Frame { timestamp_s: t, ..f.clone() })
        })
        .collect()
}

impl AppCore {
    /// Start a drift check: open the mic for the whole check and start the
    /// first clap window. `interval_s` is the wait between clap checks.
    pub async fn start_drift_check(self: &Arc<Self>, device_id: &str, camera_id: &str, interval_s: f64) -> Result<DriftStatus, CameraError> {
        self.stop_drift_check();
        self.stop_sync_test();
        self.stop_camera_preview();
        let source = self.camera_obs_source(camera_id).await;
        let class = self.camera_class(camera_id);
        if let Err(reason) = self.grab_checked(camera_id, source.as_deref()).await {
            self.note_picture(Some(reason));
            return Err(if class == CameraClass::CaptureCard { CameraError::no_picture_hdmi(reason) } else { CameraError::no_picture(reason) });
        }
        self.note_picture(None);
        let rec = Arc::new(Mutex::new(DriftRec::default()));
        let frames: Arc<Mutex<Vec<Frame>>> = Arc::new(Mutex::new(Vec::new()));
        let (sink_rec, sink_frames, emitter) = (rec.clone(), frames.clone(), self.emitter.clone());
        let sink = Box::new(move |data: &[f32], ch: u16, sr: u32| {
            let mut r = sink_rec.lock().unwrap();
            r.rate = sr;
            let n = (data.len() / ch.max(1) as usize) as u64;
            let cap = u64::from(sr) * MAX_WINDOW_S;
            if let Some((_, w)) = r.window.as_mut() {
                if (w.mono.len() as u64) < cap {
                    w.push(data, ch, sr, &emitter, &sink_frames);
                }
            }
            r.total += n;
        });
        let handle = self.platform.audio.start_capture(device_id, sink).map_err(CameraError::from)?;
        let old = {
            let mut inner = self.inner.lock().unwrap();
            inner.last_device = Some(device_id.to_string());
            inner.sync_fix = None;
            inner.drift.replace(DriftSession {
                rec,
                frames,
                grabbing: Arc::new(AtomicBool::new(false)),
                alive: Arc::new(AtomicBool::new(true)),
                capture: Some(handle),
                camera_id: camera_id.to_string(),
                source,
                started: Instant::now(),
                interval_s: interval_s.clamp(MIN_INTERVAL_S, MAX_INTERVAL_S),
                points: vec![],
                last_point_at: None,
                last: None,
            })
        };
        // Another start may have finished while we waited for the camera: end it, so its mic stream can't linger.
        if let Some(s) = old {
            s.halt();
        }
        self.start_drift_clap()?;
        self.drift_status().ok_or_else(|| CameraError::from("The drift check stopped.".to_string()))
    }

    /// Open a clap window (the first one opens by itself).
    pub fn start_drift_clap(self: &Arc<Self>) -> R<()> {
        // The camera is only read during a clap round; a running camera preview would compete for it.
        self.stop_camera_preview();
        let mut inner = self.inner.lock().unwrap();
        let s = inner.drift.as_mut().ok_or("Start the drift check first.")?;
        s.grabbing.store(false, Ordering::SeqCst);
        {
            let mut r = s.rec.lock().unwrap();
            let start = r.total;
            r.window = Some((start, SyncRec::default()));
        }
        s.frames.lock().unwrap().clear();
        let running = Arc::new(AtomicBool::new(true));
        s.grabbing = running.clone();
        let (id, source, started, frames) = (s.camera_id.clone(), s.source.clone(), s.started, s.frames.clone());
        drop(inner);
        self.spawn_frame_grabber(&id, source, started, running, frames);
        Ok(())
    }

    /// Close the clap window, measure it, add it to the series and say what it means.
    pub async fn finish_drift_clap(self: &Arc<Self>) -> R<DriftStatus> {
        let (window, rate, frames, camera_id) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.drift.as_ref().ok_or("Start the drift check first.")?;
            s.grabbing.store(false, Ordering::SeqCst);
            let (w, rate) = {
                let mut r = s.rec.lock().unwrap();
                (r.window.take().ok_or("Press Clap now first.")?, r.rate)
            };
            let frames = s.frames.lock().unwrap().clone();
            (w, rate, frames, s.camera_id.clone())
        };
        let (start_sample, rec) = window;
        let audio_start_s = if rate > 0 { start_sample as f64 / f64::from(rate) } else { 0.0 };
        let capture_card = self.camera_class(&camera_id) == CameraClass::CaptureCard;
        let (obs_input, sync_ms, render_delay, camera_source, connected) = self.sync_obs_context(&camera_id).await;
        let shifted = window_frames(&frames, audio_start_s);
        let mut last = build_report(&rec.mono, rate, &shifted, rec.claps, sync_ms, render_delay, capture_card, obs_input, camera_source, connected);
        let report = {
            let mut inner = self.inner.lock().unwrap();
            let s = inner.drift.as_mut().ok_or("The drift check was stopped.")?;
            if let Some(o) = last.offset_ms {
                // The series uses what the devices do; OBS' own offset is taken back out so changing it mid-check doesn't look like drift.
                s.points.push(OffsetSample { t_s: audio_start_s, offset_ms: o - sync_ms as f64 });
                s.last_point_at = Some(s.started.elapsed().as_secs_f64());
            }
            let report = drift_report(&s.points, capture_card);
            // Only a steady offset may be fixed with a fixed value.
            if !report.fixed_offset_ok {
                if let Some(f) = last.fix.as_mut() {
                    f.can_auto = false;
                }
            }
            s.last = Some(last.clone());
            let fix = if report.fixed_offset_ok { last.fix.clone() } else { None };
            inner.sync_fix = fix;
            report
        };
        if last.offset_ms.is_some() && report.verdict != DriftVerdictView::Drifting {
            self.spawn_drift_due();
        }
        self.drift_status().ok_or_else(|| "The drift check was stopped.".to_string())
    }

    /// Send `drift-due` once the wait after the latest check is over.
    fn spawn_drift_due(self: &Arc<Self>) {
        let Some((alive, interval, n)) = self.inner.lock().unwrap().drift.as_ref().map(|s| (s.alive.clone(), s.interval_s, s.points.len())) else { return };
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
            let same = me.inner.lock().unwrap().drift.as_ref().is_some_and(|s| s.points.len() == n && s.alive.load(Ordering::SeqCst));
            if alive.load(Ordering::SeqCst) && same {
                me.emitter.emit(crate::types::events::DRIFT_DUE, json!({ "points": n }));
            }
        });
    }

    /// Where the drift check stands, or `None` when none is running.
    pub fn drift_status(&self) -> Option<DriftStatus> {
        let inner = self.inner.lock().unwrap();
        let s = inner.drift.as_ref()?;
        let elapsed = s.started.elapsed().as_secs_f64();
        let clap_open = s.rec.lock().unwrap().window.is_some();
        let capture_card = self.camera_class(&s.camera_id) == CameraClass::CaptureCard;
        let next_due_in_s = match (clap_open, s.last_point_at) {
            (false, Some(t)) => Some(r1((t + s.interval_s - elapsed).max(0.0))),
            _ => None,
        };
        Some(DriftStatus {
            interval_s: s.interval_s,
            elapsed_s: r1(elapsed),
            next_due_in_s,
            clap_open,
            points: s.points.iter().map(|p| DriftPoint { t_s: r1(p.t_s), offset_ms: p.offset_ms.round() }).collect(),
            last: s.last.clone(),
            report: drift_report(&s.points, capture_card),
        })
    }

    /// Drop the open clap round without measuring it (Cancel / "I can't do
    /// this" / an error during a round). Earlier rounds and the mic stay.
    pub fn cancel_drift_clap(&self) -> R<DriftStatus> {
        {
            let inner = self.inner.lock().unwrap();
            let s = inner.drift.as_ref().ok_or("Start the drift check first.")?;
            s.grabbing.store(false, Ordering::SeqCst);
            s.rec.lock().unwrap().window = None;
            s.frames.lock().unwrap().clear();
        }
        self.drift_status().ok_or_else(|| "The drift check was stopped.".to_string())
    }

    /// Stop the whole drift check ("Stop the check", done, app exit). Safe any time.
    pub fn stop_drift_check(&self) {
        let s = self.inner.lock().unwrap().drift.take();
        if let Some(s) = s {
            s.halt();
        }
    }
}

impl DriftSession {
    /// End this session: timers, frame grabber and the mic stream.
    fn halt(mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.grabbing.store(false, Ordering::SeqCst);
        if let Some(h) = self.capture.take() {
            h.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::avsync_ops::tests::{scene, CLAPS};
    use super::*;

    fn pts(v: &[(f64, f64)]) -> Vec<OffsetSample> {
        v.iter().map(|&(min, o)| OffsetSample { t_s: min * 60.0, offset_ms: o }).collect()
    }

    #[test]
    fn steady_says_a_fixed_offset_holds() {
        let r = drift_report(&pts(&[(0.0, -200.0), (30.0, -198.0)]), true);
        assert_eq!(r.verdict, DriftVerdictView::Steady);
        assert!(r.fixed_offset_ok && !r.short && r.finding.is_none());
        assert!(r.message.contains("will hold") && !r.message.contains("ms"));
    }

    #[test]
    fn drift_points_at_the_clock_and_never_offers_a_fixed_offset() {
        let r = drift_report(&pts(&[(0.0, -200.0), (30.0, -140.0)]), true);
        assert_eq!(r.verdict, DriftVerdictView::Drifting);
        assert!(!r.fixed_offset_ok);
        assert_eq!(r.finding.as_ref().unwrap().id, avsync::AV_DRIFT);
        assert!(r.steps.iter().any(|s| s.contains("one device")) && r.steps.iter().any(|s| s.contains("48 kHz")));
        assert!(r.steps.iter().any(|s| s.contains("capture card")) && r.steps.last().unwrap().contains("Check again"));
        assert_eq!(r.slope_ms_per_10min, 20.0);
        assert!(!drift_report(&pts(&[(0.0, -200.0), (30.0, -140.0)]), false).steps.iter().any(|s| s.contains("capture card")));
    }

    #[test]
    fn unclear_has_a_reason_and_a_plain_next_step() {
        let r = drift_report(&pts(&[(0.0, 10.0)]), false);
        assert_eq!((r.verdict, r.reason.as_deref()), (DriftVerdictView::Unclear, Some("too_few")));
        assert!(!r.fixed_offset_ok && !r.steps.is_empty());
        let r = drift_report(&pts(&[(0.0, 0.0), (10.0, 90.0), (20.0, -60.0), (30.0, 40.0)]), false);
        assert_eq!(r.reason.as_deref(), Some("noisy"));
        let r = drift_report(&pts(&[(0.0, 0.0), (1.0, 0.5)]), false);
        assert!(r.short && r.message.contains("short check"));
    }

    #[test]
    fn a_mic_clock_that_runs_slow_shows_up_as_a_growing_offset() {
        // Same 200 ms late picture in both windows. In the second window the
        // mic's clock says 600 s but the PC says 600.08 s: 80 ms of drift.
        let (mic, frames) = scene(&CLAPS, 0.2);
        let at = |audio_start: f64, pc_start: f64| {
            let wall: Vec<Frame> = frames.iter().map(|f| Frame { timestamp_s: f.timestamp_s + pc_start, ..f.clone() }).collect();
            let r = build_report(&mic, 48_000, &window_frames(&wall, audio_start), 5, 0, 0, false, None, None, false);
            r.offset_ms.unwrap()
        };
        let o1 = at(2.0, 2.0);
        let o2 = at(600.0, 600.08);
        assert!((o1 + 200.0).abs() < 20.0, "{o1}");
        assert!(((o2 - o1) + 80.0).abs() < 20.0, "{o1} -> {o2}");
        let r = drift_report(&[OffsetSample { t_s: 2.0, offset_ms: o1 }, OffsetSample { t_s: 600.0, offset_ms: o2 }], false);
        assert_eq!(r.verdict, DriftVerdictView::Drifting, "{r:?}");
    }
}
