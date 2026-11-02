//! Input types for camera analysis. Everything is plain data handed in by the
//! Windows app: frames from the guided test, the current camera (UVC) state,
//! the mains frequency and the kind of camera.

use serde::{Deserialize, Serialize};

/// One 8-bit sRGB frame, interleaved RGB, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` bytes, R G B per pixel.
    pub rgb: Vec<u8>,
    /// Capture timestamp in seconds (any monotonic origin).
    pub timestamp_s: f64,
}

impl Frame {
    /// A frame filled with one colour (handy for tests and placeholders).
    pub fn solid(width: usize, height: usize, rgb: [u8; 3], timestamp_s: f64) -> Self {
        let mut data = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            data.extend_from_slice(&rgb);
        }
        Frame { width, height, rgb: data, timestamp_s }
    }

    #[inline]
    pub fn px(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    pub fn full_rect(&self) -> Rect {
        Rect { x: 0, y: 0, w: self.width, h: self.height }
    }

    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.rgb.len() == self.width * self.height * 3
    }
}

/// Axis-aligned pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Rect { x, y, w, h }
    }

    #[inline]
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn bottom(&self) -> usize {
        self.y + self.h
    }

    pub fn right(&self) -> usize {
        self.x + self.w
    }

    /// Clip to a frame of the given size.
    pub fn clamp_to(&self, width: usize, height: usize) -> Rect {
        let x = self.x.min(width);
        let y = self.y.min(height);
        let w = self.w.min(width - x);
        let h = self.h.min(height - y);
        Rect { x, y, w, h }
    }

    /// Sub-rectangle given as fractions of this rect (0..1).
    pub fn frac(&self, fx0: f64, fy0: f64, fx1: f64, fy1: f64) -> Rect {
        let x0 = self.x as f64 + self.w as f64 * fx0;
        let y0 = self.y as f64 + self.h as f64 * fy0;
        let x1 = self.x as f64 + self.w as f64 * fx1;
        let y1 = self.y as f64 + self.h as f64 * fy1;
        Rect {
            x: x0.round() as usize,
            y: y0.round() as usize,
            w: (x1 - x0).round().max(1.0) as usize,
            h: (y1 - y0).round().max(1.0) as usize,
        }
    }

    /// Grow horizontally by `fx` of the width on each side (and vertically by `fy`).
    pub fn expand(&self, fx: f64, fy: f64, width: usize, height: usize) -> Rect {
        let dx = (self.w as f64 * fx).round() as usize;
        let dy = (self.h as f64 * fy).round() as usize;
        let x = self.x.saturating_sub(dx);
        let y = self.y.saturating_sub(dy);
        Rect { x, y, w: self.w + 2 * dx, h: self.h + 2 * dy }.clamp_to(width, height)
    }

    /// True if (x, y) lies inside the ellipse inscribed in this rect, scaled by `scale`.
    #[inline]
    pub fn in_ellipse(&self, x: usize, y: usize, scale: f64) -> bool {
        let cx = self.x as f64 + self.w as f64 / 2.0;
        let cy = self.y as f64 + self.h as f64 / 2.0;
        let rx = self.w as f64 / 2.0 * scale;
        let ry = self.h as f64 / 2.0 * scale;
        if rx <= 0.0 || ry <= 0.0 {
            return false;
        }
        let dx = (x as f64 + 0.5 - cx) / rx;
        let dy = (y as f64 + 0.5 - cy) / ry;
        dx * dx + dy * dy <= 1.0
    }
}

/// Steps of the ~30 s guided camera test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStep {
    /// Sit normally, look at the camera.
    Neutral,
    /// Turn your head left and right.
    TurnHead,
    /// Hold a white sheet of paper next to your face.
    HoldPaper,
    /// Lean in towards the camera and back out.
    LeanInOut,
}

/// A frame from the guided test plus the regions the app found in it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaggedFrame {
    pub step: TestStep,
    pub frame: Frame,
    /// Face box from Windows' built-in face detector (used only to meter).
    pub face: Option<Rect>,
    /// White paper box (found by the app or confirmed by the user).
    pub paper: Option<Rect>,
    /// Head pitch in degrees if the detector provides it (positive = camera
    /// looks up at the face from below).
    pub face_pitch_deg: Option<f64>,
}

impl TaggedFrame {
    pub fn new(step: TestStep, frame: Frame) -> Self {
        TaggedFrame { step, frame, face: None, paper: None, face_pitch_deg: None }
    }
    pub fn with_face(mut self, r: Rect) -> Self {
        self.face = Some(r);
        self
    }
    pub fn with_paper(mut self, r: Rect) -> Self {
        self.paper = Some(r);
        self
    }
}

/// All frames captured during the guided test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoTestCapture {
    pub frames: Vec<TaggedFrame>,
}

/// Local mains frequency (sets anti-flicker and flicker-safe shutter speeds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MainsHz {
    Hz50,
    Hz60,
}

impl MainsHz {
    pub fn hz(self) -> f64 {
        match self {
            MainsHz::Hz50 => 50.0,
            MainsHz::Hz60 => 60.0,
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            MainsHz::Hz50 => 50,
            MainsHz::Hz60 => 60,
        }
    }
}

/// What the app can control on this camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraClass {
    /// Webcam exposing the usual manual UVC controls (C920, Brio, Facecam...).
    FullUvc,
    /// Webcam with only some controls (e.g. Brio 100, many laptop cams).
    LimitedUvc,
    /// Real camera through a capture card (Cam Link + Sony a6000 etc.):
    /// UVC controls only touch the capture chip, so coach-only.
    CaptureCard,
    /// Phone-as-webcam or virtual camera: settings live in the phone/app.
    PhoneOrVirtual,
}

impl CameraClass {
    /// True if the app can set exposure/WB/focus itself.
    pub fn controllable(self) -> bool {
        matches!(self, CameraClass::FullUvc | CameraClass::LimitedUvc)
    }
}

/// Pixel format chosen for the camera in the streaming app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VideoFormat {
    Mjpeg,
    /// Uncompressed 4:2:2; limited to low fps at 1080p on many webcams.
    Yuy2,
    Nv12,
    Other,
    #[default]
    Unknown,
}

/// A control's range as reported by the driver.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ControlRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default: f64,
}

impl ControlRange {
    pub fn new(min: f64, max: f64, step: f64, default: f64) -> Self {
        ControlRange { min, max, step, default }
    }
    /// Clamp and snap a value to the range's step.
    pub fn snap(&self, v: f64) -> f64 {
        let v = v.clamp(self.min, self.max);
        if self.step > 0.0 {
            let n = ((v - self.min) / self.step).round();
            (self.min + n * self.step).clamp(self.min, self.max)
        } else {
            v
        }
    }
    /// Position of `v` in the range, 0..1.
    pub fn fraction(&self, v: f64) -> f64 {
        if self.max > self.min { ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0) } else { 0.0 }
    }
}

/// Current camera state as read through UVC / DirectShow.
///
/// `None` means "not exposed by this camera" (or unknown).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraState {
    /// Exposure as log2(seconds), the UVC/DirectShow convention (-5 = 1/32 s).
    pub exposure_log2: Option<f64>,
    pub auto_exposure: bool,
    /// "Low light compensation" / auto-exposure priority: lets the camera drop fps.
    pub ae_priority: bool,
    pub gain: Option<f64>,
    pub wb_kelvin: Option<f64>,
    pub auto_wb: bool,
    pub focus: Option<f64>,
    pub auto_focus: bool,
    pub sharpness: Option<f64>,
    pub saturation: Option<f64>,
    pub contrast: Option<f64>,
    pub backlight_comp: Option<f64>,
    /// Anti-flicker setting: Some(50), Some(60), Some(0) = off, None = not exposed.
    pub powerline_hz: Option<u8>,
    pub hdr: Option<bool>,
    pub requested_fps: f64,
    pub requested_width: usize,
    pub requested_height: usize,
    pub format: VideoFormat,
    pub exposure_range: Option<ControlRange>,
    pub gain_range: Option<ControlRange>,
    pub wb_range: Option<ControlRange>,
    pub focus_range: Option<ControlRange>,
    pub sharpness_range: Option<ControlRange>,
    pub saturation_range: Option<ControlRange>,
    pub backlight_range: Option<ControlRange>,
}

impl Default for CameraState {
    /// A typical out-of-the-box webcam: everything on auto, 1080p30 MJPEG.
    fn default() -> Self {
        CameraState {
            exposure_log2: None,
            auto_exposure: true,
            ae_priority: false,
            gain: None,
            wb_kelvin: None,
            auto_wb: true,
            focus: None,
            auto_focus: true,
            sharpness: None,
            saturation: None,
            contrast: None,
            backlight_comp: None,
            powerline_hz: None,
            hdr: None,
            requested_fps: 30.0,
            requested_width: 1920,
            requested_height: 1080,
            format: VideoFormat::Mjpeg,
            exposure_range: None,
            gain_range: None,
            wb_range: None,
            focus_range: None,
            sharpness_range: None,
            saturation_range: None,
            backlight_range: None,
        }
    }
}
