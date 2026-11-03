//! Self-update: checks GitHub Releases for a newer signed installer, downloads
//! it, asks the user (plain language, native dialog) and runs it.
//!
//! * The public key comes from `plugins.updater.pubkey` in the Tauri config,
//!   which only the release build injects (see `RELEASING.md`). Without it the
//!   updater plugin is not registered and nothing is checked (one log line).
//! * `--self-test` never reaches this module (it exits before the app starts).
//! * `%APPDATA%\SetupCoach\` is never touched: the installer only replaces
//!   files under the install directory.

use crate::app::AppCore;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Where `latest.json` lives (the newest *published* GitHub release).
pub const ENDPOINT: &str = "https://github.com/ottrboii/setup-coach/releases/latest/download/latest.json";
/// Delay before the background check after start.
pub const STARTUP_DELAY: Duration = Duration::from_secs(10);

/// `check_for_update` result (API.md "Updates").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
}

/// Managed state: whether the updater plugin is active, and the update found
/// by the last `check_for_update` (for `install_update`).
pub struct UpdaterState {
    pub enabled: bool,
    pending: tokio::sync::Mutex<Option<Update>>,
}

impl UpdaterState {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, pending: tokio::sync::Mutex::new(None) }
    }
}

/// The updater public key from the Tauri `plugins` config, if a non-empty one
/// was injected at build time.
pub fn configured_pubkey(plugins: &HashMap<String, Value>) -> Option<String> {
    let k = plugins.get("updater")?.get("pubkey")?.as_str()?.trim();
    if k.is_empty() {
        None
    } else {
        Some(k.to_string())
    }
}

/// The plugin to register: `Some` only when a public key is configured, so a
/// build without keys (PR/main CI, local dev) starts normally and never checks.
pub fn plugin<R: Runtime>(plugins: &HashMap<String, Value>) -> Option<tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config>> {
    configured_pubkey(plugins).map(|_| tauri_plugin_updater::Builder::new().build())
}

fn obs_running<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Arc<AppCore>>().map(|c| c.platform.system.obs_running()).unwrap_or(false)
}

async fn find_update<R: Runtime>(app: &AppHandle<R>) -> Result<Option<Update>, String> {
    let url = ENDPOINT.parse().map_err(|e| format!("{e}"))?;
    let updater = app.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

/// Native Yes/No box. `true` = Yes. (Plain `MessageBoxW`, not a dialog crate:
/// those need the comctl32 v6 manifest, which unit-test executables lack.)
async fn ask<R: Runtime>(_app: &AppHandle<R>, title: &str, text: &str) -> bool {
    let (title, text) = (title.to_string(), text.to_string());
    tokio::task::spawn_blocking(move || message_box_yes_no(&title, &text)).await.unwrap_or(false)
}

#[cfg(windows)]
fn message_box_yes_no(title: &str, text: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_ICONINFORMATION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO};
    let (t, c) = (HSTRING::from(text), HSTRING::from(title));
    // SAFETY: both strings outlive the call; no owner window.
    unsafe { MessageBoxW(None, &t, &c, MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND | MB_TOPMOST) == IDYES }
}

#[cfg(not(windows))]
fn message_box_yes_no(title: &str, text: &str) -> bool {
    eprintln!("updater: (no native dialog on this OS) {title}: {text}");
    false
}

/// Background flow on start: check, download, ask, install. Silent on failure.
pub async fn auto_update<R: Runtime>(app: AppHandle<R>) {
    let enabled = app.try_state::<UpdaterState>().map(|s| s.enabled).unwrap_or(false);
    if !enabled {
        eprintln!("updater: no public key in this build, update check skipped");
        return;
    }
    tokio::time::sleep(STARTUP_DELAY).await;
    let update = match find_update(&app).await {
        Ok(Some(u)) => u,
        Ok(None) => return,
        Err(e) => {
            eprintln!("updater: check failed: {e}");
            return;
        }
    };
    eprintln!("updater: version {} found, downloading", update.version);
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("updater: download failed: {e}");
            return;
        }
    };
    // The voice plugin can't be replaced while OBS has it loaded.
    while obs_running(&app) {
        let closed = ask(
            &app,
            "Setup Coach update",
            "Close OBS before updating so the voice plugin can be replaced.\n\nClick Yes when OBS is closed, or No to update later.",
        )
        .await;
        if !closed {
            return;
        }
    }
    let text = format!(
        "A new version of Setup Coach is ready ({}). Install now? It takes a minute and Windows will ask for permission.\n\nYes = install now, No = ask me next time.",
        update.version
    );
    if !ask(&app, "Setup Coach update", &text).await {
        return;
    }
    if let Err(e) = update.install(bytes) {
        eprintln!("updater: install failed: {e}");
        return;
    }
    // On Windows the installer restarts the app; elsewhere restart ourselves.
    app.restart();
}

/// UI: is there a newer version? Reports "none" when this build has no update key.
#[tauri::command]
pub async fn check_for_update<R: Runtime>(app: AppHandle<R>, state: tauri::State<'_, UpdaterState>) -> Result<UpdateInfo, String> {
    if !state.enabled {
        return Ok(UpdateInfo::default());
    }
    let found = find_update(&app).await?;
    let info = match &found {
        Some(u) => UpdateInfo { available: true, version: Some(u.version.clone()), notes: u.body.clone() },
        None => UpdateInfo::default(),
    };
    *state.pending.lock().await = found;
    Ok(info)
}

/// UI: download and install the update found by `check_for_update`, then restart.
#[tauri::command]
pub async fn install_update<R: Runtime>(app: AppHandle<R>, state: tauri::State<'_, UpdaterState>) -> Result<(), String> {
    if obs_running(&app) {
        return Err("obs_running: close OBS first so the voice plugin can be replaced".into());
    }
    let update = state.pending.lock().await.take().ok_or("no update available: run check_for_update first")?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    update.install(bytes).map_err(|e| e.to_string())?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plugins(v: Value) -> HashMap<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn no_pubkey_means_no_updater() {
        assert_eq!(configured_pubkey(&plugins(json!({}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {}}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {"pubkey": ""}}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {"pubkey": "  "}}))), None);
        assert!(plugin::<tauri::Wry>(&plugins(json!({}))).is_none());
    }

    #[test]
    fn pubkey_enables_updater() {
        let p = plugins(json!({"updater": {"pubkey": "dW50cnVzdGVk"}}));
        assert_eq!(configured_pubkey(&p).as_deref(), Some("dW50cnVzdGVk"));
        assert!(plugin::<tauri::Wry>(&p).is_some());
    }
}
