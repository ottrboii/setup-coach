//! Setup Coach desktop app (Tauri 2). Logic lives in [`app::AppCore`], which is
//! Tauri-free and tested with the mock platform; [`run`] wires it to the UI.

pub mod app;
pub mod camera;
pub mod commands;
pub mod goxlr;
pub mod obs;
pub mod paths;
pub mod platform;
pub mod selftest;
pub mod types;
pub mod vault;
pub mod wav;
pub mod updater;

use app::{AppCore, Emitter, WindowControl};
use std::sync::Arc;
use tauri::{Emitter as _, Manager};

struct TauriEmitter(tauri::AppHandle);

impl Emitter for TauriEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }
}

struct TauriWindow(tauri::AppHandle);

impl WindowControl for TauriWindow {
    fn set_mini(&self, on: bool) {
        if let Some(w) = self.0.get_webview_window("main") {
            let _ = w.set_always_on_top(on);
            let size = if on { tauri::LogicalSize::new(420.0, 560.0) } else { tauri::LogicalSize::new(1100.0, 760.0) };
            let _ = w.set_size(tauri::Size::Logical(size));
        }
    }
    fn focus(&self) {
        if let Some(w) = self.0.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

pub fn run() {
    let context = tauri::generate_context!();
    // Self-update is only active when the release build injected a public key.
    let updater_plugin = updater::plugin(&context.config().plugins.0);
    let updater_enabled = updater_plugin.is_some();
    let mut builder = tauri::Builder::default();
    if let Some(p) = updater_plugin {
        builder = builder.plugin(p);
    }
    builder
        .setup(move |app| {
            app.manage(updater::UpdaterState::new(updater_enabled));
            tauri::async_runtime::spawn(updater::auto_update(app.handle().clone()));
            let core = Arc::new(AppCore::new(platform::native(), paths::data_dir(), Arc::new(TauriEmitter(app.handle().clone()))));
            core.set_window_control(Arc::new(TauriWindow(app.handle().clone())));
            app.manage(core.clone());
            // Webcams forget their settings on restart: re-apply the saved setup.
            tauri::async_runtime::spawn(async move {
                if core.load_setup().is_some() {
                    let _ = core.reapply_saved_setup().await;
                }
            });
            // TODO: re-apply camera settings on device arrival (CM_Register_Notification).
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_environment,
            commands::connect_obs,
            commands::run_quick_checks,
            commands::log_ui,
            commands::apply_actions,
            commands::mic_context,
            commands::start_mic_test,
            commands::mark_step,
            commands::finish_mic_test,
            commands::rescan_mic,
            commands::apply_mic_chain,
            commands::verify_mic,
            commands::play_preview,
            commands::stop_preview,
            commands::camera_state,
            commands::start_camera_test,
            commands::capture_camera_step,
            commands::finish_camera_test,
            commands::camera_picture_status,
            commands::free_camera,
            commands::restore_freed_camera,
            commands::get_mains,
            commands::set_mains,
            commands::apply_camera_settings,
            commands::start_health_monitor,
            commands::stop_health_monitor,
            commands::save_setup,
            commands::reapply_saved_setup,
            commands::coach_plan,
            commands::start_coach,
            commands::stop_coach,
            commands::open_for_user,
            commands::list_backups,
            commands::restore_backup,
            commands::preview_restore,
            commands::obs_running,
            commands::list_mic_profiles,
            commands::get_mic_behind,
            commands::set_mic_behind,
            commands::record_final,
            commands::get_ab,
            commands::export_ab,
        ])
        .run(context)
        .expect("error while running Setup Coach");
}
