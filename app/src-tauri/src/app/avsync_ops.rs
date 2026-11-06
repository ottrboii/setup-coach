//! Lip-sync clap test: record the mic and the camera picture together while
//! the user claps, measure the offset (`coach_core::avsync`), say which side
//! to delay, and optionally set OBS' Sync Offset for them.
//!
//! The mic is read directly from the device, so OBS' own Sync Offset on that
//! input is not in the recording: the measurement adds it back, which is what
//! makes "check again" meaningful after the offset was changed. The picture
//! comes from OBS' screenshot of the camera source when OBS is connected
//! (that includes a Render Delay filter on the camera) or from the camera.

use super::*;
use coach_core::avsync::{self, SyncMeasurement};
use coach_core::finding::Finding;
use coach_core::video::Frame;
use serde::{Deserialize, Serialize};

/// Frames kept at most (a clap test is ~12 s; this is a safety net).
const MAX_FRAMES: usize = 3000;
/// Width the frames are reduced to for the motion analysis.
const FRAME_WIDTH: usize = 160;
/// Below this many frames per second the answer is only approximate.
const COARSE_FPS: f64 = 8.0;
/// Fewest claps needed (same as the engine).
const MIN_CLAPS: usize = 3;

pub(super) struct SyncSession {
    rec: Arc<Mutex<SyncRec>>,
    frames: Arc<Mutex<Vec<Frame>>>,
    running: Arc<AtomicBool>,
    capture: Option<Box<dyn StreamHandle>>,
    camera_id: String,
    device_id: String,
    started: Instant,
}

/// Mic samples plus the live clap detector that feeds the on-screen meter.
#[derive(Default)]
struct SyncRec {
    rate: u32,
    mono: Vec<f32>,
    win_sq: f64,
    win_n: usize,
    floor: f64,
    claps: usize,
    last_clap_s: f64,
    windows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSide {
    /// The voice is late: delay the picture (OBS Render Delay on the camera).
    DelayCamera,
    /// The picture is late: delay the voice (OBS Sync Offset on the mic).
    DelaySound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncFix {
    pub side: SyncSide,
    /// The value to type in (absolute): Render Delay ms or Sync Offset ms.
    pub ms: u32,
    /// How much more delay than there is now.
    pub change_ms: u32,
    /// Plain-language steps for doing it by hand.
    pub steps: Vec<String>,
    /// The app can set it itself (delay_sound with OBS connected and the mic input found).
    pub can_auto: bool,
    /// OBS input to set the Sync Offset on (delay_sound).
    pub obs_input: Option<String>,
    /// OBS source of the camera (delay_camera), when found.
    pub camera_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncVerdict {
    InSync,
    /// Viewers hear you after they see you.
    SoundLate,
    /// Viewers hear you before they see you.
    SoundEarly,
    /// The test gave no clear answer.
    Unclear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncReport {
    pub verdict: SyncVerdict,
    /// What viewers get right now (positive: sound after picture). The mic's
    /// measured offset plus any OBS Sync Offset already set. `None` when unclear.
    pub offset_ms: Option<f64>,
    /// 0..1 how clear the match was.
    pub score: f64,
    /// Claps the live detector heard.
    pub claps: usize,
    pub frames_per_s: f64,
    /// Fewer than ~8 pictures a second: the number is a rough guide only.
    pub coarse: bool,
    pub finding: Option<Finding>,
    pub fix: Option<SyncFix>,
    /// Plain-language reason when the test was unclear (and what to change).
    pub hint: Option<String>,
    /// OBS Sync Offset that was already set on the mic input (added to the measurement).
    pub obs_sync_offset_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncApplyResult {
    pub ok: bool,
    pub input: Option<String>,
    pub previous_ms: Option<i64>,
    pub applied_ms: Option<i64>,
    pub message: String,
}

fn round10(x: f64) -> u32 {
    ((x.abs() / 10.0).round() * 10.0) as u32
}

/// Pure part of the finish step: measure, then work out the verdict and fix.
/// `obs_sync_offset_ms` is the OBS Sync Offset already set on the mic input;
/// `camera_render_delay_ms` the Render Delay already on the camera source.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_report(
    mic: &[f32],
    rate: u32,
    frames: &[Frame],
    heard_claps: usize,
    obs_sync_offset_ms: i64,
    camera_render_delay_ms: u32,
    capture_card: bool,
    obs_input: Option<String>,
    camera_source: Option<String>,
    obs_connected: bool,
) -> SyncReport {
    let span = match (frames.first(), frames.last()) {
        (Some(a), Some(b)) if b.timestamp_s > a.timestamp_s => b.timestamp_s - a.timestamp_s,
        _ => 0.0,
    };
    let fps = if span > 0.0 { (frames.len() as f64 - 1.0) / span } else { 0.0 };
    let base = SyncReport {
        verdict: SyncVerdict::Unclear,
        offset_ms: None,
        score: 0.0,
        claps: heard_claps,
        frames_per_s: (fps * 10.0).round() / 10.0,
        coarse: fps < COARSE_FPS,
        finding: None,
        fix: None,
        hint: None,
        obs_sync_offset_ms,
    };
    let Some(m) = avsync::measure_offset(mic, rate, frames) else {
        let hint = if heard_claps < MIN_CLAPS {
            format!("We only heard {heard_claps} clap{}. Clap four times, not too fast, with your hands where the camera can see them.", if heard_claps == 1 { "" } else { "s" })
        } else if frames.len() < 4 {
            "We didn't get enough pictures from your camera. Close other apps that use it and try again.".to_string()
        } else {
            "We heard your claps but couldn't match them to your hands in the picture. Clap four times with a short pause after the second, keep your hands in the middle of the picture, and make the room a bit brighter.".to_string()
        };
        return SyncReport { hint: Some(hint), ..base };
    };
    // What viewers get: the mic as measured plus the Sync Offset OBS adds.
    let effective = SyncMeasurement { offset_ms: m.offset_ms + obs_sync_offset_ms as f64, ..m };
    let finding = avsync::finding(&effective, capture_card);
    let o = effective.offset_ms;
    let mut report = SyncReport { offset_ms: Some(o.round()), score: (m.score * 100.0).round() / 100.0, claps: heard_claps.max(m.events), ..base };
    let Some(finding) = finding else {
        report.verdict = SyncVerdict::InSync;
        if report.coarse {
            report.hint = Some("Your camera gave us only a few pictures a second, so this is a rough check. It looks fine.".into());
        }
        return report;
    };
    report.finding = Some(finding);
    let change = round10(o);
    if o >= 0.0 {
        let ms = camera_render_delay_ms + change;
        let mut steps = vec!["In OBS, right-click your camera in the Sources list and choose Filters.".to_string()];
        steps.push(if camera_render_delay_ms > 0 {
            format!("Open the Render Delay filter you already have and change it to {ms} ms (it is {camera_render_delay_ms} ms now).")
        } else {
            format!("Under Effect Filters press the + button, choose Render Delay, and set it to {ms} ms.")
        });
        steps.push("Close the window, come back here and press Check again.".into());
        report.verdict = SyncVerdict::SoundLate;
        report.fix = Some(SyncFix { side: SyncSide::DelayCamera, ms, change_ms: change, steps, can_auto: false, obs_input: None, camera_source });
    } else {
        let current = obs_sync_offset_ms.max(0) as u32;
        let ms = current + change;
        let steps = vec![
            "In OBS, find your mic in the Audio Mixer, click the gear icon next to it and choose Advanced Audio Properties.".to_string(),
            format!("Set the Sync Offset of your mic to {ms} ms."),
            "Close the window, come back here and press Check again.".into(),
        ];
        let can_auto = obs_connected && obs_input.is_some();
        report.verdict = SyncVerdict::SoundEarly;
        report.fix = Some(SyncFix { side: SyncSide::DelaySound, ms, change_ms: change, steps, can_auto, obs_input, camera_source: None });
    }
    report
}

impl AppCore {
    /// Start recording the mic and the camera picture together. Rejects with
    /// a `CameraError` when there is no usable picture (same as the camera test).
    pub async fn start_sync_test(self: &Arc<Self>, device_id: &str, camera_id: &str) -> Result<(), CameraError> {
        self.stop_sync_session();
        self.stop_camera_preview();
        let source = self.camera_obs_source(camera_id).await;
        let class = self.camera_class(camera_id);
        if let Err(reason) = self.grab_checked(camera_id, source.as_deref()).await {
            self.note_picture(Some(reason));
            return Err(if class == CameraClass::CaptureCard { CameraError::no_picture_hdmi(reason) } else { CameraError::no_picture(reason) });
        }
        self.note_picture(None);

        let rec = Arc::new(Mutex::new(SyncRec::default()));
        let frames: Arc<Mutex<Vec<Frame>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_rec = rec.clone();
        let sink_frames = frames.clone();
        let emitter = self.emitter.clone();
        let sink = Box::new(move |data: &[f32], ch: u16, sr: u32| {
            let ch = ch.max(1) as usize;
            let mut r = sink_rec.lock().unwrap();
            r.rate = sr;
            let win = (sr / 100).max(1) as usize;
            for frame in data.chunks(ch) {
                let v = frame.iter().fold(0.0f32, |a, x| a.max(x.abs()));
                r.mono.push(v);
                r.win_sq += f64::from(v) * f64::from(v);
                r.win_n += 1;
                if r.win_n < win {
                    continue;
                }
                let rms = (r.win_sq / r.win_n as f64).sqrt();
                r.win_sq = 0.0;
                r.win_n = 0;
                r.windows += 1;
                let t = r.mono.len() as f64 / f64::from(sr);
                if r.floor == 0.0 {
                    r.floor = rms.max(1e-5);
                }
                let clap = rms > (r.floor * 4.0).max(0.02) && t - r.last_clap_s > 0.25;
                if clap {
                    r.claps += 1;
                    r.last_clap_s = t;
                } else if rms < r.floor * 2.0 {
                    r.floor = (r.floor * 0.98 + rms * 0.02).max(1e-5);
                }
                if clap || r.windows % 5 == 0 {
                    let n_frames = sink_frames.lock().map(|f| f.len()).unwrap_or(0);
                    emitter.emit(
                        crate::types::events::SYNC_PROGRESS,
                        json!({ "t_s": (t * 10.0).round() / 10.0, "level_db": (20.0 * rms.max(1e-9).log10() * 10.0).round() / 10.0, "claps": r.claps, "clap": clap, "frames": n_frames }),
                    );
                }
            }
        });
        let handle = self.platform.audio.start_capture(device_id, sink).map_err(CameraError::from)?;
        let started = Instant::now();
        let running = Arc::new(AtomicBool::new(true));
        {
            let mut inner = self.inner.lock().unwrap();
            inner.last_device = Some(device_id.to_string());
            inner.sync_fix = None;
            inner.sync = Some(SyncSession {
                rec,
                frames: frames.clone(),
                running: running.clone(),
                capture: Some(handle),
                camera_id: camera_id.to_string(),
                device_id: device_id.to_string(),
                started,
            });
        }
        let me = self.clone();
        let id = camera_id.to_string();
        tokio::spawn(async move {
            let mut last_preview = Instant::now() - Duration::from_secs(1);
            while running.load(Ordering::SeqCst) {
                let t0 = started.elapsed().as_secs_f64();
                match me.grab_checked(&id, source.as_deref()).await {
                    Ok(f) => {
                        // The picture belongs to the middle of the time the grab took.
                        let t = (t0 + started.elapsed().as_secs_f64()) / 2.0;
                        // Show the user their own picture (about 4 a second) so they can put their hands in view.
                        if last_preview.elapsed() >= Duration::from_millis(250) {
                            last_preview = Instant::now();
                            if let Some(url) = camera::jpeg_data_url(&camera::downscale(&f, 480)) {
                                me.emitter.emit(crate::types::events::CAMERA_PREVIEW, json!({ "image": url }));
                            }
                        }
                        let mut small = camera::downscale(&f, FRAME_WIDTH);
                        small.timestamp_s = t;
                        let mut fr = frames.lock().unwrap();
                        if fr.len() < MAX_FRAMES {
                            fr.push(small);
                        }
                    }
                    Err(reason) => me.note_picture(Some(reason)),
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        Ok(())
    }

    /// Stop recording and drop it (used by "I can't do this" and when leaving the screen).
    pub fn stop_sync_test(&self) {
        self.stop_sync_session();
    }

    fn stop_sync_session(&self) -> Option<SyncSession> {
        let s = self.inner.lock().unwrap().sync.take();
        s.map(|mut s| {
            s.running.store(false, Ordering::SeqCst);
            if let Some(h) = s.capture.take() {
                h.stop();
            }
            s
        })
    }

    /// Stop recording, measure, and say what to do.
    pub async fn finish_sync_test(&self) -> R<SyncReport> {
        let s = self.stop_sync_session().ok_or("Start the clap test first.")?;
        let (mic, rate, claps) = {
            let r = s.rec.lock().unwrap();
            (r.mono.clone(), r.rate, r.claps)
        };
        let frames = s.frames.lock().unwrap().clone();
        let _ = s.started;
        let capture_card = self.camera_class(&s.camera_id) == CameraClass::CaptureCard;

        let (mut obs_input, mut sync_ms, mut render_delay, mut camera_source, mut connected) = (None, 0i64, 0u32, None, false);
        if let Some(o) = self.obs().await {
            connected = true;
            let input = self.resolve_obs_input("").await;
            if let Ok(r) = o.request("GetInputAudioSyncOffset", json!({ "inputName": input })).await {
                sync_ms = r["inputAudioSyncOffset"].as_i64().unwrap_or(0);
                obs_input = Some(input);
            }
            camera_source = self.camera_obs_source(&s.camera_id).await;
            if let Some(src) = &camera_source {
                if let Ok(fl) = obs::filters_of(o.as_ref(), src).await {
                    render_delay = fl.iter().filter(|f| f.kind == "gpu_delay" && f.enabled).map(|f| f.settings["delay_ms"].as_u64().unwrap_or(0) as u32).sum();
                }
            }
        }
        let _ = &s.device_id;
        let report = build_report(&mic, rate, &frames, claps, sync_ms, render_delay, capture_card, obs_input, camera_source, connected);
        self.inner.lock().unwrap().sync_fix = report.fix.clone();
        Ok(report)
    }

    /// Set the OBS Sync Offset the last result asked for. Remembers the old
    /// value so `undo_sync_fix` can put it back.
    pub async fn apply_sync_fix(&self) -> SyncApplyResult {
        let fail = |m: &str| SyncApplyResult { ok: false, input: None, previous_ms: None, applied_ms: None, message: m.into() };
        let Some(fix) = self.inner.lock().unwrap().sync_fix.clone() else {
            return fail("Run the clap test first, then we can set this for you.");
        };
        let (SyncSide::DelaySound, Some(input)) = (fix.side, fix.obs_input.clone()) else {
            return fail("This one has to be done in OBS by hand. The steps are on the screen.");
        };
        let Some(o) = self.obs().await else {
            return fail("We can't reach OBS. Turn on its WebSocket server (Tools > WebSocket Server Settings), or follow the steps on screen.");
        };
        let previous = match o.request("GetInputAudioSyncOffset", json!({ "inputName": input })).await {
            Ok(r) => r["inputAudioSyncOffset"].as_i64().unwrap_or(0),
            Err(_) => return fail("OBS didn't answer. Follow the steps on screen to set it by hand."),
        };
        let ms = i64::from(fix.ms);
        if let Err(e) = o.request("SetInputAudioSyncOffset", json!({ "inputName": input, "inputAudioSyncOffset": ms })).await {
            self.log_ui("warn", "avsync", &format!("SetInputAudioSyncOffset failed: {e}"));
            return fail("OBS wouldn't change it. Follow the steps on screen to set it by hand.");
        }
        self.inner.lock().unwrap().sync_prev = Some((input.clone(), previous));
        SyncApplyResult {
            ok: true,
            input: Some(input.clone()),
            previous_ms: Some(previous),
            applied_ms: Some(ms),
            message: format!("Done: the Sync Offset of {input} in OBS is now {ms} ms. Press Check again to confirm it worked."),
        }
    }

    /// Put the Sync Offset back to what it was before `apply_sync_fix`.
    pub async fn undo_sync_fix(&self) -> SyncApplyResult {
        let fail = |m: &str| SyncApplyResult { ok: false, input: None, previous_ms: None, applied_ms: None, message: m.into() };
        let Some((input, prev)) = self.inner.lock().unwrap().sync_prev.clone() else {
            return SyncApplyResult { ok: true, input: None, previous_ms: None, applied_ms: None, message: "Nothing to undo.".into() };
        };
        let Some(o) = self.obs().await else { return fail("We can't reach OBS to undo it. Open Advanced Audio Properties in OBS and set the Sync Offset back by hand.") };
        match o.request("SetInputAudioSyncOffset", json!({ "inputName": input, "inputAudioSyncOffset": prev })).await {
            Ok(_) => {
                self.inner.lock().unwrap().sync_prev = None;
                SyncApplyResult { ok: true, input: Some(input), previous_ms: None, applied_ms: Some(prev), message: format!("Put back to {prev} ms.") }
            }
            Err(_) => fail("OBS wouldn't change it. Open Advanced Audio Properties in OBS and set the Sync Offset back by hand."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// Mic with claps at `claps` (seconds) and frames where the hands "move"
    /// at the same times plus `lag_s` (positive: picture later than sound).
    fn scene(claps: &[f64], lag_s: f64) -> (Vec<f32>, Vec<Frame>) {
        let secs = claps.last().unwrap() + 1.5;
        let mut mic = vec![0.0f32; (secs * f64::from(RATE)) as usize];
        for &c in claps {
            let s = (c * f64::from(RATE)) as usize;
            for i in 0..600 {
                mic[s + i] = 0.8 * (1.0 - i as f32 / 600.0);
            }
        }
        let fps = 30.0;
        let n = (secs * fps) as usize;
        let frames = (0..n)
            .map(|k| {
                let t = k as f64 / fps;
                // Hands visible for 120 ms after each clap, in the picture's own time.
                let on = claps.iter().any(|c| t >= c + lag_s && t < c + lag_s + 0.12);
                let v = if on { 220 } else { 40 };
                Frame { width: 40, height: 30, rgb: vec![v; 40 * 30 * 3], timestamp_s: t }
            })
            .collect();
        (mic, frames)
    }

    const CLAPS: [f64; 5] = [0.8, 1.7, 2.3, 3.6, 4.5];

    #[test]
    fn late_picture_means_delay_the_sound_and_offers_auto() {
        let (mic, frames) = scene(&CLAPS, 0.2);
        let r = build_report(&mic, RATE, &frames, 5, 0, 0, true, Some("Mic/Aux".into()), Some("Cam Link".into()), true);
        assert_eq!(r.verdict, SyncVerdict::SoundEarly, "{r:?}");
        let fix = r.fix.unwrap();
        assert_eq!(fix.side, SyncSide::DelaySound);
        assert!(fix.can_auto && fix.obs_input.as_deref() == Some("Mic/Aux"));
        assert!((150..=250).contains(&fix.ms), "{}", fix.ms);
        assert!(r.finding.is_some() && !r.coarse);
    }

    #[test]
    fn late_sound_means_delay_the_camera_by_hand() {
        let (mic, frames) = scene(&CLAPS, -0.25);
        let r = build_report(&mic, RATE, &frames, 5, 0, 0, false, Some("Mic/Aux".into()), Some("C920".into()), true);
        assert_eq!(r.verdict, SyncVerdict::SoundLate, "{r:?}");
        let fix = r.fix.unwrap();
        assert_eq!(fix.side, SyncSide::DelayCamera);
        assert!(!fix.can_auto && fix.steps.iter().any(|s| s.contains("Render Delay")));
        // An existing Render Delay is built on, not replaced.
        let (mic, frames) = scene(&CLAPS, -0.25);
        let r2 = build_report(&mic, RATE, &frames, 5, 0, 100, false, None, None, false);
        assert_eq!(r2.fix.unwrap().ms, 100 + fix.change_ms);
    }

    #[test]
    fn existing_obs_offset_is_counted_so_check_again_can_pass() {
        // Picture is 200 ms late and OBS already delays the mic by 200 ms: in sync.
        let (mic, frames) = scene(&CLAPS, 0.2);
        let r = build_report(&mic, RATE, &frames, 5, 200, 0, true, Some("Mic/Aux".into()), None, true);
        assert_eq!(r.verdict, SyncVerdict::InSync, "{r:?}");
        assert!(r.fix.is_none() && r.finding.is_none());
        // Halfway there: the fix builds on what is set.
        let r = build_report(&mic, RATE, &frames, 5, 100, 0, true, Some("Mic/Aux".into()), None, true);
        let fix = r.fix.unwrap();
        assert!((190..=210).contains(&fix.ms), "{}", fix.ms);
    }

    #[test]
    fn too_few_claps_or_no_motion_is_unclear_with_a_plain_hint() {
        let (mic, frames) = scene(&CLAPS, 0.0);
        let flat: Vec<Frame> = frames.iter().map(|f| Frame { rgb: vec![90; f.rgb.len()], ..f.clone() }).collect();
        let r = build_report(&mic, RATE, &flat, 5, 0, 0, false, None, None, false);
        assert_eq!(r.verdict, SyncVerdict::Unclear);
        assert!(r.hint.unwrap().contains("couldn't match"));
        let r = build_report(&mic, RATE, &frames, 1, 0, 0, false, None, None, false);
        // Measured fine even though the live meter missed claps.
        assert_ne!(r.verdict, SyncVerdict::Unclear);
        let r = build_report(&[0.0; 4800], RATE, &frames, 1, 0, 0, false, None, None, false);
        assert!(r.hint.unwrap().contains("only heard 1 clap."));
    }

    #[test]
    fn slow_cameras_are_marked_rough() {
        let (mic, frames) = scene(&CLAPS, 0.0);
        let slow: Vec<Frame> = frames.into_iter().step_by(6).collect();
        let r = build_report(&mic, RATE, &slow, 5, 0, 0, false, None, None, false);
        assert!(r.coarse);
    }

    #[tokio::test]
    async fn auto_fix_sets_the_obs_sync_offset_and_can_be_undone() {
        use crate::obs::mock::FakeObs;
        use crate::platform::mock::MockPlatform;
        let dir = tempfile::tempdir().unwrap();
        let mock = MockPlatform::demo();
        let core = Arc::new(AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default())));
        assert!(!core.apply_sync_fix().await.ok, "nothing to apply before a test");
        let obs = Arc::new(FakeObs::typical());
        core.set_obs(obs.clone()).await;
        let input = obs.state.lock().unwrap().inputs.iter().find(|i| i.kind.contains("input_capture") || i.kind.contains("wasapi")).map(|i| i.name.clone()).unwrap();
        obs.request("SetInputAudioSyncOffset", json!({ "inputName": input, "inputAudioSyncOffset": 40 })).await.unwrap();
        let fix = SyncFix { side: SyncSide::DelaySound, ms: 240, change_ms: 200, steps: vec![], can_auto: true, obs_input: Some(input.clone()), camera_source: None };
        core.inner.lock().unwrap().sync_fix = Some(fix);
        let r = core.apply_sync_fix().await;
        assert!(r.ok, "{r:?}");
        assert_eq!((r.previous_ms, r.applied_ms), (Some(40), Some(240)));
        let now = obs.request("GetInputAudioSyncOffset", json!({ "inputName": input })).await.unwrap();
        assert_eq!(now["inputAudioSyncOffset"], 240);
        assert!(core.undo_sync_fix().await.ok);
        let now = obs.request("GetInputAudioSyncOffset", json!({ "inputName": input })).await.unwrap();
        assert_eq!(now["inputAudioSyncOffset"], 40);
        // A hand-only fix is never applied.
        core.inner.lock().unwrap().sync_fix = Some(SyncFix { side: SyncSide::DelayCamera, ms: 200, change_ms: 200, steps: vec![], can_auto: false, obs_input: None, camera_source: None });
        assert!(!core.apply_sync_fix().await.ok);
    }
}