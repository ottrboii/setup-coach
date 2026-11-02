//! Snapshots of Windows, OBS, camera and hardware state, collected by the
//! Windows app and handed to the audit as plain data.
//!
//! Every struct is serde-friendly and has sensible defaults so a partial
//! snapshot (e.g. OBS not running) still audits cleanly.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- Windows audio

/// One Windows audio endpoint (capture or render).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioEndpoint {
    /// Windows endpoint id (MMDevice id).
    pub id: String,
    /// Friendly name, e.g. "Microphone (Yeti Stereo Microphone)".
    pub name: String,
    /// true = capture (mic), false = render (speakers/headphones).
    pub is_capture: bool,
    pub is_default: bool,
    pub is_default_comms: bool,
    /// Bluetooth headset in Hands-Free (HFP) mode: mono, 8/16 kHz.
    pub is_bluetooth_handsfree: bool,
    /// Shared-mode mix format.
    pub sample_rate: u32,
    pub channels: u16,
    /// "Audio enhancements" (system effects) enabled on this endpoint.
    pub enhancements_enabled: bool,
    /// Windows 11 "Voice Clarity" (AI effect) active on this endpoint.
    pub voice_clarity_enabled: bool,
    /// "Microphone Boost" level in dB, if the driver exposes it.
    pub mic_boost_db: Option<f32>,
    /// Onboard Realtek-style input (weak preamp, boost can be legitimate).
    pub is_onboard: bool,
    /// Endpoint volume scalar 0.0..=1.0.
    pub endpoint_volume: f32,
    pub exclusive_mode_allowed: bool,
    /// "Listen to this device" ticked.
    pub listen_to_device: bool,
}

impl Default for AudioEndpoint {
    fn default() -> Self {
        AudioEndpoint {
            id: String::new(),
            name: String::new(),
            is_capture: true,
            is_default: false,
            is_default_comms: false,
            is_bluetooth_handsfree: false,
            sample_rate: 48_000,
            channels: 1,
            enhancements_enabled: false,
            voice_clarity_enabled: false,
            mic_boost_db: None,
            is_onboard: false,
            endpoint_volume: 1.0,
            exclusive_mode_allowed: false,
            listen_to_device: false,
        }
    }
}

/// Communications tab: "When Windows detects communications activity".
/// Registry `UserDuckingPreference`: 0 mute, 1 reduce 80%, 2 reduce 50%, 3 do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DuckingPreference {
    MuteOthers,
    #[default]
    Reduce80,
    Reduce50,
    DoNothing,
}

impl DuckingPreference {
    pub fn from_registry(v: u32) -> Self {
        match v {
            0 => DuckingPreference::MuteOthers,
            2 => DuckingPreference::Reduce50,
            3 => DuckingPreference::DoNothing,
            _ => DuckingPreference::Reduce80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsAudioState {
    pub endpoints: Vec<AudioEndpoint>,
    pub ducking: DuckingPreference,
    /// "Let desktop apps access your microphone".
    pub mic_privacy_desktop_allowed: bool,
    pub usb_selective_suspend_enabled: bool,
    /// Another app (e.g. Discord AGC) changed the mic endpoint level during the scan.
    pub level_changed_by_other_app: bool,
}

impl Default for WindowsAudioState {
    fn default() -> Self {
        WindowsAudioState {
            endpoints: Vec::new(),
            ducking: DuckingPreference::DoNothing,
            mic_privacy_desktop_allowed: true,
            usb_selective_suspend_enabled: false,
            level_changed_by_other_app: false,
        }
    }
}

impl WindowsAudioState {
    pub fn endpoint(&self, id: &str) -> Option<&AudioEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }
    pub fn default_capture(&self) -> Option<&AudioEndpoint> {
        self.endpoints.iter().find(|e| e.is_capture && e.is_default)
    }
    pub fn default_render(&self) -> Option<&AudioEndpoint> {
        self.endpoints.iter().find(|e| !e.is_capture && e.is_default)
    }
}

// ---------------------------------------------------------------- OBS

/// OBS input kinds we care about.
pub const OBS_KIND_MIC: &str = "wasapi_input_capture";
pub const OBS_KIND_DESKTOP: &str = "wasapi_output_capture";
pub const OBS_KIND_APP_AUDIO: &str = "wasapi_process_output_capture";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ObsFilter {
    pub name: String,
    /// OBS filter kind id, e.g. `compressor_filter`.
    pub kind: String,
    pub enabled: bool,
    /// Raw filter settings as OBS reports them.
    pub settings: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringType {
    #[default]
    Off,
    MonitorOnly,
    MonitorAndOutput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ObsAudioInput {
    pub name: String,
    /// `wasapi_input_capture`, `wasapi_output_capture`, `wasapi_process_output_capture`, ...
    pub kind: String,
    /// Windows endpoint id, or `default`.
    pub device_id: String,
    /// Friendly device name OBS shows (helps spot Wave Link / GoXLR mixes).
    pub device_name: String,
    /// Advanced Audio Properties "Mono" (downmix).
    pub mono_downmix: bool,
    pub monitoring: MonitoringType,
    /// Fader in dB (0 = unity).
    pub volume_db: f32,
    pub muted: bool,
    /// Filters in OBS order (top first).
    pub filters: Vec<ObsFilter>,
    /// Application Audio Capture target executable, e.g. `Discord.exe`.
    pub app_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorRange {
    #[default]
    Default,
    Partial,
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ObsVideoCapture {
    pub name: String,
    pub device_id: String,
    pub device_name: String,
    /// Resolution/FPS Type = Custom (false = "Device Default").
    pub res_type_custom: bool,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f32>,
    /// `MJPEG`, `YUY2`, `NV12`, `Any`, ...
    pub video_format: String,
    pub color_range: ColorRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ObsEncoder {
    /// e.g. `obs_x264`, `obs_nvenc_h264_tex`, `jim_nvenc`, `h264_texture_amf`, `obs_qsv11`.
    pub encoder_id: String,
    /// `CBR`, `VBR`, `CQP`, ...
    pub rate_control: String,
    pub bitrate_kbps: u32,
    /// 0 = auto.
    pub keyframe_sec: u32,
    pub preset: String,
    pub output_width: u32,
    pub output_height: u32,
    pub fps: f32,
    pub audio_bitrate_kbps: u32,
    /// Twitch Enhanced Broadcasting / multitrack video enabled (platform manages encoding).
    pub enhanced_broadcasting: bool,
}

impl ObsEncoder {
    pub fn is_cpu_encoder(&self) -> bool {
        self.encoder_id.contains("x264") || self.encoder_id.contains("x265")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Twitch,
    YouTube,
    Kick,
    TikTok,
    Facebook,
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObsState {
    /// e.g. "33.0.1".
    pub version: String,
    pub websocket_enabled: bool,
    pub sample_rate: u32,
    pub audio_inputs: Vec<ObsAudioInput>,
    pub video_captures: Vec<ObsVideoCapture>,
    /// Output (video settings) colour range.
    pub output_color_range: ColorRange,
    pub encoder: Option<ObsEncoder>,
    /// `mkv`, `mp4`, `hybrid_mp4`, `mov`, `flv`, ...
    pub recording_format: String,
    pub platform: Platform,
    /// Name of the input the user picked as their main mic (else first mic input).
    pub mic_input: Option<String>,
}

impl Default for ObsState {
    fn default() -> Self {
        ObsState {
            version: String::new(),
            websocket_enabled: true,
            sample_rate: 48_000,
            audio_inputs: Vec::new(),
            video_captures: Vec::new(),
            output_color_range: ColorRange::Partial,
            encoder: None,
            recording_format: "mkv".into(),
            platform: Platform::Other,
            mic_input: None,
        }
    }
}

impl ObsState {
    /// The main mic input: the named one, else the first mic capture.
    pub fn main_mic(&self) -> Option<&ObsAudioInput> {
        if let Some(name) = &self.mic_input {
            if let Some(i) = self.audio_inputs.iter().find(|i| &i.name == name) {
                return Some(i);
            }
        }
        self.audio_inputs.iter().find(|i| i.kind == OBS_KIND_MIC)
    }

    /// Major version number, if parseable.
    pub fn major_version(&self) -> Option<u32> {
        self.version.split('.').next()?.trim().parse().ok()
    }
}

// ---------------------------------------------------------------- Camera system

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSystemState {
    pub privacy_allowed: bool,
    pub frame_server_running: bool,
    pub studio_effects_on: bool,
    pub multi_app_camera_on: bool,
    /// Vendor camera apps running, e.g. "G HUB", "Camera Hub", "Razer Synapse".
    pub vendor_apps_running: Vec<String>,
}

impl Default for CameraSystemState {
    fn default() -> Self {
        CameraSystemState {
            privacy_allowed: true,
            frame_server_running: true,
            studio_effects_on: false,
            multi_app_camera_on: false,
            vendor_apps_running: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------- Processing apps / mic context

/// Third-party voice processing currently active on the mic path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KnownProcessingApps {
    pub ghub_blue_voice_active: bool,
    pub wave_link_fx_active: bool,
    /// The official (unmaintained) TC-Helicon GoXLR App.
    pub goxlr_app_running: bool,
    /// GoXLR Utility (open source, recommended replacement) daemon.
    pub goxlr_utility_running: bool,
    /// NVIDIA Broadcast mic effects (AI).
    pub nvidia_broadcast_mic_active: bool,
    /// Krisp (AI).
    pub krisp_active: bool,
    pub sonar_mic_active: bool,
}

impl KnownProcessingApps {
    pub fn ai_active(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.nvidia_broadcast_mic_active {
            v.push("NVIDIA Broadcast");
        }
        if self.krisp_active {
            v.push("Krisp");
        }
        v
    }
    pub fn dsp_active(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.ghub_blue_voice_active {
            v.push("Blue VO!CE (G HUB)");
        }
        if self.wave_link_fx_active {
            v.push("Wave Link effects");
        }
        if self.goxlr_app_running || self.goxlr_utility_running {
            v.push("GoXLR");
        }
        if self.sonar_mic_active {
            v.push("SteelSeries Sonar");
        }
        v
    }
}

/// Hints about the mic from its device profile and the user's answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MicContext {
    /// Profile says only one channel carries the mic (interface input 1 on a 2-ch endpoint).
    pub single_live_channel: bool,
    /// Hardware direct monitoring is on (None = unknown).
    pub direct_monitoring_on: Option<bool>,
    /// For mics where the Windows slider *is* the hardware gain (e.g. no gain knob).
    pub windows_slider_is_hardware_gain: bool,
    /// Measured in the guided test, dBFS.
    pub measured_noise_floor_dbfs: Option<f32>,
    /// Quietest normal speech level measured (short-term RMS), dBFS.
    pub measured_quiet_speech_dbfs: Option<f32>,
    /// On-mic DSP from the profile that is active, e.g. `auto_level`.
    pub onboard_dsp_active: Vec<String>,
}

// ---------------------------------------------------------------- Hardware / network

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HardwareInfo {
    pub gpu_vendor: GpuVendor,
    pub gpu_model: String,
    pub has_nvenc: bool,
    pub has_amf: bool,
    pub has_qsv: bool,
    pub cpu_cores: u32,
}

impl HardwareInfo {
    /// Best hardware H.264 encoder id OBS uses for this machine, if any.
    pub fn best_hw_encoder(&self) -> Option<&'static str> {
        if self.has_nvenc {
            Some("obs_nvenc_h264_tex")
        } else if self.has_amf {
            Some("h264_texture_amf")
        } else if self.has_qsv {
            Some("obs_qsv11_v2")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Wired,
    Wifi,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NetworkInfo {
    pub connection: Connection,
    pub measured_upload_kbps: Option<u32>,
}

// ---------------------------------------------------------------- Bundle

/// Everything the audit looks at. Missing parts are simply skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SystemSnapshot {
    pub windows: Option<WindowsAudioState>,
    pub obs: Option<ObsState>,
    pub camera: Option<CameraSystemState>,
    pub processing: KnownProcessingApps,
    pub mic: MicContext,
    pub hardware: Option<HardwareInfo>,
    pub network: Option<NetworkInfo>,
}

impl SystemSnapshot {
    /// The Windows endpoint the main mic uses (resolving OBS "default").
    pub fn mic_endpoint(&self) -> Option<&AudioEndpoint> {
        let win = self.windows.as_ref()?;
        match self.obs.as_ref().and_then(|o| o.main_mic()) {
            Some(input) if input.device_id != "default" && !input.device_id.is_empty() => {
                win.endpoint(&input.device_id).or_else(|| win.default_capture())
            }
            _ => win.default_capture(),
        }
    }
}
