//! Windows audio/camera/system state for the audit, and Windows-side fixes.

use super::audio::capture_endpoints;
use super::{com_init, run_hidden, running_processes};
use crate::platform::{detect_mains, parse_usb_probe, FxErr, GainControl, MainsGuess, SystemInspector, WinKey, WinValue};
use coach_core::system::{
    Action, AudioEndpoint, CameraSystemState, Connection, DuckingPreference, GpuVendor, HardwareInfo, KnownProcessingApps, NetworkInfo,
    WindowsAudioState,
};
use std::path::PathBuf;
use windows::core::GUID;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IAudioEffectsManager, IMMDevice, AUDIO_EFFECT, AUDIO_EFFECT_STATE_OFF, AUDIO_EFFECT_STATE_ON};
use windows::Win32::Media::KernelStreaming::AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToUInt32, PROPVARIANT};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL, STGM_READ, STGM_READWRITE};
use windows::Win32::System::Variant::VT_UI4;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD};
use winreg::{RegKey, RegValue};

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

const MMDEVICES_CAPTURE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture";

fn props_key(id: &str, write: bool) -> Option<RegKey> {
    let guid = endpoint_guid(id)?;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            format!(r"{MMDEVICES_CAPTURE}\{guid}\Properties"),
            // Value access only. These keys let administrators set values but not
            // create sub-keys, and KEY_WRITE asks for CREATE_SUB_KEY too, so it is
            // refused even in an elevated process (checked on Windows 11 25H2).
            if write { KEY_QUERY_VALUE | KEY_SET_VALUE } else { KEY_READ },
        )
        .ok()
}

/// A two-state endpoint property, as Windows stores it (see `parse_flag`).
/// `None` = not stored (Windows' default applies) or not readable.
fn reg_bool(id: &str, name: &str) -> Option<bool> {
    let v = props_key(id, false)?.get_raw_value(name).ok()?;
    crate::platform::parse_flag(v.vtype == REG_DWORD, &v.bytes)
}

/// What Windows does when the value is not stored: exclusive control is
/// allowed, "Listen to this device" is off.
fn flag_default(name: &str) -> bool {
    name == PROP_EXCLUSIVE
}

/// The setting as Windows really has it right now (stored value, else its
/// default). `None` only when the mic's settings can't be read at all.
fn flag_now(id: &str, name: &str) -> Option<bool> {
    props_key(id, false)?;
    Some(reg_bool(id, name).unwrap_or_else(|| flag_default(name)))
}

fn set_reg_bool(id: &str, name: &str, on: bool) -> Result<(), FxErr> {
    props_key(id, false).ok_or(FxErr::Gone)?;
    let k = props_key(id, true).ok_or(FxErr::Denied)?;
    let cur = k.get_raw_value(name).ok();
    if cur.is_none() && name != PROP_EXCLUSIVE {
        // Not stored: Windows' default applies. Nothing to do if that is what we want;
        // only exclusive control is a plain number we may create.
        return if flag_default(name) == on { Ok(()) } else { Err(FxErr::Unsupported) };
    }
    let existing = cur.as_ref().map(|v| (v.vtype == REG_DWORD, v.bytes.as_slice()));
    let (is_dword, bytes) = crate::platform::flag_bytes(existing, on).ok_or(FxErr::Unsupported)?;
    k.set_raw_value(name, &RegValue { bytes, vtype: if is_dword { REG_DWORD } else { REG_BINARY } }).map_err(|_| FxErr::Denied)
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

/// Registry name of `PKEY_AudioEndpoint_Disable_SysFx` (Windows keeps a copy
/// in both `Properties` and `FxProperties`; checked on a real Windows 11 PC).
const FX_PROP_NAME: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";

/// The `FxProperties` copy (the one Windows' own Sound settings page shows).
fn fx_copy(endpoint: &str) -> Option<bool> {
    let guid = endpoint_guid(endpoint)?;
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(format!(r"{MMDEVICES_CAPTURE}\{guid}\FxProperties"), KEY_READ).ok()?;
    k.get_value::<u32, _>(FX_PROP_NAME).ok().map(|v| v != 0)
}

/// "Enhancements are off" for a mic, from both copies Windows keeps. Either one
/// saying "on" counts as on.
fn sysfx_off(d: &IMMDevice, endpoint: &str) -> Option<bool> {
    crate::platform::combine_sysfx(sysfx_disabled(d), fx_copy(endpoint))
}

fn read_sysfx(endpoint: &str) -> bool {
    find_device(endpoint).and_then(|d| sysfx_off(&d, endpoint)).unwrap_or(false)
}

/// Set the value on both registry copies (value access only). Ok when at least one took.
fn write_sysfx_registry(endpoint: &str, disabled: bool) -> Result<(), FxErr> {
    let guid = endpoint_guid(endpoint).ok_or(FxErr::Gone)?;
    let mut any = false;
    for sub in ["Properties", "FxProperties"] {
        if let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(format!(r"{MMDEVICES_CAPTURE}\{guid}\{sub}"), KEY_QUERY_VALUE | KEY_SET_VALUE) {
            any |= k.set_value(FX_PROP_NAME, &u32::from(disabled)).is_ok();
        }
    }
    if any {
        Ok(())
    } else {
        Err(FxErr::Denied)
    }
}

/// Write "enhancements off/on" from this process. Windows only lets
/// administrators do this (IMMDevice::OpenPropertyStore: "A client that is not
/// running as an administrator is restricted to read-only access"), so for
/// everyone else this fails and `set_sysfx` falls back to a one-off UAC helper.
/// Both copies are written (property store and registry), because the Sound
/// settings page reads the `FxProperties` one.
pub(crate) fn write_sysfx_in_process(endpoint: &str, disabled: bool) -> Result<(), FxErr> {
    com_init();
    let d = find_device(endpoint).ok_or(FxErr::Gone)?;
    let store_ok = unsafe {
        (|| -> windows::core::Result<()> {
            let store = d.OpenPropertyStore(STGM_READWRITE)?;
            let mut pv = PROPVARIANT::default();
            (*pv.Anonymous.Anonymous).vt = VT_UI4;
            (*pv.Anonymous.Anonymous).Anonymous.ulVal = u32::from(disabled);
            store.SetValue(&PKEY_DISABLE_SYSFX, &pv)?;
            store.Commit()
        })()
        .is_ok()
    };
    let reg = write_sysfx_registry(endpoint, disabled);
    if store_ok || reg.is_ok() {
        Ok(())
    } else {
        Err(FxErr::Denied)
    }
}

/// Re-read until the value shows up (Windows applies it a moment later).
fn wait_for_sysfx(endpoint: &str, disabled: bool) -> bool {
    for _ in 0..12 {
        if read_sysfx(endpoint) == disabled {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Turn Windows "Audio enhancements" off (or back on) for one mic and prove it
/// by reading the value back. Order: write directly (works for admins), else
/// ask Windows for permission (UAC) for just this one change. Only `Ok` when
/// the re-read shows the new value; the error says which step failed.
pub(crate) fn set_sysfx(endpoint: &str, disabled: bool) -> Result<(), String> {
    let direct = write_sysfx_in_process(endpoint, disabled);
    if direct.is_ok() && wait_for_sysfx(endpoint, disabled) {
        return Ok(());
    }
    if read_sysfx(endpoint) == disabled {
        return Ok(());
    }
    if let Err(e @ (FxErr::Gone | FxErr::Unsupported)) = direct {
        return Err(e.message());
    }
    run_elevated_helper(&["sysfx", endpoint, if disabled { "1" } else { "0" }])?;
    if wait_for_sysfx(endpoint, disabled) {
        Ok(())
    } else {
        Err(FxErr::NotShown.message())
    }
}

/// Registry-backed endpoint switches the elevated helper may touch.
fn reg_flag_name(which: &str) -> Option<&'static str> {
    match which {
        "listen" => Some(PROP_LISTEN),
        "exclusive" => Some(PROP_EXCLUSIVE),
        _ => None,
    }
}

fn wait_for_reg_flag(endpoint: &str, name: &str, on: bool) -> bool {
    for _ in 0..12 {
        if flag_now(endpoint, name) == Some(on) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Set "Listen to this device" / "Allow exclusive control" for one mic and
/// prove it by reading it back. Same order as `set_sysfx`: write directly
/// (administrators), else one UAC prompt for just this change.
fn set_reg_flag(which: &str, endpoint: &str, on: bool) -> Result<(), String> {
    let name = reg_flag_name(which).ok_or("Unknown setting.")?;
    let direct = set_reg_bool(endpoint, name, on);
    if direct.is_ok() && wait_for_reg_flag(endpoint, name, on) {
        return Ok(());
    }
    if flag_now(endpoint, name) == Some(on) {
        return Ok(());
    }
    if let Err(e @ (FxErr::Gone | FxErr::Unsupported)) = direct {
        return Err(e.message());
    }
    run_elevated_helper(&["regflag", which, endpoint, if on { "1" } else { "0" }])?;
    if wait_for_reg_flag(endpoint, name, on) {
        Ok(())
    } else {
        Err(FxErr::NotShown.message())
    }
}

/// Run this same exe with administrator rights for one change (Windows shows
/// its normal permission prompt), wait for it, and map its exit code.
fn run_elevated_helper(args: &[&str]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let params: String = std::iter::once("--fx-helper").chain(args.iter().copied()).map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
    let (verb, file, params) = (wide("runas".as_ref()), wide(exe.as_os_str()), wide(params.as_ref()));
    unsafe {
        let mut info = SHELLEXECUTEINFOW { cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32, fMask: SEE_MASK_NOCLOSEPROCESS, ..Default::default() };
        info.lpVerb = PCWSTR(verb.as_ptr());
        info.lpFile = PCWSTR(file.as_ptr());
        info.lpParameters = PCWSTR(params.as_ptr());
        info.nShow = 0; // SW_HIDE
        ShellExecuteExW(&mut info).map_err(|_| "Windows' permission prompt was closed, so nothing was changed.".to_string())?;
        if info.hProcess.is_invalid() {
            return Err("Windows didn't start the permission helper.".into());
        }
        let waited = WaitForSingleObject(info.hProcess, 60_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if waited.0 != 0 {
            // WAIT_OBJECT_0 is 0: anything else means it was still running after 60 s.
            return Err("The permission helper took too long, so nothing was counted as changed.".into());
        }
        if code == 0 {
            Ok(())
        } else {
            Err(FxErr::message_for_exit_code(code))
        }
    }
}

/// `setup-coach-app.exe --fx-helper sysfx <endpoint> <0|1>`: runs elevated,
/// changes exactly one value, exits 0 only if the read-back matches. Other exit
/// codes are `FxErr::code` (3 refused, 4 not shown, 5 mic gone, 6 unsupported)
/// or 2 for a request it doesn't understand.
pub fn fx_helper_main(args: &[String]) -> Option<i32> {
    let i = args.iter().position(|a| a == "--fx-helper")?;
    let rest = &args[i + 1..];
    let code = match rest {
        [what, endpoint, flag] if what == "sysfx" && crate::platform::is_capture_endpoint_id(endpoint) && (flag == "0" || flag == "1") => {
            let disabled = flag == "1";
            match write_sysfx_in_process(endpoint, disabled) {
                Ok(()) if wait_for_sysfx(endpoint, disabled) => 0,
                Ok(()) => FxErr::NotShown.code(),
                Err(e) => e.code(),
            }
        }
        [what, which, endpoint, flag]
            if what == "regflag" && reg_flag_name(which).is_some() && crate::platform::is_capture_endpoint_id(endpoint) && (flag == "0" || flag == "1") =>
        {
            let on = flag == "1";
            let name = reg_flag_name(which).unwrap_or_default();
            match set_reg_bool(endpoint, name, on) {
                Ok(()) if wait_for_reg_flag(endpoint, name, on) => 0,
                Ok(()) => FxErr::NotShown.code(),
                Err(e) => e.code(),
            }
        }
        _ => 2,
    };
    Some(code)
}

fn endpoint_volume(d: &IMMDevice) -> Option<f32> {
    unsafe { d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()?.GetMasterVolumeLevelScalar().ok() }
}

/// Windows 11 "Voice Clarity" (deep noise suppression) state.
fn voice_clarity(d: &IMMDevice) -> Option<bool> {
    voice_clarity_effect(d).map(|(_, on)| on)
}

/// (user can change it, it is on) for the deep-noise-suppression effect.
fn voice_clarity_effect(d: &IMMDevice) -> Option<(bool, bool)> {
    unsafe {
        let m: IAudioEffectsManager = d.Activate(CLSCTX_ALL, None).ok()?;
        let mut ptr: *mut AUDIO_EFFECT = std::ptr::null_mut();
        let mut n = 0u32;
        m.GetAudioEffects(&mut ptr, &mut n).ok()?;
        let effects = std::slice::from_raw_parts(ptr, n as usize);
        let found = effects
            .iter()
            .find(|e| e.id == AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION)
            .map(|e| (e.canSetState.as_bool(), e.state == AUDIO_EFFECT_STATE_ON));
        CoTaskMemFree(Some(ptr as _));
        found
    }
}

/// Switch Voice Clarity and prove it by reading it back.
fn set_voice_clarity(endpoint: &str, on: bool) -> Result<(), String> {
    let d = find_device(endpoint).ok_or("That microphone isn't connected any more.")?;
    let Some((can_set, now)) = voice_clarity_effect(&d) else {
        // No such effect on this mic: nothing to switch off.
        return if on { Err("Voice Clarity isn't available on this mic.".into()) } else { Ok(()) };
    };
    if now == on {
        return Ok(());
    }
    if !can_set {
        return Err("Windows doesn't let apps change Voice Clarity for this mic.".into());
    }
    unsafe {
        let m: IAudioEffectsManager = d.Activate(CLSCTX_ALL, None).map_err(|_| "Windows doesn't let apps change Voice Clarity for this mic.".to_string())?;
        m.SetAudioEffectState(AUDIO_EFFECT_TYPE_DEEP_NOISE_SUPPRESSION, if on { AUDIO_EFFECT_STATE_ON } else { AUDIO_EFFECT_STATE_OFF })
            .map_err(|_| "Windows wouldn't switch Voice Clarity for this mic.".to_string())?;
    }
    for _ in 0..8 {
        if find_device(endpoint).and_then(|d| voice_clarity(&d)) == Some(on) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err("Windows still shows Voice Clarity as on for this mic.".into())
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

/// Apps Windows says are using the webcam right now: the privacy log
/// (Settings > Privacy > Camera) keeps `LastUsedTimeStart` and sets
/// `LastUsedTimeStop` to 0 while the camera is open. Returns raw entry names
/// (exe paths with `#` for `\`, or packaged-app ids).
fn webcam_in_use_now() -> Vec<String> {
    capability_in_use_now("webcam")
}

/// Same for webcam or microphone.
fn capability_in_use_now(cap: &str) -> Vec<String> {
    let base = format!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{cap}");
    let mut out = Vec::new();
    let Ok(root) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(&base, KEY_READ) else { return out };
    let mut scan = |key: &RegKey| {
        for name in key.enum_keys().flatten() {
            if name.eq_ignore_ascii_case("NonPackaged") {
                continue;
            }
            let Ok(k) = key.open_subkey_with_flags(&name, KEY_READ) else { continue };
            let start: u64 = k.get_value("LastUsedTimeStart").unwrap_or(0);
            let stop: u64 = k.get_value("LastUsedTimeStop").unwrap_or(1);
            if start > 0 && stop == 0 {
                out.push(name);
            }
        }
    };
    scan(&root);
    if let Ok(np) = root.open_subkey_with_flags("NonPackaged", KEY_READ) {
        scan(&np);
    }
    out
}

/// The probe takes a few seconds; rescans within 15 s reuse the last answer.
fn usb_links_cached() -> Vec<coach_core::system::UsbLink> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<coach_core::system::UsbLink>)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = g.as_ref() {
        if at.elapsed() < Duration::from_secs(15) {
            return v.clone();
        }
    }
    let v = usb_links();
    *g = Some((Instant::now(), v.clone()));
    v
}

/// Read USB cameras/mics (speed, hubs, drop history) with a short PowerShell probe.
fn usb_links() -> Vec<coach_core::system::UsbLink> {
    use coach_core::system::{UsbLink, UsbRole, UsbSpeed};
    let path = std::env::temp_dir().join("setup-coach-usb-probe.ps1");
    if std::fs::write(&path, include_str!("usb_probe.ps1")).is_err() {
        return Vec::new();
    }
    let Some(text) = run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", &path.to_string_lossy()]) else { return Vec::new() };
    let _ = std::fs::remove_file(&path);
    parse_usb_probe(&text)
        .into_iter()
        .map(|p| UsbLink {
            name: p.name,
            role: if p.role == "camera" { UsbRole::Camera } else { UsbRole::Audio },
            vid: p.vid,
            pid: p.pid,
            speed: match p.speed.as_str() {
                "low" => UsbSpeed::Low,
                "full" => UsbSpeed::Full,
                "high" => UsbSpeed::High,
                "super" => UsbSpeed::Super,
                _ => UsbSpeed::Unknown,
            },
            superspeed_capable: p.superspeed_capable.unwrap_or(false),
            controller: p.controller,
            behind_hub: p.behind_hub,
            hub_name: p.hub_name,
            hub_bus_powered: p.hub_bus_powered,
            drops_7d: p.drops_7d,
            image_only: p.image_only,
            run_minutes: p.run_minutes,
            connected_min: p.connected_min,
            // Filled in from the camera profile / name by the app.
            capture_card: false,
        })
        .collect()
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
                    enhancements_enabled: sysfx_off(&dev, &d.id).map(|off| !off).unwrap_or(false),
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

    fn mains_guess(&self) -> MainsGuess {
        let tz = reg_str(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation", "TimeZoneKeyName");
        let region = reg_str(HKEY_CURRENT_USER, r"Control Panel\International\Geo", "Name");
        detect_mains(tz.as_deref(), region.as_deref())
    }

    fn obs_running(&self) -> bool {
        running_processes().iter().any(|p| p == "obs64.exe" || p == "obs32.exe" || p == "obs.exe")
    }

    fn camera_users(&self) -> Vec<crate::platform::CameraUser> {
        crate::platform::merge_camera_users(&webcam_in_use_now(), &running_processes())
    }

    fn connection_state(&self) -> coach_core::system::ConnectionState {
        use coach_core::system::{ConnectionState, DeviceHolder, HeldDevice};
        let running = running_processes();
        let mut holders = Vec::new();
        for (cap, device) in [("webcam", HeldDevice::Camera), ("microphone", HeldDevice::Mic)] {
            // Mic: only Windows' own "in use right now" log counts; running apps
            // are too common to accuse.
            let running_for = if device == HeldDevice::Camera { running.as_slice() } else { &[] };
            for u in crate::platform::merge_camera_users(&capability_in_use_now(cap), running_for) {
                holders.push(DeviceHolder { app: u.name, device, confirmed: u.confirmed });
            }
        }
        // Maker tools that grab a camera body and stop it acting as a webcam.
        let vendor_tools_running = [("eosutility.exe", "EOS Utility"), ("eosutility3.exe", "EOS Utility"), ("eos utility.exe", "EOS Utility"), ("cameraconnect.exe", "Canon CameraConnect"), ("imagingedge.exe", "Imaging Edge"), ("imaging edge desktop.exe", "Imaging Edge"), ("remote.exe", "Sony Imaging Edge Remote")]
            .iter()
            .filter(|(exe, _)| running.iter().any(|x| x == exe))
            .map(|(_, n)| n.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        ConnectionState { links: usb_links_cached(), holders, vendor_tools_running }
    }

    fn obs_config_dir(&self) -> Option<PathBuf> {
        crate::paths::obs_dir()
    }

    fn desktop_dir(&self) -> Option<PathBuf> {
        super::desktop_dir()
    }

    fn can_apply(&self, action: &Action) -> bool {
        match action {
            // Enhancements / Voice Clarity / Listen / Exclusive: if we aren't an administrator we ask
            // Windows for permission (UAC) for just that change, and fall back to
            // a guided step if it's refused, so always offer them.
            Action::DisableEnhancements { .. } | Action::DisableVoiceClarity { .. } | Action::DisableListen { .. } | Action::DisableExclusiveMode { .. } => true,
            Action::SetMicBoost { endpoint, .. } => props_key(endpoint, true).is_some(),
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
            Action::DisableEnhancements { endpoint } => set_sysfx(endpoint, true),
            Action::DisableVoiceClarity { endpoint } => set_voice_clarity(endpoint, false),
            Action::DisableListen { endpoint } => set_reg_flag("listen", endpoint, false),
            Action::DisableExclusiveMode { endpoint } => set_reg_flag("exclusive", endpoint, false),
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
            WinKey::Enhancements { endpoint } => json!(u32::from(sysfx_off(&find_device(endpoint)?, endpoint).unwrap_or(false))),
            WinKey::VoiceClarity { endpoint } => json!(u32::from(voice_clarity(&find_device(endpoint)?)?)),
            WinKey::Listen { endpoint } => json!(u32::from(flag_now(endpoint, PROP_LISTEN)?)),
            WinKey::ExclusiveMode { endpoint } => json!(u32::from(flag_now(endpoint, PROP_EXCLUSIVE)?)),
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
            WinKey::Enhancements { endpoint } => {
                find_device(endpoint).ok_or_else(gone)?;
                set_sysfx(endpoint, flag)
            }
            WinKey::VoiceClarity { endpoint } => set_voice_clarity(endpoint, flag).map_err(|e| format!("{e} Change it in Settings → System → Sound → your mic.")),
            WinKey::Listen { endpoint } => set_reg_bool(endpoint, PROP_LISTEN, flag).map_err(|e| e.message()),
            WinKey::ExclusiveMode { endpoint } => set_reg_bool(endpoint, PROP_EXCLUSIVE, flag).map_err(|e| e.message()),
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

