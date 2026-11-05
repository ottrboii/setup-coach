//! "Is there a picture at all?" guard.
//!
//! A webcam can be opened by one app at a time. When OBS (or Teams, Zoom...)
//! already has it, the next app typically gets all-black frames, a frozen
//! frame, or nothing. Every number measured from such frames (colour cast,
//! flicker, brightness...) is garbage, so nothing may be analysed, no finding
//! raised and no plan made until frames pass this check.
//!
//! The check is deliberately about "is this a picture of anything" and not
//! "is this a good picture": a dim room, a lens cap with a bit of light leaking
//! in, or a very dark scene with a face lit by the monitor all stay usable.

use super::types::{Frame, VideoTestCapture};
use serde::{Deserialize, Serialize};

/// Why a frame (or a whole capture) cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoPicture {
    /// Essentially all black (camera busy, covered, or sending nothing).
    Black,
    /// One flat colour with no detail (busy camera, stalled stream, test card).
    Blank,
    /// No frames arrived at all.
    NoFrames,
}

/// Mean luma (0..255) below which a frame must also be dim at its brightest
/// pixels to count as black.
const BLACK_MEAN: f64 = 3.0;
/// 99th-percentile luma below which a near-black frame is "black": sensor
/// noise in the dark or MJPEG ringing reaches this, a lit face does not.
const BLACK_P99: f64 = 12.0;
/// A real sensor always has noise and detail; a flat frame has a standard
/// deviation of luma below this (0..255 scale).
const FLAT_STD: f64 = 0.8;
/// Share of frames that must be usable for a capture to be analysed.
pub const MIN_USABLE_SHARE: f64 = 0.6;
/// Fewest usable frames needed to analyse a capture.
pub const MIN_USABLE_FRAMES: usize = 3;

/// Luma statistics used by the check (also handy for the Advanced numbers).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub mean: f64,
    pub std_dev: f64,
    pub p99: f64,
}

/// Luma mean, standard deviation and 99th percentile of a frame (sampled on
/// a coarse grid; plenty for this decision and cheap at 640 px).
pub fn frame_stats(frame: &Frame) -> Option<FrameStats> {
    if !frame.is_valid() {
        return None;
    }
    let step = ((frame.width * frame.height) / 20_000).max(1);
    let mut hist = [0u32; 256];
    let (mut n, mut sum, mut sum2) = (0u64, 0f64, 0f64);
    let mut i = 0usize;
    let total = frame.width * frame.height;
    while i < total {
        let p = &frame.rgb[i * 3..i * 3 + 3];
        let y = (p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8;
        hist[y as usize] += 1;
        let yf = y as f64;
        sum += yf;
        sum2 += yf * yf;
        n += 1;
        i += step;
    }
    if n == 0 {
        return None;
    }
    let mean = sum / n as f64;
    let var = (sum2 / n as f64 - mean * mean).max(0.0);
    let target = (n as f64 * 0.99).ceil() as u64;
    let (mut acc, mut p99) = (0u64, 255.0);
    for (v, c) in hist.iter().enumerate() {
        acc += *c as u64;
        if acc >= target {
            p99 = v as f64;
            break;
        }
    }
    Some(FrameStats { mean, std_dev: var.sqrt(), p99 })
}

/// `None` = a usable picture. `Some(reason)` = do not analyse this frame.
pub fn check_frame(frame: &Frame) -> Option<NoPicture> {
    let Some(s) = frame_stats(frame) else { return Some(NoPicture::NoFrames) };
    if s.mean < BLACK_MEAN && s.p99 < BLACK_P99 {
        return Some(NoPicture::Black);
    }
    if s.std_dev < FLAT_STD {
        // A flat dark frame is still "black"; anything else flat is "blank".
        return Some(if s.mean < 8.0 { NoPicture::Black } else { NoPicture::Blank });
    }
    None
}

/// Outcome of checking a whole capture.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureCheck {
    pub total: usize,
    pub usable: usize,
    /// Most common reason among the rejected frames (None when all are fine).
    pub reason: Option<NoPicture>,
}

impl CaptureCheck {
    pub fn is_live(&self) -> bool {
        self.usable >= MIN_USABLE_FRAMES && self.usable as f64 >= self.total as f64 * MIN_USABLE_SHARE
    }
}

pub fn check_capture(capture: &VideoTestCapture) -> CaptureCheck {
    let total = capture.frames.len();
    let (mut usable, mut black, mut blank) = (0usize, 0usize, 0usize);
    for f in &capture.frames {
        match check_frame(&f.frame) {
            None => usable += 1,
            Some(NoPicture::Black) | Some(NoPicture::NoFrames) => black += 1,
            Some(NoPicture::Blank) => blank += 1,
        }
    }
    let reason = if total == 0 {
        Some(NoPicture::NoFrames)
    } else if usable == total {
        None
    } else if blank > black {
        Some(NoPicture::Blank)
    } else {
        Some(NoPicture::Black)
    };
    CaptureCheck { total, usable, reason }
}

/// The capture with unusable frames dropped, or why it can't be analysed.
/// This is the only door into the analysis: bad frames never reach it.
pub fn live_frames_only(capture: &VideoTestCapture) -> Result<VideoTestCapture, NoPicture> {
    let c = check_capture(capture);
    if !c.is_live() {
        return Err(c.reason.unwrap_or(NoPicture::NoFrames));
    }
    Ok(VideoTestCapture { frames: capture.frames.iter().filter(|f| check_frame(&f.frame).is_none()).cloned().collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::types::{TaggedFrame, TestStep};

    /// Deterministic pseudo-noise in -amp..=amp.
    fn noise(i: usize, amp: i32) -> i32 {
        let h = (i as u32).wrapping_mul(2654435761).rotate_left(13) ^ 0x9e37_79b9;
        (h % (2 * amp as u32 + 1)) as i32 - amp
    }

    fn frame_with(f: impl Fn(usize, usize) -> [f64; 3]) -> Frame {
        let (w, h) = (160usize, 120usize);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                for v in f(x, y) {
                    rgb.push(v.clamp(0.0, 255.0) as u8);
                }
            }
        }
        Frame { width: w, height: h, rgb, timestamp_s: 0.0 }
    }

    /// Wall, warm face and paper with sensor noise; `k` scales the light.
    fn scene(k: f64) -> Frame {
        frame_with(|x, y| {
            let fx = (x as f64 - 80.0) / 20.0;
            let fy = (y as f64 - 55.0) / 28.0;
            let base = if fx * fx + fy * fy <= 1.0 {
                [200.0, 150.0, 125.0]
            } else if (110..150).contains(&x) && (40..90).contains(&y) {
                [225.0, 225.0, 225.0]
            } else {
                [110.0, 112.0, 115.0]
            };
            let n = noise(y * 160 + x, 3) as f64;
            [base[0] * k + n, base[1] * k + n, base[2] * k + n]
        })
    }

    fn tagged(frame: Frame) -> TaggedFrame {
        TaggedFrame::new(TestStep::Neutral, frame)
    }

    #[test]
    fn solid_black_is_black() {
        assert_eq!(check_frame(&Frame::solid(160, 120, [0, 0, 0], 0.0)), Some(NoPicture::Black));
    }

    #[test]
    fn black_with_sensor_noise_or_mjpeg_ringing_is_still_black() {
        let f = frame_with(|x, y| {
            let n = noise(y * 160 + x, 2).abs() as f64;
            [n, n, n]
        });
        assert_eq!(check_frame(&f), Some(NoPicture::Black));
        // Slightly lifted black (e.g. 16 in limited range) with no detail.
        assert_eq!(check_frame(&Frame::solid(160, 120, [16, 16, 16], 0.0)), Some(NoPicture::Blank));
    }

    #[test]
    fn flat_colour_is_blank() {
        assert_eq!(check_frame(&Frame::solid(160, 120, [128, 128, 128], 0.0)), Some(NoPicture::Blank));
        assert_eq!(check_frame(&Frame::solid(160, 120, [20, 90, 220], 0.0)), Some(NoPicture::Blank));
    }

    #[test]
    fn normal_frame_is_usable() {
        assert_eq!(check_frame(&scene(1.0)), None);
    }

    #[test]
    fn legitimately_dark_but_valid_frame_is_usable() {
        // A dim room: everything at ~12 % light. Darker than comfortable, but a picture.
        let dark = scene(0.12);
        let s = frame_stats(&dark).unwrap();
        assert!(s.mean < 25.0 && s.mean > BLACK_MEAN, "mean {}", s.mean);
        assert_eq!(check_frame(&dark), None);
        // Very dark scene with just a monitor-lit face.
        let face_only = frame_with(|x, y| {
            let fx = (x as f64 - 80.0) / 20.0;
            let fy = (y as f64 - 55.0) / 28.0;
            let n = noise(y * 160 + x, 2) as f64;
            if fx * fx + fy * fy <= 1.0 { [60.0 + n, 45.0 + n, 38.0 + n] } else { [2.0 + n.abs(), 2.0 + n.abs(), 2.0 + n.abs()] }
        });
        assert_eq!(check_frame(&face_only), None);
    }

    #[test]
    fn invalid_frame_has_no_picture() {
        let bad = Frame { width: 10, height: 10, rgb: vec![0; 5], timestamp_s: 0.0 };
        assert_eq!(check_frame(&bad), Some(NoPicture::NoFrames));
    }

    #[test]
    fn capture_of_black_frames_is_rejected() {
        let cap = VideoTestCapture { frames: (0..20).map(|_| tagged(Frame::solid(160, 120, [0, 0, 0], 0.0))).collect() };
        let c = check_capture(&cap);
        assert!(!c.is_live());
        assert_eq!(c.reason, Some(NoPicture::Black));
        assert_eq!(live_frames_only(&cap), Err(NoPicture::Black));
    }

    #[test]
    fn empty_capture_is_rejected() {
        assert_eq!(live_frames_only(&VideoTestCapture::default()), Err(NoPicture::NoFrames));
    }

    #[test]
    fn mostly_black_capture_is_rejected_but_a_few_dropouts_are_tolerated() {
        let mut frames: Vec<_> = (0..20).map(|_| tagged(scene(1.0))).collect();
        for f in frames.iter_mut().take(3) {
            f.frame = Frame::solid(160, 120, [0, 0, 0], 0.0);
        }
        let kept = live_frames_only(&VideoTestCapture { frames: frames.clone() }).unwrap();
        assert_eq!(kept.frames.len(), 17);
        for f in frames.iter_mut().take(14) {
            f.frame = Frame::solid(160, 120, [0, 0, 0], 0.0);
        }
        assert_eq!(live_frames_only(&VideoTestCapture { frames }), Err(NoPicture::Black));
    }
}
