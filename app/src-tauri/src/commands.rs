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

#[tauri::command]
pub async fn run_quick_checks(app: S<'_>) -> R<QuickChecks> {
    Ok(app.run_quick_checks().await)
}

#[tauri::command]
pub async fn apply_actions(app: S<'_>, action_ids: Vec<String>) -> R<ApplyResult> {
    Ok(app.apply_actions(&action_ids).await)
}

#[tauri::command]
pub async fn mic_context(app: S<'_>, device_id: String, answers: MicAnswers) -> R<MicContext> {
    app.mic_context(&device_id, &answers)
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
