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
    /// Mains frequency guessed from the time zone, then the region.
    fn mains_guess(&self) -> MainsGuess;
    fn mains(&self) -> MainsHz {
        self.mains_guess().hz
    }
    fn obs_running(&self) -> bool;
    /// Apps that are probably holding a camera right now (plain names).
    fn camera_users(&self) -> Vec<CameraUser> {
        Vec::new()
    }
    /// `%APPDATA%\obs-studio` (or the platform equivalent).
    fn obs_config_dir(&self) -> Option<PathBuf>;
    /// Execute a Windows-side action (registry, endpoint, power settings).
    fn apply(&self, action: &Action) -> Result<(), String>;
    /// Whether `apply` can possibly succeed right now (e.g. the app has the
    /// administrator rights the setting needs). Checked up front so the UI
    /// never offers a one-click fix that is certain to fail.
    fn can_apply(&self, _action: &Action) -> bool {
        true
    }
    /// The endpoint's input level in dB (IAudioEndpointVolume), if adjustable.
    fn endpoint_gain_db(&self, _endpoint_id: &str) -> Option<GainControl> {
        None
    }
    fn set_endpoint_gain_db(&self, _endpoint_id: &str, _db: f64) -> Result<(), String> {
        Err("This mic's level can't be changed from here.".into())
    }
    /// Open a Windows Settings page, URL or app for the user.
    fn open_target(&self, target: &crate::types::OpenTarget) -> Result<(), String>;
    /// The user's Desktop folder (follows OneDrive redirection on Windows).
    fn desktop_dir(&self) -> Option<PathBuf> {
        None
    }
    /// Read a Windows setting before we change it (for the settings vault).
    /// None = not readable / not present on this PC.
    fn read_setting(&self, _key: &WinKey) -> Option<WinValue> {
        None
    }
    /// Put a setting back to a value read earlier with `read_setting`.
    fn restore_setting(&self, _value: &WinValue) -> Result<(), String> {
        Err("This Windows setting can't be put back from here.".into())
    }
}

/// A Windows setting the app may change (one per `Action` kind).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WinKey {
    Ducking,
    EndpointVolume { endpoint: String },
    Enhancements { endpoint: String },
    VoiceClarity { endpoint: String },
    Listen { endpoint: String },
    ExclusiveMode { endpoint: String },
    MicBoost { endpoint: String },
    UsbSelectiveSuspend,
}

impl WinKey {
    /// The setting a Windows-side `Action` changes (None for OBS actions).
    pub fn for_action(a: &Action) -> Option<WinKey> {
        Some(match a {
            Action::SetWindowsDucking { .. } => WinKey::Ducking,
            Action::SetEndpointVolume { endpoint, .. } => WinKey::EndpointVolume { endpoint: endpoint.clone() },
            Action::DisableEnhancements { endpoint } => WinKey::Enhancements { endpoint: endpoint.clone() },
            Action::DisableVoiceClarity { endpoint } => WinKey::VoiceClarity { endpoint: endpoint.clone() },
            Action::DisableListen { endpoint } => WinKey::Listen { endpoint: endpoint.clone() },
            Action::DisableExclusiveMode { endpoint } => WinKey::ExclusiveMode { endpoint: endpoint.clone() },
            Action::SetMicBoost { endpoint, .. } => WinKey::MicBoost { endpoint: endpoint.clone() },
            Action::DisableUsbSelectiveSuspend => WinKey::UsbSelectiveSuspend,
            _ => return None,
        })
    }

    /// Stable id used by the vault ("windows:enhancements:{id}").
    pub fn id(&self) -> String {
        match self {
            WinKey::Ducking => "windows:ducking".into(),
            WinKey::UsbSelectiveSuspend => "windows:usb_selective_suspend".into(),
            WinKey::EndpointVolume { endpoint } => format!("windows:endpoint_volume:{endpoint}"),
            WinKey::Enhancements { endpoint } => format!("windows:enhancements:{endpoint}"),
            WinKey::VoiceClarity { endpoint } => format!("windows:voice_clarity:{endpoint}"),
            WinKey::Listen { endpoint } => format!("windows:listen:{endpoint}"),
            WinKey::ExclusiveMode { endpoint } => format!("windows:exclusive_mode:{endpoint}"),
            WinKey::MicBoost { endpoint } => format!("windows:mic_boost:{endpoint}"),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            WinKey::Ducking => "Windows: lower other sounds during calls",
            WinKey::EndpointVolume { .. } => "Windows: mic level",
            WinKey::Enhancements { .. } => "Windows: audio enhancements",
            WinKey::VoiceClarity { .. } => "Windows: Voice Clarity",
            WinKey::Listen { .. } => "Windows: Listen to this device",
            WinKey::ExclusiveMode { .. } => "Windows: let apps take over the mic",
            WinKey::MicBoost { .. } => "Windows: Microphone Boost",
            WinKey::UsbSelectiveSuspend => "Windows: USB power saving",
        }
    }
}

/// A saved Windows value. Raw values where Windows stores them raw, so
/// restoring writes back exactly what was there.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WinValue {
    pub key: WinKey,
    /// `UserDuckingPreference` (None = value absent), 0/1 flags, the
    /// endpoint volume scalar 0..1, boost dB, or powercfg indexes `[ac, dc]`.
    pub value: serde_json::Value,
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

/// Native yes/no questions to the user (the auto-updater asks before installing).
pub trait Dialogs: Send + Sync {
    /// Blocks until the user answers. `true` = Yes.
    fn ask_yes_no(&self, title: &str, text: &str) -> bool;
}

#[derive(Clone)]
pub struct Platform {
    pub audio: Arc<dyn AudioBackend>,
    pub system: Arc<dyn SystemInspector>,
    pub camera: Arc<dyn CameraControl>,
    pub dialogs: Arc<dyn Dialogs>,
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

/// `--fx-helper …` (the one-off elevated process that changes a single
/// Windows audio setting). `Some(exit code)` = this was a helper run: exit
/// with it without starting the app.
pub fn fx_helper_cli(args: &[String]) -> Option<i32> {
    #[cfg(windows)]
    {
        windows::fx_helper_main(args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        None
    }
}

/// Capture endpoint ids look like `{0.0.1.00000000}.{guid}`. The elevated
/// helper (which writes to HKLM) and the Settings deep link accept nothing else.
pub fn is_capture_endpoint_id(id: &str) -> bool {
    id.starts_with("{0.0.1.00000000}.{") && id.len() < 80 && id.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, '{' | '}' | '.' | '-'))
}

/// The Windows Settings page for exactly this microphone (Settings > System >
/// Sound > the mic, where "Audio enhancements" lives). Documented form
/// `ms-settings:sound-properties?endpointId={0.0.1.00000000}.{guid}`; falls
/// back to the default-mic page when the id isn't a Windows endpoint id.
pub fn sound_properties_uri(endpoint: &str) -> String {
    if is_capture_endpoint_id(endpoint) {
        format!("ms-settings:sound-properties?endpointId={endpoint}")
    } else {
        "ms-settings:sound-defaultinputproperties".into()
    }
}

/// Make stdout usable for the command-line modes (`--self-test`). The
/// release app is a GUI-subsystem program on Windows: started from a console
/// without redirection it has no stdout, so attach to the parent's console.
/// No-op elsewhere or when stdout is already a pipe/file.
pub fn attach_parent_console() {
    #[cfg(windows)]
    windows::attach_parent_console();
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

/// An app that has (or may have) the camera open.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CameraUser {
    /// Plain name, e.g. "OBS Studio".
    pub name: String,
    /// Windows says it is using the camera right now (privacy log); false =
    /// the app is running and known to use cameras, but we can't tell.
    pub confirmed: bool,
}

/// Plain name for a process / registry entry that can hold a camera.
/// `raw` is an exe name ("obs64.exe"), a path or a packaged-app id
/// ("MSTeams_8wekyb3d8bbwe"). Returns (name, is_known_app).
pub fn plain_camera_app(raw: &str) -> (String, bool) {
    let last = raw.rsplit(['#', '\\', '/']).next().unwrap_or(raw);
    let stem = last.split('_').next().unwrap_or(last);
    let stem = stem.strip_suffix(".exe").or_else(|| stem.strip_suffix(".EXE")).unwrap_or(stem);
    let l = stem.to_ascii_lowercase();
    const KNOWN: [(&str, &str); 22] = [
        ("obs64", "OBS Studio"),
        ("obs32", "OBS Studio"),
        ("obs", "OBS Studio"),
        ("streamlabs", "Streamlabs"),
        ("msteams", "Microsoft Teams"),
        ("ms-teams", "Microsoft Teams"),
        ("teams", "Microsoft Teams"),
        ("zoom", "Zoom"),
        ("discord", "Discord"),
        ("skype", "Skype"),
        ("webex", "Webex"),
        ("ciscocollabhost", "Webex"),
        ("slack", "Slack"),
        ("xsplit", "XSplit"),
        ("vmix", "vMix"),
        ("manycam", "ManyCam"),
        ("nvidia broadcast", "NVIDIA Broadcast"),
        ("windowscamera", "the Windows Camera app"),
        ("microsoft.windowscamera", "the Windows Camera app"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
    ];
    for (key, name) in KNOWN {
        if l == key || l.starts_with(&format!("{key}.")) || (key.len() > 4 && l.starts_with(key)) {
            return (name.to_string(), true);
        }
    }
    let mut chars = stem.chars();
    let name = match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Another app".into(),
    };
    (name, false)
}

/// Combine the Windows privacy log ("in use right now" entries, raw) with the
/// list of running processes into one de-duplicated list. Setup Coach itself
/// is left out. Confirmed users first.
pub fn merge_camera_users(in_use_raw: &[String], running: &[String]) -> Vec<CameraUser> {
    let is_us = |raw: &str| {
        let l = raw.to_ascii_lowercase();
        l.contains("setup-coach") || l.contains("setupcoach") || l.contains("setup_coach")
    };
    let mut out: Vec<CameraUser> = Vec::new();
    for raw in in_use_raw.iter().filter(|r| !is_us(r)) {
        let (name, _) = plain_camera_app(raw);
        if !out.iter().any(|u| u.name == name) {
            out.push(CameraUser { name, confirmed: true });
        }
    }
    // Only well-known camera users count as "running and might hold it"; the
    // browsers are too common to mention unless Windows confirms them.
    for raw in running.iter().filter(|r| !is_us(r)) {
        let (name, known) = plain_camera_app(raw);
        if known && !matches!(name.as_str(), "Chrome" | "Edge" | "Firefox" | "Slack") && !out.iter().any(|u| u.name == name) {
            out.push(CameraUser { name, confirmed: false });
        }
    }
    out
}
/// Country code → mains frequency. Unknown → 50 Hz (most of the world).
pub fn mains_for_country(code: &str) -> MainsHz {
    const SIXTY: [&str; 27] = [
        "US", "CA", "MX", "BR", "CO", "VE", "EC", "PE", "CR", "PA", "GT", "HN", "NI", "SV", "DO", "PR", "CU", "BS",
        "KR", "TW", "PH", "SA", "LR", "SR", "GU", "BZ", "JP",
    ];
    if SIXTY.contains(&code.to_ascii_uppercase().as_str()) { MainsHz::Hz60 } else { MainsHz::Hz50 }
}

/// Windows time zone id (`TimeZoneKeyName`, e.g. "W. Europe Standard Time")
/// → mains frequency. `None` when the zone doesn't settle it (zones spanning
/// both standards, or an id we don't know), so the region can decide.
pub fn mains_for_timezone(win_tz: &str) -> Option<MainsHz> {
    // 60 Hz: North America, Central America, northern South America, Caribbean,
    // Japan (west), South Korea, Taiwan.
    const SIXTY: [&str; 31] = [
        "Pacific Standard Time", "Pacific Standard Time (Mexico)", "US Mountain Standard Time", "Mountain Standard Time",
        "Mountain Standard Time (Mexico)", "Central Standard Time", "Central Standard Time (Mexico)", "Canada Central Standard Time",
        "Central America Standard Time", "Eastern Standard Time", "Eastern Standard Time (Mexico)", "US Eastern Standard Time",
        "Atlantic Standard Time", "Newfoundland Standard Time", "Alaskan Standard Time", "Hawaiian Standard Time",
        "Aleutian Standard Time", "Yukon Standard Time", "Cuba Standard Time", "Haiti Standard Time",
        "Turks And Caicos Standard Time", "SA Pacific Standard Time", "Venezuela Standard Time", "E. South America Standard Time",
        "Tocantins Standard Time", "Bahia Standard Time", "Central Brazilian Standard Time", "Tokyo Standard Time",
        "Korea Standard Time", "Taipei Standard Time", "Paraguay Standard Time",
    ];
    let id = win_tz.trim();
    if SIXTY.iter().any(|z| z.eq_ignore_ascii_case(id)) {
        return Some(MainsHz::Hz60);
    }
    // 50 Hz: Europe, the UK, Africa, Russia, the Middle East (mostly), South
    // and East Asia, Australia, New Zealand.
    const FIFTY_WORDS: [&str; 46] = [
        "FLE", "GTB", "Arabic", "SE Asia", "Sri Lanka", "Nepal", "Myanmar", "Mongolia", "Central Asia", "North Asia", "Afghanistan",
        "Georgian", "Caucasus", "Armenian", "Ukraine", "Morocco", "Namibia", "Mauritius", "Sao Tome",
        "Europe", "European", "GMT", "Greenwich", "Romance", "Russia", "Turkey", "Israel", "Jerusalem", "Egypt", "Africa", "India",
        "China", "Australia", "AUS", "Tasmania", "New Zealand", "Pakistan", "Bangladesh", "Iran", "Arabian", "Azerbaijan", "Kaliningrad",
        "Belarus", "Ekaterinburg", "Syria", "Libya",
    ];
    if FIFTY_WORDS.iter().any(|w| id.contains(w)) {
        return Some(MainsHz::Hz50);
    }
    None
}

/// Where a mains guess came from (shown under Advanced).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainsSource {
    /// The user picked it under Advanced.
    Override,
    /// From the Windows time zone (where the PC physically is).
    TimeZone,
    /// From the Windows region setting.
    Region,
    /// Nothing to go on: 50 Hz, like most of the world.
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainsGuess {
    pub hz: MainsHz,
    pub source: MainsSource,
}

/// Best guess of the local mains frequency. The time zone wins over the
/// region setting: PCs are often set to a US region/language while sitting
/// in Europe, but the time zone follows the clock on the wall.
pub fn detect_mains(win_tz: Option<&str>, region: Option<&str>) -> MainsGuess {
    if let Some(hz) = win_tz.and_then(mains_for_timezone) {
        return MainsGuess { hz, source: MainsSource::TimeZone };
    }
    match region.filter(|r| !r.trim().is_empty()) {
        Some(r) => MainsGuess { hz: mains_for_country(r), source: MainsSource::Region },
        None => MainsGuess { hz: MainsHz::Hz50, source: MainsSource::Default },
    }
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

    #[test]
    fn mains_from_timezone_beats_region() {
        // A PC in Sweden set to a US region and language: the clock says Europe.
        let g = detect_mains(Some("W. Europe Standard Time"), Some("US"));
        assert_eq!((g.hz, g.source), (MainsHz::Hz50, MainsSource::TimeZone));
        let g = detect_mains(Some("Eastern Standard Time"), Some("SE"));
        assert_eq!((g.hz, g.source), (MainsHz::Hz60, MainsSource::TimeZone));
        for tz in [
            "GMT Standard Time",
            "Central Europe Standard Time",
            "Romance Standard Time",
            "FLE Standard Time",
            "E. Europe Standard Time",
            "Russian Standard Time",
            "India Standard Time",
            "AUS Eastern Standard Time",
            "W. Central Africa Standard Time",
        ] {
            assert_eq!(mains_for_timezone(tz), Some(MainsHz::Hz50), "{tz}");
        }
        for tz in ["Pacific Standard Time", "Central Standard Time (Mexico)", "SA Pacific Standard Time", "Tokyo Standard Time", "Korea Standard Time"] {
            assert_eq!(mains_for_timezone(tz), Some(MainsHz::Hz60), "{tz}");
        }
    }

    #[test]
    fn mains_falls_back_to_region_then_50() {
        // Zones that span both standards defer to the region.
        let g = detect_mains(Some("Singapore Standard Time"), Some("PH"));
        assert_eq!((g.hz, g.source), (MainsHz::Hz60, MainsSource::Region));
        let g = detect_mains(None, Some("SE"));
        assert_eq!((g.hz, g.source), (MainsHz::Hz50, MainsSource::Region));
        let g = detect_mains(Some("Some Future Standard Time"), None);
        assert_eq!((g.hz, g.source), (MainsHz::Hz50, MainsSource::Default));
        // Never defaults to 60 just because nothing is known.
        assert_eq!(detect_mains(None, Some("")).hz, MainsHz::Hz50);
    }

    #[test]
    fn camera_users_are_named_plainly() {
        let users = merge_camera_users(
            &[r"C:#Program Files#obs-studio#bin#64bit#obs64.exe".into(), "MSTeams_8wekyb3d8bbwe".into(), r"C:#Apps#SetupCoach#setup-coach-app.exe".into()],
            &["obs64.exe".into(), "zoom.exe".into(), "chrome.exe".into(), "explorer.exe".into()],
        );
        let names: Vec<_> = users.iter().map(|u| (u.name.as_str(), u.confirmed)).collect();
        assert_eq!(names, vec![("OBS Studio", true), ("Microsoft Teams", true), ("Zoom", false)]);
        assert_eq!(plain_camera_app("weirdtool.exe"), ("Weirdtool".to_string(), false));
    }
}
