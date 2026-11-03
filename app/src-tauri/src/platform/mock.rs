//! Mock platform: used on non-Windows systems and in tests. Feeds synthetic
//! recordings and frames so the whole flow runs end to end.

use super::*;
use coach_core::audio::synth::{self, Scenario};
use coach_core::audio::MicTest;
use coach_core::system::{AudioEndpoint, DuckingPreference, GpuVendor};
use std::sync::Mutex;

pub struct MockAudio {
    pub inputs: Vec<AudioDevice>,
    /// The next capture feeds this recording (whole, in 10 ms chunks).
    pub recording: Mutex<Option<MicTest>>,
    pub played: Mutex<usize>,
    /// When set, captures stream this source live (in a thread) instead.
    pub live: Mutex<Option<LiveMock>>,
}

/// A live mock mic: loops `source` (mono) scaled by `offset_db + gain_db`,
/// where `gain_db` is shared with the mock "hardware" (Windows level or a
/// fake GoXLR), so gain changes are audible to the coach.
#[derive(Clone)]
pub struct LiveMock {
    pub source: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub gain_db: Arc<Mutex<f64>>,
    pub offset_db: f64,
    /// Faster than real time (10 = 10× speed).
    pub speed: f64,
}

struct NoopHandle;
impl StreamHandle for NoopHandle {
    fn stop(self: Box<Self>) {}
}

struct LiveHandle(Arc<std::sync::atomic::AtomicBool>);
impl StreamHandle for LiveHandle {
    fn stop(self: Box<Self>) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl AudioBackend for MockAudio {
    fn list_inputs(&self) -> Vec<AudioDevice> {
        self.inputs.clone()
    }

    fn start_capture(&self, device_id: &str, mut sink: SampleSink) -> Result<Box<dyn StreamHandle>, String> {
        if !self.inputs.iter().any(|d| d.id == device_id) {
            return Err("That microphone isn't connected any more.".into());
        }
        if let Some(live) = self.live.lock().unwrap().clone() {
            let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let r = running.clone();
            std::thread::spawn(move || {
                let chunk = (live.sample_rate / 100) as usize;
                let mut pos = 0usize;
                let pause = std::time::Duration::from_secs_f64(0.01 / live.speed.max(0.1));
                while r.load(std::sync::atomic::Ordering::SeqCst) {
                    let g = 10f64.powf((live.offset_db + *live.gain_db.lock().unwrap()) / 20.0) as f32;
                    let buf: Vec<f32> = (0..chunk).map(|i| (live.source[(pos + i) % live.source.len()] * g).clamp(-1.0, 1.0)).collect();
                    pos = (pos + chunk) % live.source.len();
                    sink(&buf, 1, live.sample_rate);
                    std::thread::sleep(pause);
                }
            });
            return Ok(Box::new(LiveHandle(running)));
        }
        let test = self.recording.lock().unwrap().clone().unwrap_or_else(|| synth::build(&Scenario::default()));
        let ch = test.audio.channels.len() as u16;
        let sr = test.audio.sample_rate;
        let n = test.audio.len();
        let chunk = (sr / 100) as usize;
        let mut inter = Vec::with_capacity(chunk * ch as usize);
        let mut i = 0;
        while i < n {
            inter.clear();
            for s in i..(i + chunk).min(n) {
                for c in &test.audio.channels {
                    inter.push(c[s]);
                }
            }
            sink(&inter, ch, sr);
            i += chunk;
        }
        Ok(Box::new(NoopHandle))
    }

    fn play(&self, _buf: AudioBuffer) -> Result<Box<dyn StreamHandle>, String> {
        *self.played.lock().unwrap() += 1;
        Ok(Box::new(NoopHandle))
    }
}

pub struct MockSystem {
    /// Software mic level in dB (shared with a `LiveMock`).
    pub mic_gain_db: Arc<Mutex<f64>>,
    pub mic_gain_range: (f64, f64),
    /// Whether the mock mic's Windows level is a real (hardware) gain.
    pub mic_gain_adjustable: bool,
    pub opened: Mutex<Vec<crate::types::OpenTarget>>,
    pub windows: Mutex<WindowsAudioState>,
    pub camera: CameraSystemState,
    pub processing: KnownProcessingApps,
    pub obs_running: Mutex<bool>,
    pub obs_dir: Mutex<Option<PathBuf>>,
    /// Where `--self-test` puts its zip by default (None: fallback folder).
    pub desktop_dir: Mutex<Option<PathBuf>>,
    pub applied: Mutex<Vec<Action>>,
}

impl SystemInspector for MockSystem {
    fn windows_build(&self) -> Option<String> {
        Some("mock".into())
    }
    fn windows_audio(&self) -> Option<WindowsAudioState> {
        Some(self.windows.lock().unwrap().clone())
    }
    fn camera_system(&self) -> Option<CameraSystemState> {
        Some(self.camera.clone())
    }
    fn processing_apps(&self) -> KnownProcessingApps {
        self.processing.clone()
    }
    fn vendor_camera_apps(&self) -> Vec<String> {
        self.camera.vendor_apps_running.clone()
    }
    fn hardware(&self) -> Option<HardwareInfo> {
        Some(HardwareInfo { gpu_vendor: GpuVendor::Nvidia, gpu_model: "Mock RTX".into(), has_nvenc: true, cpu_cores: 8, ..Default::default() })
    }
    fn network(&self) -> Option<NetworkInfo> {
        Some(NetworkInfo::default())
    }
    fn mains(&self) -> MainsHz {
        MainsHz::Hz50
    }
    fn obs_running(&self) -> bool {
        *self.obs_running.lock().unwrap()
    }
    fn obs_config_dir(&self) -> Option<PathBuf> {
        self.obs_dir.lock().unwrap().clone()
    }
    fn apply(&self, action: &Action) -> Result<(), String> {
        let mut w = self.windows.lock().unwrap();
        match action {
            Action::SetWindowsDucking { preference } => w.ducking = *preference,
            Action::SetEndpointVolume { endpoint, volume } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.endpoint_volume = *volume;
                }
            }
            Action::DisableEnhancements { endpoint } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.enhancements_enabled = false;
                }
            }
            Action::SetMicBoost { endpoint, db } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.mic_boost_db = Some(*db);
                }
            }
            Action::DisableVoiceClarity { endpoint } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.voice_clarity_enabled = false;
                }
            }
            Action::DisableListen { endpoint } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.listen_to_device = false;
                }
            }
            Action::DisableExclusiveMode { endpoint } => {
                if let Some(e) = w.endpoints.iter_mut().find(|e| &e.id == endpoint) {
                    e.exclusive_mode_allowed = false;
                }
            }
            Action::DisableUsbSelectiveSuspend => w.usb_selective_suspend_enabled = false,
            other => return Err(format!("mock can't apply {other:?}")),
        }
        self.applied.lock().unwrap().push(action.clone());
        Ok(())
    }
    fn endpoint_gain_db(&self, endpoint_id: &str) -> Option<GainControl> {
        let w = self.windows.lock().unwrap();
        (self.mic_gain_adjustable && w.endpoints.iter().any(|e| e.id == endpoint_id)).then(|| GainControl {
            current_db: *self.mic_gain_db.lock().unwrap(),
            min_db: self.mic_gain_range.0,
            max_db: self.mic_gain_range.1,
        })
    }
    fn set_endpoint_gain_db(&self, endpoint_id: &str, db: f64) -> Result<(), String> {
        if self.endpoint_gain_db(endpoint_id).is_none() {
            return Err("This mic's level can't be changed from here.".into());
        }
        *self.mic_gain_db.lock().unwrap() = db.clamp(self.mic_gain_range.0, self.mic_gain_range.1);
        Ok(())
    }
    fn open_target(&self, target: &crate::types::OpenTarget) -> Result<(), String> {
        self.opened.lock().unwrap().push(target.clone());
        Ok(())
    }
    fn desktop_dir(&self) -> Option<PathBuf> {
        self.desktop_dir.lock().unwrap().clone()
    }
    fn read_setting(&self, key: &WinKey) -> Option<WinValue> {
        use serde_json::json;
        let w = self.windows.lock().unwrap();
        let ep = |id: &str| w.endpoints.iter().find(|e| e.id == id);
        let value = match key {
            WinKey::Ducking => json!(ducking_raw(w.ducking)),
            WinKey::UsbSelectiveSuspend => {
                let v = u32::from(w.usb_selective_suspend_enabled);
                json!([v, v])
            }
            WinKey::EndpointVolume { endpoint } => json!(ep(endpoint)?.endpoint_volume),
            WinKey::Enhancements { endpoint } => json!(u32::from(!ep(endpoint)?.enhancements_enabled)),
            WinKey::VoiceClarity { endpoint } => json!(u32::from(ep(endpoint)?.voice_clarity_enabled)),
            WinKey::Listen { endpoint } => json!(u32::from(ep(endpoint)?.listen_to_device)),
            WinKey::ExclusiveMode { endpoint } => json!(u32::from(ep(endpoint)?.exclusive_mode_allowed)),
            WinKey::MicBoost { endpoint } => json!(ep(endpoint)?.mic_boost_db),
        };
        Some(WinValue { key: key.clone(), value })
    }
    fn restore_setting(&self, v: &WinValue) -> Result<(), String> {
        let mut w = self.windows.lock().unwrap();
        let flag = v.value.as_u64().unwrap_or(0) != 0;
        let gone = || "That microphone isn't connected any more.".to_string();
        match &v.key {
            WinKey::Ducking => w.ducking = ducking_from_raw(v.value.as_u64().map(|x| x as u32)),
            WinKey::UsbSelectiveSuspend => w.usb_selective_suspend_enabled = v.value[0].as_u64().unwrap_or(1) != 0,
            WinKey::EndpointVolume { endpoint }
            | WinKey::Enhancements { endpoint }
            | WinKey::VoiceClarity { endpoint }
            | WinKey::Listen { endpoint }
            | WinKey::ExclusiveMode { endpoint }
            | WinKey::MicBoost { endpoint } => {
                let e = w.endpoints.iter_mut().find(|e| &e.id == endpoint).ok_or_else(gone)?;
                match &v.key {
                    WinKey::EndpointVolume { .. } => e.endpoint_volume = v.value.as_f64().unwrap_or(1.0) as f32,
                    WinKey::Enhancements { .. } => e.enhancements_enabled = !flag,
                    WinKey::VoiceClarity { .. } => e.voice_clarity_enabled = flag,
                    WinKey::Listen { .. } => e.listen_to_device = flag,
                    WinKey::ExclusiveMode { .. } => e.exclusive_mode_allowed = flag,
                    _ => e.mic_boost_db = v.value.as_f64().map(|x| x as f32),
                }
            }
        }
        Ok(())
    }
}

/// `UserDuckingPreference` registry value for a preference.
pub fn ducking_raw(p: DuckingPreference) -> u32 {
    match p {
        DuckingPreference::MuteOthers => 0,
        DuckingPreference::Reduce80 => 1,
        DuckingPreference::Reduce50 => 2,
        DuckingPreference::DoNothing => 3,
    }
}

/// Registry value → preference (absent = Windows' default, reduce by 80 %).
pub fn ducking_from_raw(v: Option<u32>) -> DuckingPreference {
    match v {
        Some(0) => DuckingPreference::MuteOthers,
        Some(2) => DuckingPreference::Reduce50,
        Some(3) => DuckingPreference::DoNothing,
        _ => DuckingPreference::Reduce80,
    }
}

pub struct MockCamera {
    pub devices: Vec<CameraDevice>,
    pub state: Mutex<CameraState>,
    /// Brightness of the synthetic scene (0..1); low = dark room.
    pub light: f64,
    pub frame_counter: Mutex<u64>,
}

/// Face box used by the synthetic frames (on a 320×240 frame).
pub const MOCK_FACE: Rect = Rect { x: 120, y: 50, w: 80, h: 110 };

impl CameraControl for MockCamera {
    fn list(&self) -> Vec<CameraDevice> {
        self.devices.clone()
    }
    fn read_state(&self, id: &str) -> Result<CameraState, String> {
        if !self.devices.iter().any(|d| d.id == id) {
            return Err("Camera not found.".into());
        }
        Ok(self.state.lock().unwrap().clone())
    }
    fn apply(&self, _id: &str, uvc: &UvcSettings) -> Result<Vec<String>, String> {
        let mut s = self.state.lock().unwrap();
        s.auto_exposure = uvc.auto_exposure;
        s.ae_priority = uvc.ae_priority;
        s.auto_wb = uvc.auto_wb;
        s.auto_focus = uvc.auto_focus;
        if uvc.exposure_log2.is_some() {
            s.exposure_log2 = uvc.exposure_log2;
        }
        if uvc.wb_kelvin.is_some() {
            s.wb_kelvin = uvc.wb_kelvin;
        }
        if uvc.gain.is_some() {
            s.gain = uvc.gain;
        }
        if uvc.powerline_hz.is_some() {
            s.powerline_hz = uvc.powerline_hz;
        }
        Ok(Vec::new())
    }
    fn grab_frame(&self, _id: &str) -> Result<Frame, String> {
        let mut c = self.frame_counter.lock().unwrap();
        *c += 1;
        Ok(synthetic_frame(self.light, *c as f64 / 30.0))
    }
    fn detect_face(&self, frame: &Frame) -> Option<Rect> {
        (frame.width == 320 && frame.height == 240).then_some(MOCK_FACE)
    }
}

/// A 320×240 scene: grey wall, warm face ellipse, white paper held beside
/// the face inside the on-screen guide box.
pub fn synthetic_frame(light: f64, t: f64) -> Frame {
    let (w, h) = (320usize, 240usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    let k = light.clamp(0.05, 1.0);
    for y in 0..h {
        for x in 0..w {
            let fx = (x as f64 - 160.0) / 40.0;
            let fy = (y as f64 - 105.0) / 55.0;
            let face = fx * fx + fy * fy <= 1.0;
            let paper = (211..300).contains(&x) && (84..180).contains(&y);
            let px = if paper {
                [225.0, 225.0, 225.0]
            } else if face {
                [200.0, 150.0, 125.0]
            } else {
                [110.0, 112.0, 115.0]
            };
            let n = ((x * 31 + y * 17) % 7) as f64 - 3.0;
            for v in px {
                rgb.push(((v * k) + n).clamp(0.0, 255.0) as u8);
            }
        }
    }
    Frame { width: w, height: h, rgb, timestamp_s: t }
}

/// A complete mock world for demos and tests.
pub struct MockPlatform {
    pub audio: Arc<MockAudio>,
    pub system: Arc<MockSystem>,
    pub camera: Arc<MockCamera>,
}

impl MockPlatform {
    /// A typical "out of the box" setup with a few fixable problems.
    pub fn demo() -> Self {
        let mic = AudioDevice {
            id: "{0.0.1.00000000}.{mock-yeti}".into(),
            name: "Microphone (Yeti Stereo Microphone)".into(),
            is_default: true,
            is_default_comms: true,
            channels: 2,
            sample_rate: 48_000,
            usb: None,
        };
        let windows = WindowsAudioState {
            endpoints: vec![AudioEndpoint {
                id: mic.id.clone(),
                name: mic.name.clone(),
                is_default: true,
                is_default_comms: true,
                sample_rate: 44_100,
                channels: 2,
                enhancements_enabled: true,
                endpoint_volume: 0.6,
                ..Default::default()
            }],
            ducking: DuckingPreference::Reduce80,
            mic_privacy_desktop_allowed: true,
            usb_selective_suspend_enabled: true,
            level_changed_by_other_app: false,
        };
        MockPlatform {
            audio: Arc::new(MockAudio { inputs: vec![mic], recording: Mutex::new(None), played: Mutex::new(0), live: Mutex::new(None) }),
            system: Arc::new(MockSystem {
                mic_gain_db: Arc::new(Mutex::new(0.0)),
                mic_gain_range: (-10.0, 30.0),
                mic_gain_adjustable: true,
                opened: Mutex::new(Vec::new()),
                windows: Mutex::new(windows),
                camera: CameraSystemState::default(),
                processing: KnownProcessingApps::default(),
                obs_running: Mutex::new(true),
                obs_dir: Mutex::new(None),
                desktop_dir: Mutex::new(None),
                applied: Mutex::new(Vec::new()),
            }),
            camera: Arc::new(MockCamera {
                devices: vec![CameraDevice { id: "mock-c920".into(), name: "HD Pro Webcam C920".into(), usb: Some((0x046d, 0x082d)) }],
                // Out of the box: auto everything, anti-flicker set for the wrong region.
                state: Mutex::new(CameraState { powerline_hz: Some(60), ..CameraState::default() }),
                light: 0.9,
                frame_counter: Mutex::new(0),
            }),
        }
    }

    pub fn platform(&self) -> Platform {
        Platform { audio: self.audio.clone(), system: self.system.clone(), camera: self.camera.clone() }
    }
}
