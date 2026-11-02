//! Webcams: Media Foundation enumeration and frame grabs, UVC controls through
//! the DirectShow `IAMCameraControl` / `IAMVideoProcAmp` interfaces exposed by
//! the MF capture source, and WinRT face detection (metering only).

use super::audio::pwstr_take;
use super::com_init;
use crate::platform::{parse_usb_ids, CameraControl, CameraDevice};
use coach_core::video::{CameraState, ControlRange, Frame, Rect, UvcSettings, VideoFormat};
use std::sync::Mutex;
use windows::core::{Interface, PWSTR};
use windows::Win32::Media::DirectShow::{IAMCameraControl, IAMVideoProcAmp};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

// KSPROPERTY ids (the DirectShow interfaces pass them straight to the driver).
const CC_EXPOSURE: i32 = 4;
const CC_FOCUS: i32 = 6;
const CC_AE_PRIORITY: i32 = 19;
const VP_CONTRAST: i32 = 1;
const VP_SATURATION: i32 = 3;
const VP_SHARPNESS: i32 = 4;
const VP_WHITEBALANCE: i32 = 7;
const VP_BACKLIGHT: i32 = 8;
const VP_GAIN: i32 = 9;
const VP_POWERLINE: i32 = 13;
const FLAG_AUTO: i32 = 1;
const FLAG_MANUAL: i32 = 2;

pub struct WinCamera {
    /// Lazily created WinRT face detector.
    detector: Mutex<Option<windows::Media::FaceAnalysis::FaceDetector>>,
}

impl WinCamera {
    pub fn new() -> Self {
        WinCamera { detector: Mutex::new(None) }
    }
}

fn mf_start() {
    com_init();
    unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
    }
}

/// (activate, name, symbolic link) for each video capture device.
fn devices() -> Vec<(IMFActivate, String, String)> {
    mf_start();
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        if MFCreateAttributes(&mut attrs, 1).is_err() {
            return Vec::new();
        }
        let Some(attrs) = attrs else { return Vec::new() };
        if attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID).is_err() {
            return Vec::new();
        }
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        if MFEnumDeviceSources(&attrs, &mut arr, &mut n).is_err() || arr.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..n as usize {
            if let Some(a) = (*arr.add(i)).take() {
                let name = alloc_string(&a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).unwrap_or_else(|| "Camera".into());
                let link = alloc_string(&a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK).unwrap_or_default();
                out.push((a, name, link));
            }
        }
        CoTaskMemFree(Some(arr as _));
        out
    }
}

fn alloc_string(a: &IMFActivate, key: &windows::core::GUID) -> Option<String> {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0u32;
        a.GetAllocatedString(key, &mut p, &mut len).ok()?;
        Some(pwstr_take(p))
    }
}

fn source(id: &str) -> Result<IMFMediaSource, String> {
    let (a, _, _) = devices().into_iter().find(|d| d.2 == id).ok_or("Camera not found. Is it plugged in?")?;
    unsafe { a.ActivateObject::<IMFMediaSource>().map_err(|e| format!("Couldn't open the camera ({e}). Another app may be using it.")) }
}

fn get_cc(c: &IAMCameraControl, p: i32) -> Option<(i32, bool)> {
    let (mut v, mut f) = (0i32, 0i32);
    unsafe { c.Get(p, &mut v, &mut f).ok()? };
    Some((v, f & FLAG_AUTO != 0))
}

fn range_cc(c: &IAMCameraControl, p: i32) -> Option<ControlRange> {
    let (mut min, mut max, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
    unsafe { c.GetRange(p, &mut min, &mut max, &mut step, &mut def, &mut caps).ok()? };
    Some(ControlRange::new(min as f64, max as f64, step.max(1) as f64, def as f64))
}

fn get_vp(c: &IAMVideoProcAmp, p: i32) -> Option<(i32, bool)> {
    let (mut v, mut f) = (0i32, 0i32);
    unsafe { c.Get(p, &mut v, &mut f).ok()? };
    Some((v, f & FLAG_AUTO != 0))
}

fn range_vp(c: &IAMVideoProcAmp, p: i32) -> Option<ControlRange> {
    let (mut min, mut max, mut step, mut def, mut caps) = (0, 0, 0, 0, 0);
    unsafe { c.GetRange(p, &mut min, &mut max, &mut step, &mut def, &mut caps).ok()? };
    Some(ControlRange::new(min as f64, max as f64, step.max(1) as f64, def as f64))
}

impl CameraControl for WinCamera {
    fn list(&self) -> Vec<CameraDevice> {
        devices().into_iter().map(|(_, name, link)| CameraDevice { usb: parse_usb_ids(&link), id: link, name }).collect()
    }

    fn read_state(&self, id: &str) -> Result<CameraState, String> {
        let src = source(id)?;
        let mut s = CameraState::default();
        if let Ok(cc) = src.cast::<IAMCameraControl>() {
            if let Some((v, auto)) = get_cc(&cc, CC_EXPOSURE) {
                s.exposure_log2 = Some(v as f64);
                s.auto_exposure = auto;
            }
            s.exposure_range = range_cc(&cc, CC_EXPOSURE);
            if let Some((v, auto)) = get_cc(&cc, CC_FOCUS) {
                s.focus = Some(v as f64);
                s.auto_focus = auto;
            }
            s.focus_range = range_cc(&cc, CC_FOCUS);
            s.ae_priority = get_cc(&cc, CC_AE_PRIORITY).map(|(v, _)| v != 0).unwrap_or(false);
        }
        if let Ok(vp) = src.cast::<IAMVideoProcAmp>() {
            s.gain = get_vp(&vp, VP_GAIN).map(|v| v.0 as f64);
            s.gain_range = range_vp(&vp, VP_GAIN);
            if let Some((v, auto)) = get_vp(&vp, VP_WHITEBALANCE) {
                s.wb_kelvin = Some(v as f64);
                s.auto_wb = auto;
            }
            s.wb_range = range_vp(&vp, VP_WHITEBALANCE);
            s.sharpness = get_vp(&vp, VP_SHARPNESS).map(|v| v.0 as f64);
            s.sharpness_range = range_vp(&vp, VP_SHARPNESS);
            s.saturation = get_vp(&vp, VP_SATURATION).map(|v| v.0 as f64);
            s.saturation_range = range_vp(&vp, VP_SATURATION);
            s.contrast = get_vp(&vp, VP_CONTRAST).map(|v| v.0 as f64);
            s.backlight_comp = get_vp(&vp, VP_BACKLIGHT).map(|v| v.0 as f64);
            s.backlight_range = range_vp(&vp, VP_BACKLIGHT);
            // 1 = 50 Hz, 2 = 60 Hz (KSPROPERTY_VIDEOPROCAMP_POWERLINE_FREQUENCY).
            s.powerline_hz = get_vp(&vp, VP_POWERLINE).and_then(|(v, _)| match v {
                1 => Some(50),
                2 => Some(60),
                _ => None,
            });
        }
        // Current stream format isn't known without opening a reader; the OBS
        // source settings carry it, so leave the default (1080p30 MJPEG).
        s.format = VideoFormat::Unknown;
        unsafe {
            let _ = src.Shutdown();
        }
        Ok(s)
    }

    fn apply(&self, id: &str, u: &UvcSettings) -> Result<Vec<String>, String> {
        let src = source(id)?;
        let mut warn = Vec::new();
        let mut cc_set = |c: &IAMCameraControl, p: i32, v: i32, auto: bool, what: &str| unsafe {
            if c.Set(p, v, if auto { FLAG_AUTO } else { FLAG_MANUAL }).is_err() {
                warn.push(format!("Your camera didn't accept the {what} setting."));
            }
        };
        if let Ok(cc) = src.cast::<IAMCameraControl>() {
            if u.auto_exposure {
                let cur = get_cc(&cc, CC_EXPOSURE).map(|v| v.0).unwrap_or(0);
                cc_set(&cc, CC_EXPOSURE, cur, true, "exposure");
            } else if let Some(e) = u.exposure_log2 {
                cc_set(&cc, CC_EXPOSURE, e.round() as i32, false, "exposure");
            }
            cc_set(&cc, CC_AE_PRIORITY, u.ae_priority as i32, false, "low-light compensation");
            if u.auto_focus {
                let cur = get_cc(&cc, CC_FOCUS).map(|v| v.0).unwrap_or(0);
                cc_set(&cc, CC_FOCUS, cur, true, "focus");
            } else if let Some(f) = u.focus {
                cc_set(&cc, CC_FOCUS, f.round() as i32, false, "focus");
            }
        } else {
            warn.push("This camera doesn't let apps change exposure or focus.".into());
        }
        let mut vp_set = |c: &IAMVideoProcAmp, p: i32, v: i32, auto: bool, what: &str| unsafe {
            if c.Set(p, v, if auto { FLAG_AUTO } else { FLAG_MANUAL }).is_err() {
                warn.push(format!("Your camera didn't accept the {what} setting."));
            }
        };
        if let Ok(vp) = src.cast::<IAMVideoProcAmp>() {
            if let Some(g) = u.gain {
                vp_set(&vp, VP_GAIN, g.round() as i32, false, "gain");
            }
            if u.auto_wb {
                let cur = get_vp(&vp, VP_WHITEBALANCE).map(|v| v.0).unwrap_or(4500);
                vp_set(&vp, VP_WHITEBALANCE, cur, true, "white balance");
            } else if let Some(k) = u.wb_kelvin {
                vp_set(&vp, VP_WHITEBALANCE, k.round() as i32, false, "white balance");
            }
            if let Some(v) = u.sharpness {
                vp_set(&vp, VP_SHARPNESS, v.round() as i32, false, "sharpness");
            }
            if let Some(v) = u.saturation {
                vp_set(&vp, VP_SATURATION, v.round() as i32, false, "colour saturation");
            }
            if let Some(v) = u.backlight_comp {
                vp_set(&vp, VP_BACKLIGHT, v.round() as i32, false, "backlight compensation");
            }
            if let Some(hz) = u.powerline_hz {
                vp_set(&vp, VP_POWERLINE, if hz == 60 { 2 } else { 1 }, false, "anti-flicker");
            }
        }
        if u.hdr == Some(false) {
            warn.push("If your camera app has an HDR switch, turn it off.".into());
        }
        unsafe {
            let _ = src.Shutdown();
        }
        Ok(warn)
    }

    fn grab_frame(&self, id: &str) -> Result<Frame, String> {
        let src = source(id)?;
        let r = unsafe { grab(&src) };
        unsafe {
            let _ = src.Shutdown();
        }
        r
    }

    fn detect_face(&self, frame: &Frame) -> Option<Rect> {
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Media::FaceAnalysis::FaceDetector;
        use windows::Security::Cryptography::CryptographicBuffer;
        com_init();
        let mut det = self.detector.lock().ok()?;
        if det.is_none() {
            *det = FaceDetector::CreateAsync().ok()?.join().ok();
        }
        let d = det.as_ref()?;
        let gray: Vec<u8> = frame
            .rgb
            .chunks(3)
            .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
            .collect();
        let buf = CryptographicBuffer::CreateFromByteArray(&gray).ok()?;
        let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Gray8, frame.width as i32, frame.height as i32).ok()?;
        let faces = d.DetectFacesAsync(&bmp).ok()?.join().ok()?;
        let mut best: Option<Rect> = None;
        for f in faces {
            if let Ok(b) = f.FaceBox() {
                let r = Rect::new(b.X as usize, b.Y as usize, b.Width as usize, b.Height as usize);
                if best.is_none_or(|x| r.w * r.h > x.w * x.h) {
                    best = Some(r);
                }
            }
        }
        best
    }
}

/// Read one RGB frame through a source reader with video processing on.
unsafe fn grab(src: &IMFMediaSource) -> Result<Frame, String> {
    let err = |e: windows::core::Error| format!("Couldn't read from the camera ({e}).");
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(err)?;
    let attrs = attrs.ok_or("Couldn't read from the camera.")?;
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(err)?;
    let reader = MFCreateSourceReaderFromMediaSource(src, &attrs).map_err(err)?;
    let mt = MFCreateMediaType().map_err(err)?;
    mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
    mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(err)?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    reader.SetCurrentMediaType(stream, None, &mt).map_err(err)?;
    let cur = reader.GetCurrentMediaType(stream).map_err(err)?;
    let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(err)?;
    let (w, h) = ((size >> 32) as usize, (size & 0xffff_ffff) as usize);
    let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
    // Skip a few frames so auto exposure settles.
    for attempt in 0..30 {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample)).map_err(err)?;
        let Some(sample) = sample else { continue };
        if attempt < 5 {
            continue;
        }
        let buf = sample.ConvertToContiguousBuffer().map_err(err)?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
        let data = std::slice::from_raw_parts(ptr, len as usize);
        let mut rgb = Vec::with_capacity(w * h * 3);
        let abs = stride.unsigned_abs() as usize;
        for y in 0..h {
            let row = if stride < 0 { h - 1 - y } else { y };
            let line = &data[row * abs..(row * abs + w * 4).min(data.len())];
            for px in line.chunks(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        let _ = buf.Unlock();
        if rgb.len() == w * h * 3 {
            return Ok(Frame { width: w, height: h, rgb, timestamp_s: 0.0 });
        }
    }
    Err("The camera didn't send a picture.".into())
}
