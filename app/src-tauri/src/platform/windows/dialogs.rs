//! Native Yes/No box. Plain `MessageBoxW`, not a dialog crate: those need the
//! comctl32 v6 manifest, which unit-test executables lack.

use crate::platform::Dialogs;
use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONINFORMATION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO};

pub struct WinDialogs;

impl Dialogs for WinDialogs {
    fn ask_yes_no(&self, title: &str, text: &str) -> bool {
        let (t, c) = (HSTRING::from(text), HSTRING::from(title));
        // SAFETY: both strings outlive the call; no owner window.
        unsafe { MessageBoxW(None, &t, &c, MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST) == IDYES }
    }
}
