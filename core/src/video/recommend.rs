//! Compute locked camera settings from the measurements, adapting to what
//! the user said they can't change.
//!
//! The app applies these, captures again and re-measures (verify loop), so a
//! first estimate that's slightly off is corrected on the next pass.

use super::findings::{PAPER_TARGET_HI, PAPER_TARGET_LO};
use super::metrics::{VideoMetrics, luma_to_linear};
use super::types::{CameraClass, CameraState, MainsHz, VideoFormat};
use crate::constraints::Constraint;
use crate::finding::{Area, Finding, FixKind, Severity};
use serde::{Deserialize, Serialize};

/// Assumed usable span of a webcam gain control in stops. Gain units are not
/// standardised; the verify loop corrects any error.
pub const GAIN_RANGE_STOPS: f64 = 4.0;
/// Normal gain ceiling (fraction of range) before noise becomes visible.
pub const GAIN_CAP: f64 = 0.5;
/// Gain ceiling when the user can't add light.
pub const GAIN_CAP_NO_LIGHT: f64 = 0.75;
/// Kelvin correction slope: ln(R/B) of the paper per mired of mismatch.
const LN_RB_PER_MIRED: f64 = 0.0045;

/// UVC values to write (webcams only). `None` = leave as is / not exposed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UvcSettings {
    pub auto_exposure: bool,
    pub ae_priority: bool,
    /// log2(seconds), snapped to the driver's step.
    pub exposure_log2: Option<f64>,
    /// The flicker-safe shutter we aimed for, in seconds.
    pub target_shutter_s: Option<f64>,
    /// False if the driver's exposure steps can't hit the flicker-safe value
    /// exactly: re-check for bands after applying.
    pub shutter_exact: bool,
    pub gain: Option<f64>,
    pub auto_wb: bool,
    pub wb_kelvin: Option<f64>,
    pub auto_focus: bool,
    /// Starting focus; refine with a focus sweep (`best_focus`).
    pub focus: Option<f64>,
    pub sharpness: Option<f64>,
    pub saturation: Option<f64>,
    pub backlight_comp: Option<f64>,
    pub powerline_hz: Option<u8>,
    pub hdr: Option<bool>,
}

/// Stream format to choose in OBS for the camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatChoice {
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub format: VideoFormat,
}

/// The full recommendation for one camera.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraSettings {
    pub class: CameraClass,
    /// What to write over UVC; `None` for capture cards, phones and virtual cams.
    pub uvc: Option<UvcSettings>,
    pub format: FormatChoice,
    /// The room doesn't have enough light for a clean picture at this setting.
    pub light_limited: bool,
    /// Step-by-step instructions for the user (camera menus, phone app, room).
    pub coach_steps: Vec<String>,
    /// Short notes on trade-offs made.
    pub notes: Vec<String>,
}

/// Flicker-safe shutter times (seconds) that fit in one frame, longest first.
/// Lights pulse at twice the mains frequency, so whole multiples of that
/// period average the pulses out.
pub fn flicker_safe_shutters(mains: MainsHz, fps: f64) -> Vec<f64> {
    let period = 1.0 / (2.0 * mains.hz());
    let frame = 1.0 / fps.max(1.0);
    let mut v: Vec<f64> = (1..=12).map(|k| k as f64 * period).filter(|t| *t <= frame + 1e-9).collect();
    v.reverse();
    v
}

fn has(c: &[Constraint], x: Constraint) -> bool {
    c.contains(&x)
}

/// Exposure plan: shutter, gain fraction and whether light ran out.
struct ExposurePlan {
    shutter_s: f64,
    gain_frac: f64,
    light_limited: bool,
    too_bright: bool,
}

/// Plan exposure for a given fps. `needed_ev` is log2 of the exposure time the
/// image needs at gain 0 (i.e. shutter seconds with zero gain).
fn plan_exposure(needed_ev: f64, mains: MainsHz, fps: f64, gain_cap: f64) -> ExposurePlan {
    let safe = flicker_safe_shutters(mains, fps);
    let needed_t = needed_ev.exp2();
    // Use the longest flicker-safe shutter that doesn't overshoot; put the rest into gain.
    let shutter = safe.iter().cloned().find(|t| *t <= needed_t * 1.02).unwrap_or(*safe.last().unwrap());
    let too_bright = needed_t < *safe.last().unwrap() * 0.98;
    let remaining_stops = (needed_t / shutter).log2().max(0.0);
    let gain_frac = (remaining_stops / GAIN_RANGE_STOPS).min(gain_cap);
    let light_limited = remaining_stops / GAIN_RANGE_STOPS > gain_cap + 1e-9;
    ExposurePlan { shutter_s: shutter, gain_frac, light_limited, too_bright }
}

/// Recommend locked settings.
pub fn recommend_settings(
    m: &VideoMetrics,
    s: &CameraState,
    class: CameraClass,
    mains: MainsHz,
    constraints: &[Constraint],
) -> CameraSettings {
    let mut notes = Vec::new();
    let mut coach = Vec::new();
    let no_light = has(constraints, Constraint::CantAddLight);
    let gain_cap = if no_light { GAIN_CAP_NO_LIGHT } else { GAIN_CAP };

    // --- Format / fps -------------------------------------------------------
    let mut fps = if s.requested_fps > 0.0 { s.requested_fps.min(60.0) } else { 30.0 };
    let mut format = s.format;
    if s.requested_height >= 1080 && matches!(format, VideoFormat::Yuy2 | VideoFormat::Unknown) {
        format = VideoFormat::Mjpeg;
        notes.push("Using MJPEG: uncompressed 1080p can't run smoothly over most webcam USB links.".into());
    }

    // --- Exposure -----------------------------------------------------------
    // What exposure (seconds at zero gain) would put the paper at the target?
    let current_ev = match (s.exposure_log2, s.gain, s.gain_range) {
        (Some(e), Some(g), Some(r)) => Some(e + r.fraction(g) * GAIN_RANGE_STOPS),
        (Some(e), _, _) => Some(e),
        _ => None,
    };
    let target = (PAPER_TARGET_LO + PAPER_TARGET_HI) / 2.0;
    let needed_ev = match (current_ev, m.paper_luma) {
        (Some(ev), Some(p)) => {
            let delta = if m.paper_clipped_pct.unwrap_or(0.0) > 5.0 || p >= 250.0 {
                notes.push("Your paper was blown out; starting one stop darker and re-checking.".into());
                -1.0
            } else {
                (luma_to_linear(target) / luma_to_linear(p.max(1.0))).log2()
            };
            Some(ev + delta)
        }
        (Some(ev), None) => {
            notes.push("No white paper measured: keeping the current brightness. Hold up paper for an exact value.".into());
            Some(ev)
        }
        _ => None,
    };

    let mut light_limited = false;
    let mut plan = needed_ev.map(|ev| plan_exposure(ev, mains, fps, gain_cap));
    if let (Some(ev), Some(p)) = (needed_ev, plan.as_ref()) {
        if p.light_limited && fps > 30.0 {
            // Halving the frame rate doubles the light per frame.
            fps = 30.0;
            notes.push("Not enough light for 60 fps: 30 fps gives each frame twice the light.".into());
            plan = Some(plan_exposure(ev, mains, fps, gain_cap));
        }
    }
    if let Some(p) = &plan {
        light_limited = p.light_limited;
        if p.light_limited {
            if no_light {
                notes.push("You can't add light, so we're using the slowest flicker-safe shutter and more gain. Expect a little grain.".into());
            } else {
                coach.push("Add light in front of you: face a window, or bounce a lamp off the wall behind your monitor.".into());
            }
        }
        if p.too_bright {
            coach.push("Your light is very bright for the camera: dim it or move it a little further away.".into());
        }
    }

    // --- White balance ------------------------------------------------------
    let fixed_kelvin = if s.auto_wb { None } else { s.wb_kelvin };
    let wb_kelvin = m.paper_ln_rb.map(|lnrb| {
        let base = fixed_kelvin.unwrap_or_else(|| {
            notes.push("White balance was on auto: starting from 5000 K and fine-tuning on the next check.".into());
            5000.0
        });
        // Warm paper (ln R/B > 0) means the light is warmer than the setting:
        // move the setting warmer (higher mired, lower Kelvin).
        let mired = 1.0e6 / base + lnrb / LN_RB_PER_MIRED;
        let k = (1.0e6 / mired.max(50.0)).clamp(2000.0, 10000.0);
        match s.wb_range {
            Some(r) => r.snap(k),
            None => (k / 50.0).round() * 50.0,
        }
    });
    if has(constraints, Constraint::CantFixMixedLight) {
        notes.push("Mixed light colours stay: white balance is set from the paper at your face, so your skin looks right even if the room doesn't.".into());
    }

    // --- Backlight ----------------------------------------------------------
    let backlit = m.background_vs_paper_stops.map(|b| b > 0.0).unwrap_or(false);
    if has(constraints, Constraint::CantMoveBacklight) && backlit {
        notes.push("The bright background stays: exposure is set for your face, so the background will look brighter or blown out.".into());
    }
    if has(constraints, Constraint::CantMoveCamera) {
        coach.push("If the camera can't go higher, tilt it down slightly and sit a little further back to soften the upward angle.".into());
    }

    // --- Class-specific output ---------------------------------------------
    let uvc = if class.controllable() {
        let (exposure_log2, target_shutter, exact) = match &plan {
            Some(p) => {
                let (v, exact) = snap_exposure(p.shutter_s, fps, s);
                (Some(v), Some(p.shutter_s), exact)
            }
            None => (None, None, true),
        };
        if !exact {
            notes.push("This camera's exposure steps can't hit the flicker-safe shutter exactly; we'll re-check for bands after applying.".into());
        }
        let gain = match (s.gain_range, &plan) {
            (Some(r), Some(p)) => Some(r.snap(r.min + p.gain_frac * (r.max - r.min))),
            (Some(r), None) => s.gain.map(|g| r.snap(g.min(r.min + gain_cap * (r.max - r.min)))),
            _ => None,
        };
        let backlight = s.backlight_range.map(|r| {
            if has(constraints, Constraint::CantMoveBacklight) && backlit {
                r.snap((r.min + r.max) / 2.0)
            } else {
                r.min
            }
        });
        Some(UvcSettings {
            auto_exposure: false,
            ae_priority: false,
            exposure_log2,
            target_shutter_s: target_shutter,
            shutter_exact: exact,
            gain,
            auto_wb: wb_kelvin.is_none() && s.auto_wb,
            wb_kelvin: wb_kelvin.or(fixed_kelvin),
            auto_focus: false,
            focus: s.focus.or(s.focus_range.map(|r| r.default)),
            sharpness: cap_at_default(s.sharpness, s.sharpness_range),
            saturation: cap_at_default(s.saturation, s.saturation_range),
            backlight_comp: backlight,
            powerline_hz: s.powerline_hz.map(|_| mains.as_u8()),
            hdr: s.hdr.map(|_| false),
        })
    } else {
        None
    };
    match class {
        CameraClass::CaptureCard => coach.extend(capture_card_steps(mains, fps, wb_kelvin)),
        CameraClass::PhoneOrVirtual => coach.extend(phone_steps(mains)),
        _ => {}
    }

    CameraSettings {
        class,
        uvc,
        format: FormatChoice { width: s.requested_width, height: s.requested_height, fps, format },
        light_limited,
        coach_steps: coach,
        notes,
    }
}

/// Snap a shutter time to the camera's exposure control. Returns the value
/// to write and whether it matches the target within 3%.
fn snap_exposure(shutter_s: f64, fps: f64, s: &CameraState) -> (f64, bool) {
    let want = shutter_s.log2();
    let frame_max = (1.0 / fps.max(1.0)).log2() + 1e-9;
    match s.exposure_range {
        Some(r) => {
            let v = r.snap(want);
            // Never exceed the frame time (that's what drops the fps).
            let v = if v > frame_max { r.snap(v - r.step.max(1e-6)).min(frame_max) } else { v };
            let exact = ((v.exp2() - shutter_s) / shutter_s).abs() < 0.03;
            (v, exact)
        }
        None => (want, true),
    }
}

/// Sharpness/saturation: start at default and only go down.
fn cap_at_default(v: Option<f64>, r: Option<super::types::ControlRange>) -> Option<f64> {
    match (v, r) {
        (Some(v), Some(r)) => Some(r.snap(v.min(r.default))),
        (None, Some(r)) => Some(r.default),
        (v, None) => v,
    }
}

/// Coach-only steps for a real camera through a capture card (Sony a6000
/// class and similar). The UVC controls of a capture card only touch its
/// chip, so these are changed in the camera's own menu.
pub fn capture_card_steps(mains: MainsHz, fps: f64, wb_kelvin: Option<f64>) -> Vec<String> {
    let shutter = match (mains, fps > 30.0) {
        (MainsHz::Hz50, false) => "1/50",
        (MainsHz::Hz50, true) => "1/100",
        (MainsHz::Hz60, false) => "1/60",
        (MainsHz::Hz60, true) => "1/120",
    };
    let wb = wb_kelvin
        .map(|k| format!("Set white balance to {:.0} K (or custom white balance on your white paper), not auto.", k))
        .unwrap_or_else(|| "Set white balance to a fixed Kelvin value (or custom white balance on your white paper), not auto.".into());
    vec![
        "Put the camera in movie mode (or manual exposure, M).".into(),
        format!("Set the shutter to {shutter} s: flicker-safe for your {} Hz lights.", mains.as_u8()),
        "Use the lowest ISO that still gives a bright picture (keep it under about 1000); add light before raising ISO.".into(),
        "Open the aperture (smallest f-number) for more light and a softer background.".into(),
        wb,
        "Turn off HDMI info display so menus and focus boxes don't show on stream (clean HDMI).".into(),
        "Set auto power-off to off/maximum and the auto power-off temperature to High, so the camera doesn't shut down mid-stream.".into(),
        "Run it from a dummy battery on mains power; batteries won't last a stream.".into(),
        "Use continuous autofocus with face/eye detection (AF-C), or manual focus at your sitting distance.".into(),
        "Check the camera's HDMI output frame rate matches what you stream (NTSC for 60p); expect ~100 ms delay and set the mic sync offset in OBS.".into(),
    ]
}

fn phone_steps(mains: MainsHz) -> Vec<String> {
    vec![
        "Open the phone's webcam app (Camo, DroidCam, Iriun) and use the rear camera for better quality.".into(),
        "Tap your face and lock focus and exposure (AE/AF lock).".into(),
        "Lock white balance while pointing at your white paper.".into(),
        format!("Set anti-banding/anti-flicker to {} Hz if the app has it.", mains.as_u8()),
        "Keep the phone on a charger and in a mount at eye level.".into(),
    ]
}

/// Pick the sharpest focus value from a sweep of (focus value, face
/// sharpness) samples, refined with a parabola through the peak.
pub fn best_focus(samples: &[(f64, f64)]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut v: Vec<(f64, f64)> = samples.to_vec();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (i, _) = v
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.1.partial_cmp(&b.1.1).unwrap())?;
    if i == 0 || i + 1 >= v.len() {
        return Some(v[i].0);
    }
    let (x0, y0) = v[i - 1];
    let (x1, y1) = v[i];
    let (x2, y2) = v[i + 1];
    let denom = (x0 - x1) * (x0 - x2) * (x1 - x2);
    if denom.abs() < 1e-12 {
        return Some(x1);
    }
    let a = (x2 * (y1 - y0) + x1 * (y0 - y2) + x0 * (y2 - y1)) / denom;
    let b = (x2 * x2 * (y0 - y1) + x1 * x1 * (y2 - y0) + x0 * x0 * (y1 - y2)) / denom;
    if a >= 0.0 {
        return Some(x1);
    }
    Some((-b / (2.0 * a)).clamp(x0, x2))
}

/// Compare the camera's live state with what we applied. Webcams often
/// forget settings after a replug/reboot, or another app overwrites them.
pub fn settings_drift(current: &CameraState, applied: &UvcSettings) -> Option<Finding> {
    let mut changed: Vec<&str> = Vec::new();
    if current.auto_exposure != applied.auto_exposure {
        changed.push("auto_exposure");
    }
    if current.ae_priority != applied.ae_priority {
        changed.push("low_light_compensation");
    }
    if current.auto_wb != applied.auto_wb {
        changed.push("auto_white_balance");
    }
    if current.auto_focus != applied.auto_focus {
        changed.push("auto_focus");
    }
    let differs = |a: Option<f64>, b: Option<f64>, tol: f64| matches!((a, b), (Some(x), Some(y)) if (x - y).abs() > tol);
    if differs(current.exposure_log2, applied.exposure_log2, 0.05) {
        changed.push("exposure");
    }
    if differs(current.gain, applied.gain, 0.5) {
        changed.push("gain");
    }
    if differs(current.wb_kelvin, applied.wb_kelvin, 60.0) {
        changed.push("white_balance");
    }
    if differs(current.focus, applied.focus, 0.5) {
        changed.push("focus");
    }
    if let (Some(a), Some(b)) = (current.powerline_hz, applied.powerline_hz) {
        if a != b {
            changed.push("anti_flicker");
        }
    }
    if changed.is_empty() {
        return None;
    }
    let mut fd = Finding::new(
        "camera.settings_reset",
        Area::Camera,
        Severity::Major,
        "Your camera forgot its settings",
        "Webcams often lose their settings after a restart or replug, or another app (G HUB, Camera Hub, Windows) writes its own.",
        "We've put them back. If it keeps happening, close the camera's own app or let only one app control the camera.",
        FixKind::Auto,
    )
    .with_evidence("changed_controls", changed.len() as f64);
    fd.why = format!("{} Changed: {}.", fd.why, changed.join(", "));
    Some(fd)
}
