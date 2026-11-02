//! OS integration behind traits, so the app logic compiles and is tested on
//! any OS. `windows/` holds the real implementations; `mock` is used on other
//! systems and in tests.

use coach_core::audio::AudioBuffer;
use coach_core::system::{Action, CameraSystemState, HardwareInfo, KnownProcessingApps, NetworkInfo, WindowsAudioState};
use coach_core::video::{CameraState, Frame, MainsHz, Rect, UvcSettings};
use std::path::PathBuf;
use std::sync::Arc;

pub mod mock;
#[cfg(windows)]
pub mod windows;

/// A capture endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_default_comms: bool,
    pub channels: u16,
    pub sample_rate: u32,
    pub usb: Option<(u16, u16)>,
}

/// Called from the audio thread with interleaved f32 samples.
pub type SampleSink = Box<dyn FnMut(&[f32], u16, u32) + Send>;

/// Stops a running capture or playback when dropped or stopped.
pub trait StreamHandle: Send {
    fn stop(self: Box<Self>);
}

pub trait AudioBackend: Send + Sync {
    fn list_inputs(&self) -> Vec<AudioDevice>;
    /// Start recording `device_id` at its native format; samples go to `sink`.
    fn start_capture(&self, device_id: &str, sink: SampleSink) -> Result<Box<dyn StreamHandle>, String>;
    /// Play a buffer on the default output.
    fn play(&self, buf: AudioBuffer) -> Result<Box<dyn StreamHandle>, String>;
}

/// A software-controllable input gain (Windows endpoint level in dB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainControl {
    pub current_db: f64,
    pub min_db: f64,
    pub max_db: f64,
}

pub trait SystemInspector: Send + Sync {
    fn windows_build(&self) -> Option<String>;
    fn windows_audio(&self) -> Option<WindowsAudioState>;
    fn camera_system(&self) -> Option<CameraSystemState>;
    fn processing_apps(&self) -> KnownProcessingApps;
    /// Names of running vendor camera apps (G HUB, Camera Hub…).
    fn vendor_camera_apps(&self) -> Vec<String>;
    fn hardware(&self) -> Option<HardwareInfo>;
    fn network(&self) -> Option<NetworkInfo>;
    fn mains(&self) -> MainsHz;
    fn obs_running(&self) -> bool;
    /// `%APPDATA%\obs-studio` (or the platform equivalent).
    fn obs_config_dir(&self) -> Option<PathBuf>;
    /// Execute a Windows-side action (registry, endpoint, power settings).
    fn apply(&self, action: &Action) -> Result<(), String>;
    /// The endpoint's input level in dB (IAudioEndpointVolume), if adjustable.
    fn endpoint_gain_db(&self, _endpoint_id: &str) -> Option<GainControl> {
        None
    }
    fn set_endpoint_gain_db(&self, _endpoint_id: &str, _db: f64) -> Result<(), String> {
        Err("This mic's level can't be changed from here.".into())
    }
    /// Open a Windows Settings page, URL or app for the user.
    fn open_target(&self, target: &crate::types::OpenTarget) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct CameraDevice {
    pub id: String,
    pub name: String,
    pub usb: Option<(u16, u16)>,
}

pub trait CameraControl: Send + Sync {
    fn list(&self) -> Vec<CameraDevice>;
    fn read_state(&self, id: &str) -> Result<CameraState, String>;
    /// Write UVC values; returns warnings for controls that didn't take.
    fn apply(&self, id: &str, uvc: &UvcSettings) -> Result<Vec<String>, String>;
    /// Grab one frame directly from the camera (when no app holds it).
    fn grab_frame(&self, id: &str) -> Result<Frame, String>;
    /// Face rectangle for metering only.
    fn detect_face(&self, frame: &Frame) -> Option<Rect>;
}

#[derive(Clone)]
pub struct Platform {
    pub audio: Arc<dyn AudioBackend>,
    pub system: Arc<dyn SystemInspector>,
    pub camera: Arc<dyn CameraControl>,
}

/// The real platform for this OS.
pub fn native() -> Platform {
    #[cfg(windows)]
    {
        windows::platform()
    }
    #[cfg(not(windows))]
    {
        mock::MockPlatform::demo().platform()
    }
}

/// Parse `VID_046D&PID_082D` style substrings from a device id / symbolic link.
pub fn parse_usb_ids(s: &str) -> Option<(u16, u16)> {
    let up = s.to_ascii_uppercase();
    let v = up.find("VID_")?;
    let p = up.find("PID_")?;
    let vid = u16::from_str_radix(up.get(v + 4..v + 8)?, 16).ok()?;
    let pid = u16::from_str_radix(up.get(p + 4..p + 8)?, 16).ok()?;
    Some((vid, pid))
}

/// Country code → mains frequency. Unknown → 50 Hz (most of the world).
pub fn mains_for_country(code: &str) -> MainsHz {
    const SIXTY: [&str; 27] = [
        "US", "CA", "MX", "BR", "CO", "VE", "EC", "PE", "CR", "PA", "GT", "HN", "NI", "SV", "DO", "PR", "CU", "BS",
        "KR", "TW", "PH", "SA", "LR", "SR", "GU", "BZ", "JP",
    ];
    if SIXTY.contains(&code.to_ascii_uppercase().as_str()) { MainsHz::Hz60 } else { MainsHz::Hz50 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_ids() {
        assert_eq!(parse_usb_ids(r"\\?\usb#vid_046d&pid_082d&mi_00#7&1"), Some((0x046d, 0x082d)));
        assert_eq!(parse_usb_ids("SWD\\MMDEVAPI\\{0.0.1.00000000}"), None);
    }

    #[test]
    fn mains() {
        assert_eq!(mains_for_country("us"), MainsHz::Hz60);
        assert_eq!(mains_for_country("SE"), MainsHz::Hz50);
    }
}
