//! Capture-card / HDMI chain doctor: "no signal" and menu overlays that stay
//! on the stream (REC dot, info bar, icons).
//!
//! Pure frame statistics, no image processing. Overlays are found as pixels
//! that never change over the guided test while the rest of the picture does
//! (the user moves), sit in the outer band of the frame, and look like camera
//! graphics (saturated red, or sharp white-on-dark text edges). Thresholds are
//! starting values: only synthetic scenes so far, no real HDMI frames yet.

use super::guard::NoPicture;
use super::types::{CameraClass, Frame, VideoTestCapture};
use crate::finding::{Area, Finding, FixKind, Severity};

pub const HDMI_NO_SIGNAL: &str = "camera.hdmi_no_signal";
pub const HDMI_OVERLAY: &str = "camera.hdmi_overlay";

/// Working width for the analysis grid (pixels); plenty for icons and text.
const GRID_W: usize = 160;
/// Fewest frames needed to tell "static" from "still scene".
const MIN_FRAMES: usize = 6;
/// Temporal range (luma) at or below which a pixel counts as unchanging
/// (sensor noise and compression stay below this).
const STATIC_RANGE: f64 = 8.0;
/// Temporal range above which a pixel counts as moving.
const MOVING_RANGE: f64 = 25.0;
/// Share of the middle of the picture that must move, else the scene is
/// too still to say anything is stuck on it.
const MIN_MOVING_SHARE: f64 = 0.04;
/// Outer band (share of width / height) where camera graphics live.
const BORDER: f64 = 0.22;
/// Red pixels needed for a REC dot (at 160 px width) and the most it may be.
const RED_MIN: usize = 5;
const RED_MAX_SHARE: f64 = 0.01;
/// Sharp static white-on-dark edge pixels that make text or icons.
const TEXT_EDGE_MIN: usize = 30;

/// What kind of graphic is stuck on the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    /// Red recording dot.
    RecDot,
    /// Text, battery, focus or menu icons.
    InfoDisplay,
}

/// Overlays found on a capture-card picture (empty = clean).
pub fn static_overlays(capture: &VideoTestCapture) -> Vec<OverlayKind> {
    let frames: Vec<&Frame> = capture.frames.iter().map(|t| &t.frame).filter(|f| f.is_valid()).collect();
    if frames.len() < MIN_FRAMES {
        return Vec::new();
    }
    let (w0, h0) = (frames[0].width, frames[0].height);
    if frames.iter().any(|f| f.width != w0 || f.height != h0) || w0 < 32 || h0 < 32 {
        return Vec::new();
    }
    let gw = GRID_W.min(w0);
    let gh = (h0 * gw / w0).max(1);
    let grid: Vec<Vec<[u8; 3]>> = frames
        .iter()
        .map(|f| {
            (0..gw * gh)
                .map(|i| {
                    let (x, y) = (i % gw * w0 / gw, i / gw * h0 / gh);
                    f.px(x, y)
                })
                .collect()
        })
        .collect();
    let luma = |p: [u8; 3]| (p[0] as f64 * 0.299 + p[1] as f64 * 0.587 + p[2] as f64 * 0.114).round();
    // Per-pixel temporal range of luma and of the red-ness.
    let mut range = vec![0f64; gw * gh];
    for (i, r) in range.iter_mut().enumerate() {
        let (mut lo, mut hi) = (255f64, 0f64);
        for g in &grid {
            let l = luma(g[i]);
            lo = lo.min(l);
            hi = hi.max(l);
        }
        *r = hi - lo;
    }
    let in_border = |i: usize| {
        let (x, y) = ((i % gw) as f64 / gw as f64, (i / gw) as f64 / gh as f64);
        x < BORDER || x > 1.0 - BORDER || y < BORDER || y > 1.0 - BORDER
    };
    // The middle must move, otherwise "static" proves nothing.
    let mid: Vec<usize> = (0..gw * gh).filter(|&i| !in_border(i)).collect();
    let moving = mid.iter().filter(|&&i| range[i] > MOVING_RANGE).count();
    if mid.is_empty() || (moving as f64) < mid.len() as f64 * MIN_MOVING_SHARE {
        return Vec::new();
    }
    let ref_frame = &grid[grid.len() / 2];
    let mut out = Vec::new();

    let red = |p: [u8; 3]| p[0] >= 170 && p[1] <= 70 && p[2] <= 70;
    let red_px = (0..gw * gh).filter(|&i| in_border(i) && range[i] <= STATIC_RANGE && grid.iter().all(|g| red(g[i]))).count();
    if red_px >= RED_MIN && (red_px as f64) <= (gw * gh) as f64 * RED_MAX_SHARE {
        out.push(OverlayKind::RecDot);
    }

    // White text edges: a static near-white pixel with a static dark pixel two
    // pixels away. Flat white walls have no such pairs.
    let mut edge_px = 0usize;
    for i in 0..gw * gh {
        if !in_border(i) || range[i] > STATIC_RANGE || luma(ref_frame[i]) < 235.0 {
            continue;
        }
        let (x, y) = (i % gw, i / gw);
        let mut hit = false;
        for (dx, dy) in [(2i32, 0i32), (-2, 0), (0, 2), (0, -2)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                continue;
            }
            let j = ny as usize * gw + nx as usize;
            if range[j] <= STATIC_RANGE && luma(ref_frame[j]) <= 60.0 {
                hit = true;
                break;
            }
        }
        if hit {
            edge_px += 1;
        }
    }
    if edge_px >= TEXT_EDGE_MIN {
        out.push(OverlayKind::InfoDisplay);
    }
    out
}

/// "Nothing from the camera" through a capture card: black or flat frames
/// while the card itself is present mean the camera sends no HDMI picture.
pub fn no_signal_finding(reason: NoPicture) -> Finding {
    let mut f = Finding::new(
        HDMI_NO_SIGNAL,
        Area::Camera,
        Severity::Critical,
        "Your capture card isn't getting a picture from the camera",
        match reason {
            NoPicture::Blank => "The card shows a flat colour, which is what it does when nothing arrives over the HDMI cable.",
            _ => "The card is there but the picture is black. The camera is not sending video over HDMI.",
        },
        "Check these in order: the camera is on and in shooting mode (not playback), the HDMI cable is pushed into the camera's HDMI OUT and the card's IN, try another HDMI cable or port, and turn off the camera's auto power-off and screen sleep. In the camera's menu set the HDMI output to 1080p (and NTSC, if it asks) when it has the choice. If you have a TV nearby, plug the camera in there to see whether the camera or the card is the problem.",
        FixKind::Manual,
    );
    f.evidence.insert("no_picture_flat".into(), if reason == NoPicture::Blank { 1.0 } else { 0.0 });
    f
}

/// Findings from the frames of a capture-card camera. Webcams and phones
/// never get these (their overlays are not HDMI menus).
pub fn findings(capture: &VideoTestCapture, class: CameraClass) -> Vec<Finding> {
    if class != CameraClass::CaptureCard {
        return Vec::new();
    }
    let found = static_overlays(capture);
    if found.is_empty() {
        return Vec::new();
    }
    let what = match (found.contains(&OverlayKind::RecDot), found.contains(&OverlayKind::InfoDisplay)) {
        (true, true) => "a red recording dot and camera text",
        (true, false) => "a red recording dot",
        _ => "camera text or icons",
    };
    vec![Finding::new(
        HDMI_OVERLAY,
        Area::Camera,
        Severity::Major,
        "Your camera's menus are showing on your stream",
        &format!("Something on the edge of your picture never moves while you do: it looks like {what}. That is the camera's own display coming through the HDMI cable."),
        "In the camera's menu turn HDMI info display off (on many cameras the Display or Info button also cycles it). If the dot or text stays, your camera may not give a clean HDMI picture; check its model in the camera guide.",
        FixKind::Manual,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::types::{TaggedFrame, TestStep};

    fn noise(i: usize) -> i32 {
        let h = (i as u32).wrapping_mul(2654435761).rotate_left(13) ^ 0x9e37_79b9;
        (h % 5) as i32 - 2
    }

    /// 160x90 scene: a face-sized blob that moves with `t`, plus `overlay` on top.
    fn scene(t: usize, overlay: impl Fn(usize, usize) -> Option<[u8; 3]>) -> TaggedFrame {
        let (w, h) = (160usize, 90usize);
        let cx = 50 + (t * 9) % 60;
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let dx = x as i32 - cx as i32;
                let dy = y as i32 - 45;
                let base = if dx * dx + dy * dy < 20 * 20 { [200, 150, 125] } else { [110, 112, 115] };
                let n = noise(y * w + x);
                let mut p = [(base[0] + n) as u8, (base[1] + n) as u8, (base[2] + n) as u8];
                if let Some(o) = overlay(x, y) {
                    p = o;
                }
                rgb.extend_from_slice(&p);
            }
        }
        TaggedFrame::new(TestStep::TurnHead, Frame { width: w, height: h, rgb, timestamp_s: t as f64 * 0.5 })
    }
    fn cap(overlay: impl Fn(usize, usize) -> Option<[u8; 3]> + Copy) -> VideoTestCapture {
        VideoTestCapture { frames: (0..10).map(|t| scene(t, overlay)).collect() }
    }

    #[test]
    fn clean_picture_has_no_overlay() {
        assert!(static_overlays(&cap(|_, _| None)).is_empty());
    }

    #[test]
    fn red_rec_dot_in_a_corner_is_found() {
        let c = cap(|x, y| ((8..13).contains(&x) && (6..11).contains(&y)).then_some([220, 20, 20]));
        assert_eq!(static_overlays(&c), vec![OverlayKind::RecDot]);
        let f = findings(&c, CameraClass::CaptureCard);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, HDMI_OVERLAY);
        // Webcams never get it.
        assert!(findings(&c, CameraClass::FullUvc).is_empty());
    }

    #[test]
    fn white_text_on_a_dark_chip_is_found_but_a_white_wall_is_not() {
        // Info bar along the bottom: dark chip with white strokes every 3rd column.
        let text = cap(|x, y| {
            if (78..86).contains(&y) && (10..70).contains(&x) {
                Some(if x % 4 < 2 { [250, 250, 250] } else { [20, 20, 20] })
            } else {
                None
            }
        });
        assert_eq!(static_overlays(&text), vec![OverlayKind::InfoDisplay]);
        // Bright static wall in the corner: no dark partner, no finding.
        let wall = cap(|x, y| (x < 30 && y < 25).then_some([245, 245, 245]));
        assert!(static_overlays(&wall).is_empty());
    }

    #[test]
    fn a_dark_pillarbox_is_not_an_overlay() {
        let bars = cap(|x, _| (x < 20 || x >= 140).then_some([8, 8, 8]));
        assert!(static_overlays(&bars).is_empty());
    }

    #[test]
    fn a_still_scene_proves_nothing() {
        // Nothing moves, so we cannot say the red dot is stuck on top.
        let frames = (0..10)
            .map(|_| scene(0, |x, y| ((8..13).contains(&x) && (6..11).contains(&y)).then_some([220, 20, 20])))
            .collect();
        assert!(static_overlays(&VideoTestCapture { frames }).is_empty());
        assert!(static_overlays(&VideoTestCapture::default()).is_empty());
    }

    #[test]
    fn no_signal_text_is_plain_and_lists_camera_menu_fixes() {
        for r in [NoPicture::Black, NoPicture::Blank] {
            let f = no_signal_finding(r);
            assert_eq!(f.id, HDMI_NO_SIGNAL);
            assert!(f.fix.contains("auto power-off") && f.fix.contains("HDMI"));
            assert!(f.can_decline);
        }
    }
}
