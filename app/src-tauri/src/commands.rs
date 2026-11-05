//! Thin Tauri command layer over `AppCore` (names/args per API.md).

use crate::app::AppCore;
use crate::types::*;
use coach_core::audio::{findings::MicContext, MicReport, ProcessingChain, SegmentKind};
use coach_core::health::StreamReport;
use coach_core::video::{CameraSettings, CameraState, TestStep};
use coach_core::Constraint;
use std::sync::Arc;
use tauri::State;

type S<'a> = State<'a, Arc<AppCore>>;
type R<T> = Result<T, String>;

#[tauri::command]
pub async fn get_environment(app: S<'_>) -> R<Environment> {
    Ok(app.get_environment().await)
}

#[tauri::command]
pub async fn connect_obs(app: S<'_>, password: Option<String>) -> R<ObsStatus> {
    Ok(app.connect_obs(password).await)
}

/// The quick scan, guarded: it runs in its own task with a time limit, so a
/// panic or a stuck Windows call becomes a plain error the UI can show
/// instead of a promise that never settles.
#[tauri::command]
pub async fn run_quick_checks(app: S<'_>) -> R<QuickChecks> {
    let core = app.inner().clone();
    let task = tokio::spawn(async move { core.run_quick_checks().await });
    match tokio::time::timeout(std::time::Duration::from_secs(45), task).await {
        Ok(Ok(q)) => Ok(q),
        Ok(Err(_)) => {
            app.log_ui("error", "backend", "run_quick_checks panicked");
            Err("The scan hit an unexpected problem. Please try again.".into())
        }
        Err(_) => {
            app.log_ui("error", "backend", "run_quick_checks timed out after 45 s");
            Err("The scan is taking too long. Please try again.".into())
        }
    }
}

/// UI -> log: errors the screens caught land in `%APPDATA%\SetupCoach\logs\ui.log`
/// (and so in the self-test zip).
#[tauri::command]
pub async fn log_ui(app: S<'_>, level: String, source: String, message: String) -> R<()> {
    app.log_ui(&level, &source, &message);
    Ok(())
}

#[tauri::command]
pub async fn apply_actions(app: S<'_>, action_ids: Vec<String>) -> R<ApplyResult> {
    Ok(app.apply_actions(&action_ids).await)
}

#[tauri::command]
pub async fn mic_context(app: S<'_>, device_id: String, answers: MicAnswers) -> R<MicContext> {
    app.mic_context_async(&device_id, &answers).await
}

#[tauri::command]
pub async fn start_mic_test(app: S<'_>, device_id: String) -> R<()> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.start_mic_test(&device_id)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn mark_step(app: S<'_>, kind: SegmentKind, edge: Edge) -> R<()> {
    app.mark_step(kind, edge)
}

#[tauri::command]
pub async fn finish_mic_test(app: S<'_>, context: MicContext, constraints: Vec<Constraint>) -> R<MicReport> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.finish_mic_test(context, constraints)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn rescan_mic(app: S<'_>, context: MicContext, constraints: Vec<Constraint>) -> R<MicReport> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.rescan_mic(context, constraints)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn apply_mic_chain(app: S<'_>, chain: ProcessingChain, obs_input: String) -> R<ApplyResult> {
    Ok(app.apply_mic_chain(chain, &obs_input).await)
}

#[tauri::command]
pub async fn verify_mic(app: S<'_>) -> R<VerifyResult> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.verify_mic()).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn play_preview(app: S<'_>, which: PreviewWhich, phone: bool) -> R<()> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.play_preview(which, phone)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn stop_preview(app: S<'_>) -> R<()> {
    app.stop_preview();
    Ok(())
}

#[tauri::command]
pub async fn camera_state(app: S<'_>, camera_id: String) -> R<CameraState> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.camera_state(&camera_id)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn start_camera_test(app: S<'_>, camera_id: String) -> R<()> {
    app.inner().start_camera_test(&camera_id).await
}

#[tauri::command]
pub async fn capture_camera_step(app: S<'_>, step: TestStep, seconds: f64) -> R<CaptureStepResult> {
    app.capture_camera_step(step, seconds).await
}

#[tauri::command]
pub async fn finish_camera_test(app: S<'_>, constraints: Vec<Constraint>) -> R<CameraReport> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.finish_camera_test(constraints)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn apply_camera_settings(app: S<'_>, camera_id: String, settings: CameraSettings) -> R<ApplyResult> {
    Ok(app.apply_camera_settings(&camera_id, settings).await)
}

#[tauri::command]
pub async fn start_health_monitor(app: S<'_>) -> R<()> {
    app.inner().start_health_monitor().await
}

#[tauri::command]
pub async fn stop_health_monitor(app: S<'_>) -> R<StreamReport> {
    app.stop_health_monitor()
}

#[tauri::command]
pub async fn save_setup(app: S<'_>, name: String) -> R<SavedProfile> {
    app.save_setup(&name)
}

#[tauri::command]
pub async fn reapply_saved_setup(app: S<'_>) -> R<ApplyResult> {
    Ok(app.reapply_saved_setup().await)
}

#[tauri::command]
pub async fn coach_plan(app: S<'_>, area: CoachArea) -> R<Vec<CoachStep>> {
    app.coach_plan(area).await
}

#[tauri::command]
pub async fn start_coach(app: S<'_>, step: CoachStep) -> R<()> {
    app.inner().start_coach(step).await
}

#[tauri::command]
pub async fn stop_coach(app: S<'_>) -> R<()> {
    app.stop_coach();
    Ok(())
}

#[tauri::command]
pub async fn open_for_user(app: S<'_>, target: OpenTarget) -> R<()> {
    app.open_for_user(&target).await
}

#[tauri::command]
pub async fn list_backups(app: S<'_>) -> R<Vec<crate::vault::BackupInfo>> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.list_backups()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn restore_backup(app: S<'_>, id: String) -> R<ApplyResult> {
    Ok(app.restore_backup(&id).await)
}

#[tauri::command]
pub async fn record_final(app: S<'_>, device_id: String, context: MicContext, constraints: Vec<Constraint>) -> Result<AbInfo, AbError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.record_final(&device_id, context, constraints)).await.map_err(|e| AbError::from(e.to_string()))?
}

#[tauri::command]
pub async fn get_ab(app: S<'_>, device_id: String) -> Result<AbInfo, AbError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.get_ab(&device_id)).await.map_err(|e| AbError::from(e.to_string()))?
}

#[tauri::command]
pub async fn export_ab(app: S<'_>, device_id: String) -> Result<String, AbError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.export_ab(&device_id)).await.map_err(|e| AbError::from(e.to_string()))?
}

#[tauri::command]
pub async fn preview_restore(app: S<'_>, id: String) -> R<RestorePreview> {
    app.preview_restore(&id).await
}

#[tauri::command]
pub async fn obs_running(app: S<'_>) -> R<bool> {
    Ok(app.obs_running())
}
