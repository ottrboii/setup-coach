//! Windows audio/camera/system state for the audit, and Windows-side fixes.

use super::audio::capture_endpoints;
use super::{com_init, run_hidden, running_processes};
use crate::platform::{mains_for_country, GainControl, SystemInspector, WinKey, WinValue};
use coach_core::system::{
    Action, AudioEndpoint, CameraSystemState, Connection, DuckingPreference, GpuVendor, HardwareInfo, KnownProcessingApps, NetworkInfo,
    WindowsAudioState,
};
use coach_core::video::MainsHz;
use std::path::PathBuf;
use windows::core::GUID;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IAudioEffectsManager, IMMDevice, AUDIO_EFFECT, AUDIO_EFFECT_STATE_OFF, AUDIO_EFFECT_STATE_ON};
use windows::Win32::Media::KernelStreaming::AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToUInt32, PROPVARIANT};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL, STGM_READ, STGM_READWRITE};
use windows::Win32::System::Variant::VT_UI4;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};
use winreg::RegKey;

pub struct WinSystem;

/// `PKEY_AudioEndpoint_Disable_SysFx` (1 = enhancements off).
const PKEY_DISABLE_SYSFX: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e), pid: 5 };
/// Registry property names under MMDevices\…\Properties.
const PROP_LISTEN: &str = "{24dbb0fc-9311-4b3d-9cf0-18ff155639d4},1";
const PROP_EXCLUSIVE: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},3";

const USB_SUBGROUP: &str = "2a737441-1930-4402-8d77-b2bebba308a3";
const USB_SUSPEND: &str = "48e6b7a6-50f5-4782-a5d4-53bb8f07e226";

fn endpoint_guid(id: &str) -> Option<&str> {
    id.rsplit('.').next()
}

fn props_key(id: &str, write: bool) -> Option<RegKey> {
    let guid = endpoint_guid(id)?;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\Properties"),
            if write { KEY_READ | KEY_WRITE } else { KEY_READ },
        )
        .ok()
}

/// Serialized VT_BOOL property: bytes 0..2 = VT (0x000b), value at 8..10.
fn reg_bool(id: &str, name: &str) -> Option<bool> {
    let k = props_key(id, false)?;
    let v = k.get_raw_value(name).ok()?;
    let b = v.bytes;
    if b.len() >= 10 && b[0] == 0x0b {
        Some(b[8] != 0 || b[9] != 0)
    } else {
        None
    }
}

fn set_reg_bool(id: &str, name: &str, on: bool) -> Result<(), String> {
    let k = props_key(id, true).ok_or_else(admin_msg)?;
    let mut v = k.get_raw_value(name).map_err(|_| admin_msg())?;
    if v.bytes.len() < 10 {
        return Err("This setting isn't available for your mic.".into());
    }
    let x = if on { 0xff } else { 0x00 };
    v.bytes[8] = x;
    v.bytes[9] = x;
    k.set_raw_value(name, &v).map_err(|_| admin_msg())
}

fn admin_msg() -> String {
    "Windows needs administrator permission for this one. Restart Setup Coach as administrator, or change it in Sound settings.".into()
}

fn sysfx_disabled(d: &IMMDevice) -> Option<bool> {
    unsafe {
        let store = d.OpenPropertyStore(STGM_READ).ok()?;
        let mut pv = store.GetValue(&PKEY_DISABLE_SYSFX).ok()?;
        let v = PropVariantToUInt32(&pv).ok();
        let _ = PropVariantClear(&mut pv);
        v.map(|v| v != 0)
    }
}

fn endpoint_volume(d: &IMMDevice) -> Option<f32> {
    unsafe { d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()?.GetMasterVolumeLevelScalar().ok() }
}

/// Windows 11 "Voice Clarity" (deep noise suppression) state.
fn voice_clarity(d: &IMMDevice) -> Option<bool> {
    unsafe {
        let m: IAudioEffectsManager = d.Activate(CLSCTX_ALL, None).ok()?;
        let mut ptr: *mut AUDIO_EFFECT = std::ptr::null_mut();
        let mut n = 0u32;
        m.GetAudioEffects(&mut ptr, &mut n).ok()?;
        let effects = std::slice::from_raw_parts(ptr, n as usize);
        let on = effects.iter().find(|e| e.id == AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION).map(|e| e.state == AUDIO_EFFECT_STATE_ON);
        CoTaskMemFree(Some(ptr as _));
        on
    }
}

fn find_device(id: &str) -> Option<IMMDevice> {
    capture_endpoints().into_iter().find(|(_, d)| d.id == id).map(|(dev, _)| dev)
}

fn reg_u32(root: winreg::HKEY, path: &str, name: &str) -> Option<u32> {
    RegKey::predef(root).open_subkey(path).ok()?.get_value::<u32, _>(name).ok()
}

fn reg_str(root: winreg::HKEY, path: &str, name: &str) -> Option<String> {
    RegKey::predef(root).open_subkey(path).ok()?.get_value::<String, _>(name).ok()
}

fn consent_allowed(cap: &str) -> bool {
    let base = format!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{cap}");
    let deny = |root, p: &str| reg_str(root, p, "Value").is_some_and(|v| v.eq_ignore_ascii_case("Deny"));
    !(deny(HKEY_LOCAL_MACHINE, &base) || deny(HKEY_CURRENT_USER, &base) || deny(HKEY_CURRENT_USER, &format!(r"{base}\NonPackaged")))
}

const DUCKING_KEY: &str = r"Software\Microsoft\Multimedia\Audio";

/// Current AC and DC indexes of the USB selective suspend power setting.
fn usb_suspend_indexes() -> Option<[u32; 2]> {
    let t = run_hidden("powercfg", &["/query", "SCHEME_CURRENT", USB_SUBGROUP, USB_SUSPEND])?;
    let idx = |tag: &str| {
        t.lines()
            .find(|l| l.contains(tag))
            .and_then(|l| l.rsplit("0x").next())
            .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
    };
    Some([idx("AC Power Setting Index")?, idx("DC Power Setting Index").unwrap_or(1)])
}

fn usb_suspend_enabled() -> bool {
    run_hidden("powercfg", &["/query", "SCHEME_CURRENT", USB_SUBGROUP, USB_SUSPEND])
        .and_then(|t| t.lines().find(|l| l.contains("AC Power Setting Index")).map(|l| !l.trim_end().ends_with("0x00000000")))
        .unwrap_or(false)
}

impl SystemInspector for WinSystem {
    fn windows_build(&self) -> Option<String> {
        let p = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let build = reg_str(HKEY_LOCAL_MACHINE, p, "CurrentBuild")?;
        let ubr = reg_u32(HKEY_LOCAL_MACHINE, p, "UBR").map(|u| format!(".{u}")).unwrap_or_default();
        let disp = reg_str(HKEY_LOCAL_MACHINE, p, "DisplayVersion").map(|d| format!(" ({d})")).unwrap_or_default();
        Some(format!("{build}{ubr}{disp}"))
    }

    fn windows_audio(&self) -> Option<WindowsAudioState> {
        let endpoints = capture_endpoints()
            .into_iter()
            .map(|(dev, d)| {
                let lname = d.name.to_ascii_lowercase();
                AudioEndpoint {
                    is_capture: true,
                    is_default: d.is_default,
                    is_default_comms: d.is_default_comms,
                    is_bluetooth_handsfree: lname.contains("hands-free") || lname.contains("handsfree"),
                    sample_rate: d.sample_rate,
                    channels: d.channels,
                    enhancements_enabled: sysfx_disabled(&dev).map(|off| !off).unwrap_or(false),
                    voice_clarity_enabled: voice_clarity(&dev).unwrap_or(false),
                    mic_boost_db: None, // needs IDeviceTopology walking; not probed yet
                    is_onboard: lname.contains("realtek") || lname.contains("high definition audio"),
                    endpoint_volume: endpoint_volume(&dev).unwrap_or(1.0),
                    exclusive_mode_allowed: reg_bool(&d.id, PROP_EXCLUSIVE).unwrap_or(true),
                    listen_to_device: reg_bool(&d.id, PROP_LISTEN).unwrap_or(false),
                    id: d.id,
                    name: d.name,
                }
            })
            .collect();
        let ducking = reg_u32(HKEY_CURRENT_USER, r"Software\Microsoft\Multimedia\Audio", "UserDuckingPreference")
            .map(DuckingPreference::from_registry)
            .unwrap_or(DuckingPreference::Reduce80);
        Some(WindowsAudioState {
            endpoints,
            ducking,
            mic_privacy_desktop_allowed: consent_allowed("microphone"),
            usb_selective_suspend_enabled: usb_suspend_enabled(),
            level_changed_by_other_app: false,
        })
    }

    fn camera_system(&self) -> Option<CameraSystemState> {
        let frame_server = run_hidden("sc", &["query", "FrameServer"]).map(|t| t.contains("RUNNING") || !t.contains("STATE")).unwrap_or(true);
        Some(CameraSystemState {
            privacy_allowed: consent_allowed("webcam"),
            frame_server_running: frame_server,
            studio_effects_on: false, // per-camera Studio Effects state isn't exposed to desktop apps
            multi_app_camera_on: false,
            vendor_apps_running: self.vendor_camera_apps(),
        })
    }

    fn processing_apps(&self) -> KnownProcessingApps {
        let p = running_processes();
        let has = |n: &str| p.iter().any(|x| x == n);
        KnownProcessingApps {
            ghub_blue_voice_active: has("lghub.exe") || has("lghub_agent.exe"),
            wave_link_fx_active: has("wavelink.exe") || has("wave link.exe"),
            goxlr_app_running: has("goxlr app.exe") || has("goxlrapp.exe"),
            goxlr_utility_running: has("goxlr-daemon.exe") || has("goxlr-launcher.exe"),
            nvidia_broadcast_mic_active: has("nvidia broadcast.exe") || has("nvidiabroadcast.exe"),
            krisp_active: has("krisp.exe"),
            sonar_mic_active: has("steelseriessonar.exe") || has("steelseriesgg.exe"),
        }
    }

    fn vendor_camera_apps(&self) -> Vec<String> {
        let p = running_processes();
        [
            ("lghub.exe", "Logitech G HUB"),
            ("logitune.exe", "Logi Tune"),
            ("camera hub.exe", "Elgato Camera Hub"),
            ("camerahub.exe", "Elgato Camera Hub"),
            ("razer synapse.exe", "Razer Synapse"),
            ("razer synapse 3.exe", "Razer Synapse"),
            ("obsbot_center.exe", "OBSBOT Center"),
            ("insta360 link controller.exe", "Insta360 Link Controller"),
        ]
        .iter()
        .filter(|(exe, _)| p.iter().any(|x| x == exe))
        .map(|(_, n)| n.to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
    }

    fn hardware(&self) -> Option<HardwareInfo> {
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
        let mut best: Option<(GpuVendor, String, u64)> = None;
        unsafe {
            let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut i = 0;
            while let Ok(a) = f.EnumAdapters1(i) {
                i += 1;
                let Ok(d) = a.GetDesc1() else { continue };
                if (d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                    continue;
                }
                let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string();
                let vendor = match d.VendorId {
                    0x10DE => GpuVendor::Nvidia,
                    0x1002 | 0x1022 => GpuVendor::Amd,
                    0x8086 => GpuVendor::Intel,
                    _ => GpuVendor::Unknown,
                };
                let mem = d.DedicatedVideoMemory as u64;
                if best.as_ref().is_none_or(|b| mem > b.2) {
                    best = Some((vendor, name, mem));
                }
            }
        }
        let (vendor, model, _) = best.unwrap_or((GpuVendor::Unknown, String::new(), 0));
        Some(HardwareInfo {
            gpu_vendor: vendor,
            has_nvenc: vendor == GpuVendor::Nvidia && !model.contains("GT 7") && !model.contains("GT 6"),
            has_amf: vendor == GpuVendor::Amd,
            has_qsv: vendor == GpuVendor::Intel,
            gpu_model: model,
            cpu_cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(0),
        })
    }

    fn network(&self) -> Option<NetworkInfo> {
        use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IP_ADAPTER_ADDRESSES_LH};
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
        use windows::Win32::Networking::WinSock::AF_UNSPEC;
        unsafe {
            let mut size = 16 * 1024u32;
            let mut buf = vec![0u8; size as usize];
            let mut rc = GetAdaptersAddresses(AF_UNSPEC.0 as u32, GAA_FLAG_INCLUDE_GATEWAYS, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size);
            if rc == 111 {
                // ERROR_BUFFER_OVERFLOW
                buf = vec![0u8; size as usize];
                rc = GetAdaptersAddresses(AF_UNSPEC.0 as u32, GAA_FLAG_INCLUDE_GATEWAYS, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size);
            }
            if rc != 0 {
                return Some(NetworkInfo::default());
            }
            let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
            let mut conn = Connection::Unknown;
            while !p.is_null() {
                let a = &*p;
                if a.OperStatus == IfOperStatusUp && !a.FirstGatewayAddress.is_null() {
                    match a.IfType {
                        71 => conn = Connection::Wifi,
                        6 if conn != Connection::Wifi => conn = Connection::Wired,
                        _ => {}
                    }
                }
                p = a.Next;
            }
            Some(NetworkInfo { connection: conn, measured_upload_kbps: None })
        }
    }

    fn mains(&self) -> MainsHz {
        reg_str(HKEY_CURRENT_USER, r"Control Panel\International\Geo", "Name").map(|c| mains_for_country(&c)).unwrap_or(MainsHz::Hz50)
    }

    fn obs_running(&self) -> bool {
        running_processes().iter().any(|p| p == "obs64.exe" || p == "obs32.exe" || p == "obs.exe")
    }

    fn obs_config_dir(&self) -> Option<PathBuf> {
        crate::paths::obs_dir()
    }

    fn desktop_dir(&self) -> Option<PathBuf> {
        super::desktop_dir()
    }

    fn can_apply(&self, action: &Action) -> bool {
        match action {
            // Registry-backed endpoint settings: the exact key must open for writing.
            Action::DisableExclusiveMode { endpoint } | Action::DisableListen { endpoint } => props_key(endpoint, true).is_some(),
            // Property-store / audio-effects / boost changes need the same rights; probe the shared parent key.
            Action::DisableEnhancements { endpoint } | Action::DisableVoiceClarity { endpoint } | Action::SetMicBoost { endpoint, .. } => {
                props_key(endpoint, true).is_some()
            }
            _ => true,
        }
    }

    fn apply(&self, action: &Action) -> Result<(), String> {
        com_init();
        match action {
            Action::SetWindowsDucking { preference } => {
                let v: u32 = match preference {
                    DuckingPreference::MuteOthers => 0,
                    DuckingPreference::Reduce80 => 1,
                    DuckingPreference::Reduce50 => 2,
                    DuckingPreference::DoNothing => 3,
                };
                let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Microsoft\Multimedia\Audio").map_err(|e| e.to_string())?;
                k.set_value("UserDuckingPreference", &v).map_err(|e| e.to_string())
            }
            Action::SetEndpointVolume { endpoint, volume } => unsafe {
                let d = find_device(endpoint).ok_or("That microphone isn't connected any more.")?;
                let v: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
                v.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null()).map_err(|e| e.to_string())
            },
            Action::DisableEnhancements { endpoint } => unsafe {
                let d = find_device(endpoint).ok_or("That microphone isn't connected any more.")?;
                let store = d.OpenPropertyStore(STGM_READWRITE).map_err(|_| admin_msg())?;
                let mut pv = PROPVARIANT::default();
                (*pv.Anonymous.Anonymous).vt = VT_UI4;
                (*pv.Anonymous.Anonymous).Anonymous.ulVal = 1;
                store.SetValue(&PKEY_DISABLE_SYSFX, &pv).map_err(|_| admin_msg())?;
                store.Commit().map_err(|_| admin_msg())
            },
            Action::DisableVoiceClarity { endpoint } => unsafe {
                let d = find_device(endpoint).ok_or("That microphone isn't connected any more.")?;
                let m: IAudioEffectsManager = d.Activate(CLSCTX_ALL, None).map_err(|_| "Voice Clarity can't be changed on this PC. Turn it off in Settings → System → Sound → your mic.".to_string())?;
                m.SetAudioEffectState(AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION, AUDIO_EFFECT_STATE_OFF)
                    .map_err(|_| "Turn off Voice Clarity in Settings → System → Sound → your mic → Audio enhancements.".to_string())
            },
            Action::DisableListen { endpoint } => set_reg_bool(endpoint, PROP_LISTEN, false),
            Action::DisableExclusiveMode { endpoint } => set_reg_bool(endpoint, PROP_EXCLUSIVE, false),
            Action::DisableUsbSelectiveSuspend => {
                for sub in ["/setacvalueindex", "/setdcvalueindex"] {
                    run_hidden("powercfg", &[sub, "SCHEME_CURRENT", USB_SUBGROUP, USB_SUSPEND, "0"]).ok_or("Couldn't change the power plan.")?;
                }
                run_hidden("powercfg", &["/setactive", "SCHEME_CURRENT"]).ok_or("Couldn't change the power plan.")?;
                Ok(())
            }
            Action::SetMicBoost { db, .. } => Err(format!(
                "Set Microphone Boost to {db:.0} dB: Control Panel → Sound → Recording → your mic → Properties → Levels."
            )),
            other => Err(format!("This fix needs OBS: {other:?}")),
        }
    }

    fn read_setting(&self, key: &WinKey) -> Option<WinValue> {
        use serde_json::json;
        com_init();
        let value = match key {
            WinKey::Ducking => json!(reg_u32(HKEY_CURRENT_USER, DUCKING_KEY, "UserDuckingPreference")),
            WinKey::UsbSelectiveSuspend => json!(usb_suspend_indexes()?),
            WinKey::EndpointVolume { endpoint } => json!(endpoint_volume(&find_device(endpoint)?)?),
            WinKey::Enhancements { endpoint } => json!(u32::from(sysfx_disabled(&find_device(endpoint)?).unwrap_or(false))),
            WinKey::VoiceClarity { endpoint } => json!(u32::from(voice_clarity(&find_device(endpoint)?)?)),
            WinKey::Listen { endpoint } => json!(u32::from(reg_bool(endpoint, PROP_LISTEN)?)),
            WinKey::ExclusiveMode { endpoint } => json!(u32::from(reg_bool(endpoint, PROP_EXCLUSIVE)?)),
            // We never write Microphone Boost (it's coached), so nothing to save.
            WinKey::MicBoost { .. } => return None,
        };
        Some(WinValue { key: key.clone(), value })
    }

    fn restore_setting(&self, v: &WinValue) -> Result<(), String> {
        com_init();
        let gone = || "That microphone isn't connected any more.".to_string();
        let flag = v.value.as_u64().unwrap_or(0) != 0;
        match &v.key {
            WinKey::Ducking => {
                let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(DUCKING_KEY).map_err(|e| e.to_string())?;
                match v.value.as_u64() {
                    Some(x) => k.set_value("UserDuckingPreference", &(x as u32)).map_err(|e| e.to_string()),
                    None => match k.delete_value("UserDuckingPreference") {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                        _ => Ok(()),
                    },
                }
            }
            WinKey::UsbSelectiveSuspend => {
                let ac = v.value[0].as_u64().unwrap_or(1).to_string();
                let dc = v.value[1].as_u64().unwrap_or(1).to_string();
                for (sub, val) in [("/setacvalueindex", &ac), ("/setdcvalueindex", &dc)] {
                    run_hidden("powercfg", &[sub, "SCHEME_CURRENT", USB_SUBGROUP, USB_SUSPEND, val]).ok_or("Couldn't change the power plan.")?;
                }
                run_hidden("powercfg", &["/setactive", "SCHEME_CURRENT"]).ok_or("Couldn't change the power plan.")?;
                Ok(())
            }
            WinKey::EndpointVolume { endpoint } => unsafe {
                let d = find_device(endpoint).ok_or_else(gone)?;
                let ev: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
                ev.SetMasterVolumeLevelScalar((v.value.as_f64().unwrap_or(1.0) as f32).clamp(0.0, 1.0), std::ptr::null()).map_err(|e| e.to_string())
            },
            WinKey::Enhancements { endpoint } => unsafe {
                let d = find_device(endpoint).ok_or_else(gone)?;
                let store = d.OpenPropertyStore(STGM_READWRITE).map_err(|_| admin_msg())?;
                let mut pv = PROPVARIANT::default();
                (*pv.Anonymous.Anonymous).vt = VT_UI4;
                (*pv.Anonymous.Anonymous).Anonymous.ulVal = u32::from(flag);
                store.SetValue(&PKEY_DISABLE_SYSFX, &pv).map_err(|_| admin_msg())?;
                store.Commit().map_err(|_| admin_msg())
            },
            WinKey::VoiceClarity { endpoint } => unsafe {
                let d = find_device(endpoint).ok_or_else(gone)?;
                let m: IAudioEffectsManager = d.Activate(CLSCTX_ALL, None).map_err(|_| "Voice Clarity can't be changed on this PC.".to_string())?;
                m.SetAudioEffectState(AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION, if flag { AUDIO_EFFECT_STATE_ON } else { AUDIO_EFFECT_STATE_OFF })
                    .map_err(|_| "Windows didn't let us change Voice Clarity back. Change it in Settings → System → Sound → your mic.".to_string())
            },
            WinKey::Listen { endpoint } => set_reg_bool(endpoint, PROP_LISTEN, flag),
            WinKey::ExclusiveMode { endpoint } => set_reg_bool(endpoint, PROP_EXCLUSIVE, flag),
            WinKey::MicBoost { .. } => Ok(()),
        }
    }

    fn endpoint_gain_db(&self, endpoint_id: &str) -> Option<GainControl> {
        com_init();
        let d = find_device(endpoint_id)?;
        unsafe {
            let v: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None).ok()?;
            let cur = v.GetMasterVolumeLevel().ok()?;
            let (mut min, mut max, mut step) = (0f32, 0f32, 0f32);
            v.GetVolumeRange(&mut min, &mut max, &mut step).ok()?;
            (max > min).then_some(GainControl { current_db: cur as f64, min_db: min as f64, max_db: max as f64 })
        }
    }

    fn set_endpoint_gain_db(&self, endpoint_id: &str, db: f64) -> Result<(), String> {
        com_init();
        let d = find_device(endpoint_id).ok_or("That microphone isn't connected any more.")?;
        unsafe {
            let v: IAudioEndpointVolume = d.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
            let (mut min, mut max, mut step) = (0f32, 0f32, 0f32);
            v.GetVolumeRange(&mut min, &mut max, &mut step).map_err(|e| e.to_string())?;
            v.SetMasterVolumeLevel((db as f32).clamp(min, max), std::ptr::null()).map_err(|e| e.to_string())
        }
    }

    fn open_target(&self, target: &crate::types::OpenTarget) -> Result<(), String> {
        // `start` handles ms-settings: URIs, URLs, protocols and installed apps.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let uri = target.uri_or_app.trim();
        if uri.is_empty() || uri.contains(['&', '|', '<', '>', '^', '"']) {
            return Err(format!("Open {} yourself, then come back here.", target.label));
        }
        std::process::Command::new("cmd")
            .args(["/C", "start", "", uri])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|_| format!("Couldn't open {}. Open it yourself, then come back here.", target.label))
    }
}

