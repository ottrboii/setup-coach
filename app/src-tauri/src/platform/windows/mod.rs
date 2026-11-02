//! Real Windows implementations (WASAPI/cpal, MMDevice properties, registry,
//! Media Foundation + DirectShow camera controls, WinRT face detection).
//! Every probe is best-effort: unknown → None/default, never a panic.

mod audio;
mod camera;
mod system;

use super::Platform;
use std::sync::Arc;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Join the multithreaded COM apartment on the current thread (idempotent).
pub(crate) fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// Run `cmd` hidden (no console flash) and return stdout.
pub(crate) fn run_hidden(cmd: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(cmd).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Lower-cased names of running processes (`tasklist`).
pub(crate) fn running_processes() -> Vec<String> {
    run_hidden("tasklist", &["/FO", "CSV", "/NH"])
        .map(|t| t.lines().filter_map(|l| l.split(',').next()).map(|n| n.trim_matches('"').to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

pub fn platform() -> Platform {
    Platform { audio: Arc::new(audio::WinAudio), system: Arc::new(system::WinSystem), camera: Arc::new(camera::WinCamera::new()) }
}
