//! Real Windows implementations (WASAPI/cpal, MMDevice properties, registry,
//! Media Foundation + DirectShow camera controls, WinRT face detection).
//! Every probe is best-effort: unknown → None/default, never a panic.

mod audio;
mod camera;
mod dialogs;
mod system;
pub use system::fx_helper_main;

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

/// Lower-cased names of running processes. A native snapshot (milliseconds);
/// `tasklist` took 1.5 s+ per call on a real PC and the self-test checks
/// call this several times.
pub(crate) fn running_processes() -> Vec<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut names = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return names };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            names.push(String::from_utf16_lossy(&e.szExeFile[..len]).to_ascii_lowercase());
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    names
}

/// See [`crate::platform::attach_parent_console`].
pub(crate) fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
    unsafe {
        let has_stdout = GetStdHandle(STD_OUTPUT_HANDLE).map(|h| !h.is_invalid() && !h.0.is_null()).unwrap_or(false);
        if !has_stdout {
            // Fails harmlessly when there is no parent console (double-click).
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// `FOLDERID_Desktop` (the real Desktop, also when OneDrive moved it).
pub(crate) fn desktop_dir() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(std::path::PathBuf::from)
    }
}

pub fn platform() -> Platform {
    Platform { audio: Arc::new(audio::WinAudio), system: Arc::new(system::WinSystem), camera: Arc::new(camera::WinCamera::new()), dialogs: Arc::new(dialogs::WinDialogs) }
}
