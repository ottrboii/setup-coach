//! Camera pipeline rules (Windows camera stack + OBS video capture sources).

use super::ids;
use super::state::{ColorRange, ObsVideoCapture, SystemSnapshot};
use crate::finding::{Area, Finding, FixKind, Severity};

/// Uncompressed formats that choke USB bandwidth at high resolutions.
pub fn is_uncompressed(format: &str) -> bool {
    matches!(format.to_ascii_uppercase().as_str(), "YUY2" | "YUYV" | "NV12" | "I420" | "RGB24" | "XRGB")
}

/// Uncompressed at a resolution/fps most USB webcams can't deliver smoothly.
pub fn uncompressed_too_heavy(c: &ObsVideoCapture) -> bool {
    c.res_type_custom
        && is_uncompressed(&c.video_format)
        && (c.height >= 1080 || (c.height >= 720 && c.fps.unwrap_or(30.0) >= 50.0))
}

pub(crate) fn check(s: &SystemSnapshot, out: &mut Vec<Finding>) {
    if let Some(cam) = &s.camera {
        if !cam.privacy_allowed {
            out.push(Finding::new(
                ids::CAM_PRIVACY,
                Area::Camera,
                Severity::Critical,
                "Windows is blocking apps from using your camera",
                "The Windows camera privacy setting is off for desktop apps, so OBS gets a black screen.",
                "Open Settings > Privacy > Camera and allow desktop apps. We'll open the page for you.",
                FixKind::Guided,
            ));
        }
        if !cam.frame_server_running {
            out.push(Finding::new(
                ids::CAM_FRAME_SERVER,
                Area::Camera,
                Severity::Major,
                "Your camera may show \"in use\" or stay black",
                "The Windows Camera Frame Server service isn't running, which often causes \"camera in use\" errors or a black preview.",
                "Restart the \"Windows Camera Frame Server\" service (we'll walk you through it), then reopen OBS.",
                FixKind::Guided,
            ));
        }
        if cam.studio_effects_on {
            out.push(Finding::new(
                ids::CAM_STUDIO_EFFECTS,
                Area::Camera,
                Severity::Major,
                "Windows is adding AI effects to your camera",
                "Windows Studio Effects (background blur, eye contact, auto framing) change your image behind your back and make the picture look processed.",
                "Turn Studio Effects off in Settings > Bluetooth & devices > Cameras. We'll fix the image with light and plain settings instead.",
                FixKind::Guided,
            ));
        }
        if cam.multi_app_camera_on {
            out.push(Finding::new(
                ids::CAM_MULTI_APP,
                Area::Camera,
                Severity::Minor,
                "Multi-app camera mode stops us from fixing your camera",
                "With Windows \"multi-app camera\" on, image settings can only be changed from Windows Settings, so we can't lock exposure or white balance for you.",
                "Turn multi-app camera mode off for this camera while we apply settings, or change them yourself in Windows Settings with our values.",
                FixKind::Guided,
            ));
        }
        if !cam.vendor_apps_running.is_empty() {
            out.push(Finding::new(
                ids::CAM_VENDOR_APPS,
                Area::Camera,
                Severity::Minor,
                "Another camera app keeps overwriting your settings",
                &format!(
                    "{} also controls your camera and re-applies its own settings, undoing ours.",
                    cam.vendor_apps_running.join(", ")
                ),
                "Close it, or turn off its camera control, and let one app manage the camera.",
                FixKind::Guided,
            ));
        }
    }

    let Some(obs) = &s.obs else { return };
    for c in &obs.video_captures {
        if !c.res_type_custom {
            out.push(Finding::new(
                ids::CAM_DEVICE_DEFAULT,
                Area::Camera,
                Severity::Major,
                &format!("OBS lets \"{}\" pick a poor video mode", c.name),
                "\"Device Default\" often picks a low resolution, 30 fps or an uncompressed format the camera can't keep up with.",
                "Set a fixed mode: 1920x1080, 30 fps, MJPEG (or 720p60 if your camera supports it in your light). We can set it for you.",
                FixKind::Auto,
            ));
        } else if uncompressed_too_heavy(c) {
            out.push(
                Finding::new(
                    ids::CAM_YUY2_HIGH_RES,
                    Area::Camera,
                    Severity::Major,
                    &format!("\"{}\" is choking on uncompressed video", c.name),
                    "Uncompressed video (YUY2) at this resolution is more than most USB webcams can send, so the frame rate drops or freezes.",
                    "Switch the video format to MJPEG. We can do it for you.",
                    FixKind::Auto,
                )
                .with_evidence("height", c.height as f64)
                .with_evidence("fps", c.fps.unwrap_or(0.0) as f64),
            );
        }
        if c.color_range != ColorRange::Default && c.color_range != obs.output_color_range {
            out.push(Finding::new(
                ids::CAM_COLOR_RANGE,
                Area::Camera,
                Severity::Minor,
                &format!("\"{}\" may look washed out or too contrasty", c.name),
                "The camera source's colour range doesn't match OBS's output range. A mismatch makes blacks grey (washed out) or crushes dark detail.",
                "Set the source's colour range to match the device (usually \"Default\" or \"Partial\"). We'll confirm with the picture check.",
                FixKind::Guided,
            ));
        }
    }
}
