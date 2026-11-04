//! Self-update: checks GitHub Releases for a newer signed installer, downloads
//! it in the background (nothing is installed yet), then asks the user (plain
//! language, native dialog) and only installs on an explicit Yes.
//!
//! * The update endpoint lives in one place: `plugins.updater.endpoints` in
//!   `tauri.release.conf.json` (the release build injects it with the key).

//! * The public key comes from `plugins.updater.pubkey` in the Tauri config,
//!   which only the release build injects (see `RELEASING.md`). Without it the
//!   updater plugin is not registered and nothing is checked (one log line).
//! * `--self-test` never reaches this module (it exits before the app starts).
//! * `%APPDATA%\SetupCoach\` is never touched: the installer only replaces
//!   files under the install directory.

use crate::app::AppCore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use crate::platform::Platform;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Delay before the background check after start.
pub const STARTUP_DELAY: Duration = Duration::from_secs(10);
/// How many times we ask "is OBS closed yet?" before giving up until next start.
pub const MAX_OBS_PROMPTS: usize = 3;
/// Pause after a "Yes, OBS is closed" that turned out untrue, before asking again.
pub const OBS_RECHECK_PAUSE: Duration = Duration::from_secs(5);

/// Managed state: whether the updater plugin is active (a public key is configured).
pub struct UpdaterState {
    pub enabled: bool,
}

impl UpdaterState {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
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

async fn find_update<R: Runtime>(app: &AppHandle<R>) -> Result<Option<Update>, String> {
    // Endpoints come from the plugin config (see the module doc).
    let updater = app.updater().map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

/// What the confirmation flow ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    /// The user said No: nothing was installed.
    Declined,
    /// OBS stayed open after `MAX_OBS_PROMPTS` questions, or the user said No to closing it.
    ObsStillRunning,
    InstallFailed(String),
}

async fn ask(platform: &Platform, title: &str, text: &str) -> bool {
    let (d, title, text) = (platform.dialogs.clone(), title.to_string(), text.to_string());
    tokio::task::spawn_blocking(move || d.ask_yes_no(&title, &text)).await.unwrap_or(false)
}

/// The decision flow for an already downloaded update: wait for OBS to be
/// closed (the voice plugin can't be replaced while it is loaded), ask, and
/// run `install` only on an explicit Yes. No means do not install.
pub async fn confirm_and_install(platform: &Platform, version: &str, obs_pause: Duration, install: impl FnOnce() -> Result<(), String>) -> Outcome {
    let mut prompts = 0;
    while platform.system.obs_running() {
        if prompts >= MAX_OBS_PROMPTS {
            return Outcome::ObsStillRunning;
        }
        prompts += 1;
        let closed = ask(
            platform,
            "Setup Coach update",
            "Close OBS before updating so the voice plugin can be replaced.\n\nClick Yes when OBS is closed, or No to update later.",
        )
        .await;
        if !closed {
            return Outcome::ObsStillRunning;
        }
        if platform.system.obs_running() {
            tokio::time::sleep(obs_pause).await;
        }
    }
    let text = format!(
        "A new version of Setup Coach is ready ({version}). Install now? It takes a minute and Windows will ask for permission.\n\nYes = install now, No = ask me next time."
    );
    if !ask(platform, "Setup Coach update", &text).await {
        return Outcome::Declined;
    }
    match install() {
        Ok(()) => Outcome::Installed,
        Err(e) => Outcome::InstallFailed(e),
    }
}

/// Background flow on start: check, download quietly, ask, install. Silent on failure.
pub async fn auto_update<R: Runtime>(app: AppHandle<R>) {
    let enabled = app.try_state::<UpdaterState>().map(|s| s.enabled).unwrap_or(false);
    if !enabled {
        eprintln!("updater: no public key in this build, update check skipped");
        return;
    }
    tokio::time::sleep(STARTUP_DELAY).await;
    let Some(platform) = app.try_state::<Arc<AppCore>>().map(|c| c.platform.clone()) else {
        eprintln!("updater: app core not ready, update check skipped");
        return;
    };
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
    match confirm_and_install(&platform, &update.version, OBS_RECHECK_PAUSE, || update.install(bytes).map_err(|e| e.to_string())).await {
        // On Windows the installer restarts the app; elsewhere restart ourselves.
        Outcome::Installed => app.restart(),
        Outcome::InstallFailed(e) => eprintln!("updater: install failed: {e}"),
        other => eprintln!("updater: not installed now ({other:?})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockPlatform;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn plugins(v: Value) -> HashMap<String, Value> {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn no_pubkey_means_no_updater() {
        assert_eq!(configured_pubkey(&plugins(json!({}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {}}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {"pubkey": ""}}))), None);
        assert_eq!(configured_pubkey(&plugins(json!({"updater": {"pubkey": "  "}}))), None);
    }

    #[test]
    fn pubkey_enables_updater() {
        let p = plugins(json!({"updater": {"pubkey": "dW50cnVzdGVk"}}));
        assert_eq!(configured_pubkey(&p).as_deref(), Some("dW50cnVzdGVk"));
    }

    fn world(obs_running: bool, answers: &[bool]) -> (MockPlatform, Platform) {
        let m = MockPlatform::demo();
        *m.system.obs_running.lock().unwrap() = obs_running;
        m.dialogs.answers.lock().unwrap().extend(answers.iter().copied());
        let p = m.platform();
        (m, p)
    }

    #[tokio::test]
    async fn no_means_no_install() {
        let (m, p) = world(false, &[false]);
        let ran = AtomicBool::new(false);
        let o = confirm_and_install(&p, "1.2.3", Duration::ZERO, || {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert_eq!(o, Outcome::Declined);
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(m.dialogs.asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn yes_installs_once() {
        let (m, p) = world(false, &[true]);
        let ran = AtomicBool::new(false);
        let o = confirm_and_install(&p, "1.2.3", Duration::ZERO, || {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert_eq!(o, Outcome::Installed);
        assert!(ran.load(Ordering::SeqCst));
        assert!(m.dialogs.asked.lock().unwrap()[0].contains("1.2.3"));
    }

    #[tokio::test]
    async fn failed_install_is_reported() {
        let (_m, p) = world(false, &[true]);
        let o = confirm_and_install(&p, "1.2.3", Duration::ZERO, || Err("disk full".into())).await;
        assert_eq!(o, Outcome::InstallFailed("disk full".into()));
    }

    #[tokio::test]
    async fn waits_for_obs_to_close_then_asks_to_install() {
        let (m, p) = world(true, &[true, true]);
        let sys = m.system.clone();
        // The user closes OBS while the first box is open.
        *m.dialogs.on_ask.lock().unwrap() = Some(Box::new(move |_| *sys.obs_running.lock().unwrap() = false));
        let ran = AtomicBool::new(false);
        let o = confirm_and_install(&p, "2.0.0", Duration::ZERO, || {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert_eq!(o, Outcome::Installed);
        let asked = m.dialogs.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains("Close OBS") && asked[1].contains("Install now"));
    }

    #[tokio::test]
    async fn obs_prompt_is_capped_and_no_stops_it() {
        // Says Yes every time but never closes OBS: gives up after the cap.
        let (m, p) = world(true, &[true; 10]);
        let o = confirm_and_install(&p, "2.0.0", Duration::ZERO, || panic!("must not install")).await;
        assert_eq!(o, Outcome::ObsStillRunning);
        assert_eq!(m.dialogs.asked.lock().unwrap().len(), MAX_OBS_PROMPTS);
        // No to "close OBS" stops straight away.
        let (m, p) = world(true, &[false]);
        let o = confirm_and_install(&p, "2.0.0", Duration::ZERO, || panic!("must not install")).await;
        assert_eq!(o, Outcome::ObsStillRunning);
        assert_eq!(m.dialogs.asked.lock().unwrap().len(), 1);
    }
}
