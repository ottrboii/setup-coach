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
}

struct NoopHandle;
impl StreamHandle for NoopHandle {
    fn stop(self: Box<Self>) {}
}

impl AudioBackend for MockAudio {
    fn list_inputs(&self) -> Vec<AudioDevice> {
        self.inputs.clone()
    }

    fn start_capture(&self, device_id: &str, mut sink: SampleSink) -> Result<Box<dyn StreamHandle>, String> {
        if !self.inputs.iter().any(|d| d.id == device_id) {
            return Err("That microphone isn't connected any more.".into());
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
    pub windows: Mutex<WindowsAudioState>,
    pub camera: CameraSystemState,
    pub processing: KnownProcessingApps,
    pub obs_running: Mutex<bool>,
    pub obs_dir: Mutex<Option<PathBuf>>,
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
            Action::DisableUsbSelectiveSuspend => w.usb_selective_suspend_enabled = false,
            other => return Err(format!("mock can't apply {other:?}")),
        }
        self.applied.lock().unwrap().push(action.clone());
        Ok(())
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
            audio: Arc::new(MockAudio { inputs: vec![mic], recording: Mutex::new(None), played: Mutex::new(0) }),
            system: Arc::new(MockSystem {
                windows: Mutex::new(windows),
                camera: CameraSystemState::default(),
                processing: KnownProcessingApps::default(),
                obs_running: Mutex::new(true),
                obs_dir: Mutex::new(None),
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
