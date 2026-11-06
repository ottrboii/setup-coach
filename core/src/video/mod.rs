//! Camera analysis and camera settings recommender.
//!
//! Pure computation on frames handed in by the Windows app. Face boxes come
//! from Windows' built-in face detector and are used only to meter; nothing
//! here alters the image. Philosophy: fix light and position first, then lock
//! plain manual settings (exposure, gain, white balance in Kelvin, focus,
//! anti-flicker, sharpness, saturation, format).
//!
//! Exposure and white balance are metered on a white sheet of paper the user
//! holds next to their face, so the result is right for every skin tone: the
//! face is never pushed towards a lighter-skin brightness target.

pub mod findings;
pub mod guard;
pub mod hdmi;
pub mod image;
pub mod metrics;
pub mod recommend;
pub mod types;

#[cfg(test)]
mod tests;

pub use findings::findings;
pub use guard::{CaptureCheck, FrameStats, NoPicture, check_capture, check_frame, frame_stats, live_frames_only};
pub use metrics::{ColourCast, FlickerInfo, RangeHint, VideoMetrics, measure};
pub use recommend::{
    CameraSettings, FormatChoice, UvcSettings, best_focus, capture_card_steps, flicker_safe_shutters,
    recommend_settings, settings_drift,
};
pub use types::{
    CameraClass, CameraState, ControlRange, Frame, MainsHz, Rect, TaggedFrame, TestStep, VideoFormat,
    VideoTestCapture,
};

use crate::finding::{Finding, rank};

/// Measure the guided test and list what's wrong, most important first.
pub fn analyze(
    capture: &VideoTestCapture,
    state: &CameraState,
    class: CameraClass,
    mains: MainsHz,
) -> (VideoMetrics, Vec<Finding>) {
    let m = measure(capture, state.requested_fps);
    let mut f = findings::findings(&m, state, class, mains);
    f.extend(hdmi::findings(capture, class));
    rank(&mut f);
    (m, f)
}

/// Like [`analyze`], but refuses captures without a real picture (black or
/// flat frames from a camera another app holds): no metrics, no findings.
/// Frames that are individually unusable are dropped before measuring.
pub fn analyze_live(
    capture: &VideoTestCapture,
    state: &CameraState,
    class: CameraClass,
    mains: MainsHz,
) -> Result<(VideoMetrics, Vec<Finding>, VideoTestCapture), NoPicture> {
    let live = live_frames_only(capture)?;
    let (m, f) = analyze(&live, state, class, mains);
    Ok((m, f, live))
}