//! Profile schema: what we know about each device. Stored as JSON under
//! `setup-coach/profiles/`. Numbers we are not confident about are `null`
//! and the profile is `verified: false` until checked on real hardware.

use serde::{Deserialize, Serialize};

/// Bump when the JSON layout changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------- shared --

/// A USB vendor/product id pair, written in JSON as hex strings
/// (`{"vid": "046d", "pid": "082d"}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "UsbIdRaw", into = "UsbIdRaw")]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Serialize, Deserialize)]
struct UsbIdRaw {
    vid: String,
    pid: String,
}

impl TryFrom<UsbIdRaw> for UsbId {
    type Error = String;
    fn try_from(raw: UsbIdRaw) -> Result<Self, Self::Error> {
        let parse = |s: &str| {
            u16::from_str_radix(s.trim_start_matches("0x"), 16)
                .map_err(|e| format!("bad USB id '{s}': {e}"))
        };
        Ok(UsbId { vid: parse(&raw.vid)?, pid: parse(&raw.pid)? })
    }
}

impl From<UsbId> for UsbIdRaw {
    fn from(id: UsbId) -> Self {
        UsbIdRaw { vid: format!("{:04x}", id.vid), pid: format!("{:04x}", id.pid) }
    }
}

/// Built-in processing on the device or in its vendor app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnboardDsp {
    pub name: String,
    pub kind: DspKind,
    /// `None` = not known whether it can be turned off.
    #[serde(default)]
    pub can_disable: Option<bool>,
    /// What we tell the user to do with it.
    pub recommendation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DspKind {
    /// Automatic level riding (Shure Auto Level, Yeti Orb auto gain).
    AutoLevel,
    /// Peak protection (Clipguard, Clip Safe, analog limiter). Usually fine to keep.
    Limiter,
    /// One-shot or continuous automatic gain setting.
    AutoGain,
    /// Noise cancellation / denoise (often AI) - turn off per our philosophy.
    NoiseCancel,
    /// Voice presets (Blue VO!CE, DJI voice tone, APHEX presets).
    Preset,
    /// A full processing chain (GoXLR, RODECaster): gate/comp/EQ/de-ess.
    Processing,
}

// ------------------------------------------------------------------ mics --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MicConnection {
    Usb,
    Xlr,
    /// USB and XLR outputs on the same mic (Q2U, ATR2100x, MV7, PodMic USB).
    UsbXlrHybrid,
    Headset { path: HeadsetPath },
    /// Wireless transmitter + receiver (DJI Mic, Rode Wireless GO).
    Wireless,
    /// Laptop / webcam built-in mic.
    Builtin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadsetPath {
    /// 3.5 mm jack into the PC (usually Realtek front panel).
    Analog35,
    /// Wired USB or a USB sound-card dongle.
    UsbDongle,
    /// 2.4 GHz wireless dongle.
    Wireless24,
    /// Bluetooth in Hands-Free mode (mono, 8-16 kHz). Always warn.
    BluetoothHfp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capsule {
    Dynamic,
    Condenser,
    /// Small electret capsule (headsets, lavs, laptop mics).
    Electret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    Cardioid,
    Supercardioid,
    Hypercardioid,
    Omni,
    Figure8,
    Stereo,
}

/// Which way the mic is spoken into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    /// Speak into the side (usually the logo side) - Yeti, AT2020, QuadCast.
    Side,
    /// Speak into the end/tip - SM7B, PodMic, RE20.
    End,
    /// Speak into the top of a small desk mic - SoloCast, Seiren Mini.
    Top,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Address {
    Side,
    End,
    Top,
    /// Omni capsule where direction matters little (lavs, laptop arrays).
    Any,
    /// Switchable pattern; `default` is the factory setting.
    MultiPattern { default: Pattern, facing: Facing },
}

impl Address {
    pub fn facing(&self) -> Option<Facing> {
        match self {
            Address::Side => Some(Facing::Side),
            Address::End => Some(Facing::End),
            Address::Top => Some(Facing::Top),
            Address::Any => None,
            Address::MultiPattern { facing, .. } => Some(*facing),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLayout {
    /// Two channels carrying the same mono signal (most USB mics).
    MonoAsStereo,
    TrueMono,
    /// Two channels, only one live (interface input 1 = left only).
    StereoOneLive,
    Stereo,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DistanceCm {
    pub min: f32,
    pub ideal: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EqKind {
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    /// High-pass; `gain_db` is ignored (0).
    HighPass,
}

/// A gentle starting EQ move, before measurement corrects it. Capped at +/-6 dB.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqMove {
    #[serde(default)]
    pub kind: EqKind,
    pub freq_hz: f32,
    #[serde(default)]
    pub gain_db: f32,
    pub q: f32,
}

/// Generic fallback classes for devices we don't have a specific profile for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicClass {
    UsbCondenserSide,
    UsbCondenserEnd,
    HybridDynamicEnd,
    XlrDynamicLowOutput,
    XlrCondenser,
    HeadsetAnalog,
    HeadsetUsbWireless,
    HeadsetBluetooth,
    BuiltinMic,
    WirelessLav,
}

impl MicClass {
    pub const ALL: [MicClass; 10] = [
        MicClass::UsbCondenserSide,
        MicClass::UsbCondenserEnd,
        MicClass::HybridDynamicEnd,
        MicClass::XlrDynamicLowOutput,
        MicClass::XlrCondenser,
        MicClass::HeadsetAnalog,
        MicClass::HeadsetUsbWireless,
        MicClass::HeadsetBluetooth,
        MicClass::BuiltinMic,
        MicClass::WirelessLav,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicProfile {
    pub id: String,
    pub brand: String,
    pub model: String,
    /// Groups variants that share specs (e.g. `yeti`, `quadcast`).
    pub family: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Set only on generic fallback profiles.
    #[serde(default)]
    pub generic_class: Option<MicClass>,
    pub connection: MicConnection,
    pub capsule: Capsule,
    pub address: Address,
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub needs_phantom: bool,
    #[serde(default)]
    pub sensitivity_dbv_pa: Option<f32>,
    /// Typical clean gain needed for normal speech at the ideal distance.
    #[serde(default)]
    pub gain_needed_db: Option<f32>,
    /// For USB mics: does the Windows input slider change the real gain?
    #[serde(default)]
    pub windows_slider_is_hw_gain: Option<bool>,
    #[serde(default)]
    pub hardware_controls: Vec<String>,
    /// Switches the user may have flipped (asked about with a photo).
    #[serde(default)]
    pub switches: Vec<String>,
    #[serde(default)]
    pub onboard_dsp: Vec<OnboardDsp>,
    #[serde(default)]
    pub vendor_app: Option<String>,
    #[serde(default)]
    pub sample_rates: Vec<u32>,
    #[serde(default)]
    pub channel_layout: Option<ChannelLayout>,
    #[serde(default)]
    pub usb_ids: Vec<UsbId>,
    /// Lower-case substrings of the Windows endpoint name.
    #[serde(default)]
    pub endpoint_name_patterns: Vec<String>,
    pub distance_cm: DistanceCm,
    /// Ideal off-axis angle in degrees (0 = straight at the mouth).
    #[serde(default)]
    pub angle_deg: Option<f32>,
    #[serde(default)]
    pub eq_prior: Vec<EqMove>,
    /// Plain-language things that commonly go wrong with this device.
    #[serde(default)]
    pub gotchas: Vec<String>,
    #[serde(default)]
    pub verified: bool,
    /// Owner has this device: verify on real hardware first.
    #[serde(default)]
    pub verify_first: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}

// ------------------------------------------------------------ interfaces --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    Interface,
    /// Streamer mixers with routing + DSP (GoXLR, RODECaster, Wave XLR).
    StreamerMixer,
    /// Analog mixer with USB (Yamaha AG, Behringer Xenyx).
    Mixer,
    /// Combined capture card + audio interface (Rode Streamer X).
    CaptureAndInterface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceFeature {
    AutoGain,
    ClipSafe,
    AirMode,
    Clipguard,
    OnboardDsp,
    Loopback,
    DirectMonitoring,
    MultipleMixes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// The finished mix meant for OBS (GoXLR "Broadcast Stream Mix").
    StreamMix,
    MonitorMix,
    /// The raw mic input.
    MicRaw,
    /// Processed mic meant for Discord/chat apps.
    ChatMic,
    System,
    Game,
    Music,
    Sample,
    ChatOutput,
    /// Main stereo input/output.
    Main,
}

/// A named Windows endpoint a mixer creates, and whether OBS should use it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndpointRole {
    pub name_pattern: String,
    pub role: EndpointKind,
    #[serde(default)]
    pub use_in_obs: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceProfile {
    pub id: String,
    pub brand: String,
    pub model: String,
    pub family: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub kind: InterfaceKind,
    #[serde(default)]
    pub xlr_inputs: u8,
    #[serde(default)]
    pub max_gain_db: Option<f32>,
    /// Equivalent input noise, dBu (more negative = quieter).
    #[serde(default)]
    pub ein_dbu: Option<f32>,
    /// Mic in input 1 arrives as the left channel only of a stereo endpoint.
    #[serde(default)]
    pub input1_left_only: bool,
    #[serde(default)]
    pub phantom: bool,
    #[serde(default)]
    pub features: Vec<InterfaceFeature>,
    #[serde(default)]
    pub onboard_dsp: Vec<OnboardDsp>,
    #[serde(default)]
    pub endpoints: Vec<EndpointRole>,
    #[serde(default)]
    pub vendor_app: Option<String>,
    #[serde(default)]
    pub usb_ids: Vec<UsbId>,
    #[serde(default)]
    pub name_patterns: Vec<String>,
    #[serde(default)]
    pub gotchas: Vec<String>,
    #[serde(default)]
    pub discontinued: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub verify_first: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}

impl InterfaceProfile {
    /// Role of a Windows endpoint created by this device, if known.
    pub fn endpoint_role(&self, endpoint_name: &str) -> Option<&EndpointRole> {
        let name = super::matcher::normalize(endpoint_name);
        self.endpoints
            .iter()
            .filter(|e| name.contains(&super::matcher::normalize(&e.name_pattern)))
            .max_by_key(|e| e.name_pattern.len())
    }
}

/// Inline mic booster (Cloudlifter, FetHead): a chain modifier, not a device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoosterProfile {
    pub id: String,
    pub brand: String,
    pub model: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub gain_db: f32,
    pub needs_phantom: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}

// --------------------------------------------------------------- cameras --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraClass {
    /// Standard UVC webcam with manual exposure/WB/focus.
    FullUvc,
    /// UVC webcam with few manual controls (fixed focus, basic procamp).
    LimitedUvc,
    /// HDMI capture: UVC controls only touch the capture chip -> coach-only.
    CaptureCard,
    /// Phone used as webcam via an app -> coach-only (+ in-app tips).
    Phone,
    /// Virtual camera (OBS Virtual Camera, NVIDIA Broadcast) -> coach-only.
    Virtual,
    /// Laptop built-in camera.
    Laptop,
}

impl CameraClass {
    /// True when the app can't change image settings and only coaches.
    pub fn coach_only(self) -> bool {
        matches!(self, CameraClass::CaptureCard | CameraClass::Phone | CameraClass::Virtual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlName {
    Exposure,
    AutoExposure,
    /// "Low Light Compensation" (KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY).
    AutoExposurePriority,
    Gain,
    WhiteBalance,
    AutoWhiteBalance,
    Focus,
    AutoFocus,
    Zoom,
    Pan,
    Tilt,
    Brightness,
    Contrast,
    Saturation,
    Sharpness,
    BacklightCompensation,
    PowerLineFrequency,
    Hue,
    Gamma,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UvcControl {
    pub name: ControlName,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub default: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    /// Lost on replug / power cycle: the app must re-apply.
    Volatile,
    /// Camera has a "save to device" command (Facecam, Kiyo Pro).
    OnDeviceSaveCmd,
    /// Vendor app stores presets and re-applies them.
    VendorAppPreset,
    /// Settings stored on the device automatically.
    OnDevice,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CameraQuirk {
    /// Auto exposure + low-light compensation drops fps in dim light.
    AePriorityFpsDrop,
    /// Ignores controls set before the stream starts -> apply after start.
    IgnoresControlsBeforeStream,
    /// Uncompressed YUY2 at 1080p gives very low fps -> use MJPEG.
    #[serde(rename = "yuy2_1080p_low_fps")]
    Yuy21080pLowFps,
    FixedFocus {
        #[serde(default)]
        min_cm: Option<f32>,
        #[serde(default)]
        max_cm: Option<f32>,
    },
    HdrHalvesFps,
    /// On-device AI tracking/auto-framing that reframes by itself.
    OnboardAiTracking,
    OversharpenedDefault,
    /// Some modes (4K, 1080p60, uncompressed) need USB 3.
    RequiresUsb3,
    /// Exposure only moves in coarse whole-stop steps.
    CoarseExposureSteps,
    RunsHot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Mjpeg,
    Yuy2,
    Nv12,
    H264,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: PixelFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraProfile {
    pub id: String,
    pub brand: String,
    pub model: String,
    pub family: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub class: CameraClass,
    /// Generic fallback profile.
    #[serde(default)]
    pub generic: bool,
    #[serde(default)]
    pub usb_ids: Vec<UsbId>,
    /// Lower-case substrings of the Windows device name.
    #[serde(default)]
    pub name_patterns: Vec<String>,
    /// Controls reachable through standard UVC (DirectShow / Media Foundation).
    #[serde(default)]
    pub controls: Vec<UvcControl>,
    /// Controls only reachable through the vendor app (extension units).
    #[serde(default)]
    pub vendor_xu_only: Vec<String>,
    #[serde(default)]
    pub persistence: Persistence,
    #[serde(default)]
    pub quirks: Vec<CameraQuirk>,
    #[serde(default)]
    pub recommended_modes: Vec<VideoMode>,
    #[serde(default)]
    pub vendor_app: Option<String>,
    #[serde(default)]
    pub gotchas: Vec<String>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub verify_first: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}

impl CameraProfile {
    pub fn control(&self, name: ControlName) -> Option<&UvcControl> {
        self.controls.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyConnection {
    /// HDMI out into a capture card (Cam Link etc.).
    HdmiCapture,
    /// Camera acts as a UVC webcam over USB by itself.
    NativeUvc,
    /// Needs the maker's webcam utility (virtual camera).
    VendorUtility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceTopic {
    Shutter,
    Iso,
    WhiteBalance,
    CleanHdmi,
    AutoPowerOff,
    Overheating,
    DummyBattery,
    Autofocus,
    FrameRate,
    PictureProfile,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuGuidance {
    pub topic: GuidanceTopic,
    pub text: String,
}

/// A real camera used through a capture card or webcam utility: the app can't
/// change its settings, so it coaches through the camera's own menus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraBodyProfile {
    pub id: String,
    pub brand: String,
    pub model: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub connections: Vec<BodyConnection>,
    #[serde(default)]
    pub clean_hdmi: Option<bool>,
    #[serde(default)]
    pub hdmi_max_output: Option<String>,
    #[serde(default)]
    pub dummy_battery: Option<String>,
    #[serde(default)]
    pub webcam_utility: Option<String>,
    #[serde(default)]
    pub guidance: Vec<MenuGuidance>,
    #[serde(default)]
    pub gotchas: Vec<String>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub verify_first: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}
