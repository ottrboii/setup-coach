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
pub async fn run_quick_checks(app: S<'_>, device_id: Option<String>) -> R<QuickChecks> {
    let core = app.inner().clone();
    let task = tokio::spawn(async move { core.run_quick_checks_for(device_id.as_deref()).await });
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

/// Apply the chosen quick fixes, then re-scan and report each one as fixed /
/// couldn't fix (and why) / needs you. One line per fix goes to `logs\ui.log`.
#[tauri::command]
pub async fn fix_and_verify(app: S<'_>, action_ids: Vec<String>, device_id: Option<String>) -> R<FixReport> {
    let core = app.inner().clone();
    let ids = action_ids.clone();
    let task = tokio::spawn(async move { core.fix_and_verify(&ids, device_id.as_deref()).await });
    match tokio::time::timeout(std::time::Duration::from_secs(180), task).await {
        Ok(Ok(r)) => {
            for i in &r.items {
                app.log_ui("info", "fix", &format!("{} [{:?}] {}", i.label, i.status, i.message));
            }
            Ok(r)
        }
        Ok(Err(_)) => {
            app.log_ui("error", "backend", "fix_and_verify panicked");
            Err("The fix hit an unexpected problem. Nothing was counted as fixed; run the scan again to see where things stand.".into())
        }
        Err(_) => {
            app.log_ui("error", "backend", "fix_and_verify timed out after 180 s");
            Err("The fix is taking too long. Run the scan again to see where things stand.".into())
        }
    }
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
pub async fn start_camera_test(app: S<'_>, camera_id: String) -> Result<(), CameraError> {
    app.inner().start_camera_test(&camera_id).await
}

#[tauri::command]
pub async fn capture_camera_step(app: S<'_>, step: TestStep, seconds: f64) -> Result<CaptureStepResult, CameraError> {
    app.capture_camera_step(step, seconds).await
}

#[tauri::command]
pub async fn finish_camera_test(app: S<'_>, constraints: Vec<Constraint>) -> Result<CameraReport, CameraError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.finish_camera_test(constraints)).await.map_err(|e| CameraError::from(e.to_string()))?
}

#[tauri::command]
pub async fn camera_picture_status(app: S<'_>, camera_id: String) -> R<PictureStatus> {
    Ok(app.inner().camera_picture_status(&camera_id).await)
}

#[tauri::command]
pub async fn free_camera(app: S<'_>, camera_id: String) -> R<FreeCameraResult> {
    Ok(app.inner().free_camera(&camera_id).await)
}

#[tauri::command]
pub async fn restore_freed_camera(app: S<'_>) -> R<FreeCameraResult> {
    Ok(app.inner().restore_freed_camera().await)
}

#[tauri::command]
pub async fn start_sync_test(app: S<'_>, device_id: String, camera_id: String) -> Result<(), CameraError> {
    app.inner().start_sync_test(&device_id, &camera_id).await
}

#[tauri::command]
pub async fn finish_sync_test(app: S<'_>) -> R<crate::app::SyncReport> {
    app.finish_sync_test().await
}

#[tauri::command]
pub async fn stop_sync_test(app: S<'_>) -> R<()> {
    app.stop_sync_test();
    Ok(())
}

#[tauri::command]
pub async fn apply_sync_fix(app: S<'_>) -> R<crate::app::SyncApplyResult> {
    Ok(app.apply_sync_fix().await)
}

#[tauri::command]
pub async fn undo_sync_fix(app: S<'_>) -> R<crate::app::SyncApplyResult> {
    Ok(app.undo_sync_fix().await)
}

#[tauri::command]
pub async fn start_drift_check(app: S<'_>, device_id: String, camera_id: String, interval_s: f64) -> Result<crate::app::DriftStatus, CameraError> {
    app.inner().start_drift_check(&device_id, &camera_id, interval_s).await
}

#[tauri::command]
pub async fn start_drift_clap(app: S<'_>) -> R<()> {
    app.inner().start_drift_clap()
}

#[tauri::command]
pub async fn finish_drift_clap(app: S<'_>) -> R<crate::app::DriftStatus> {
    app.inner().finish_drift_clap().await
}

#[tauri::command]
pub async fn cancel_drift_clap(app: S<'_>) -> R<crate::app::DriftStatus> {
    app.cancel_drift_clap()
}

#[tauri::command]
pub async fn drift_status(app: S<'_>) -> R<Option<crate::app::DriftStatus>> {
    Ok(app.drift_status())
}

#[tauri::command]
pub async fn stop_drift_check(app: S<'_>) -> R<()> {
    app.stop_drift_check();
    Ok(())
}

#[tauri::command]
pub async fn get_mains(app: S<'_>) -> R<MainsInfo> {
    Ok(app.mains_info())
}

#[tauri::command]
pub async fn set_mains(app: S<'_>, hz: Option<u8>) -> R<MainsInfo> {
    app.set_mains(hz)
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
pub async fn list_setup_profiles(app: S<'_>, device_id: String) -> R<Vec<coach_core::setup_profile::ProfileInfo>> {
    app.list_setup_profiles(&device_id)
}

#[tauri::command]
pub async fn get_setup_profile(app: S<'_>) -> R<coach_core::SetupProfile> {
    Ok(app.active_profile())
}

#[tauri::command]
pub async fn set_setup_profile(app: S<'_>, profile: coach_core::SetupProfile) -> R<SetupProfileSwitch> {
    app.set_setup_profile(profile).await
}

#[tauri::command]
pub async fn get_voiceover_target(app: S<'_>) -> R<VoiceoverTargetInfo> {
    Ok(app.voiceover_target_info())
}

#[tauri::command]
pub async fn set_voiceover_target(app: S<'_>, target: coach_core::VoiceoverTarget) -> R<VoiceoverTargetInfo> {
    app.set_voiceover_target(target)
}

#[tauri::command]
pub async fn start_voice_tune(app: S<'_>, device_id: String) -> Result<VoiceTuneResult, VoiceTuneError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.start_voice_tune(&device_id)).await.map_err(|e| VoiceTuneError::from(e.to_string()))?
}

#[tauri::command]
pub async fn gentler_voice_tune(app: S<'_>, tune_id: String) -> Result<VoiceTuneResult, VoiceTuneError> {
    let app = app.inner().clone();
    tokio::task::spawn_blocking(move || app.gentler_voice_tune(&tune_id)).await.map_err(|e| VoiceTuneError::from(e.to_string()))?
}

#[tauri::command]
pub async fn apply_voice_tune(app: S<'_>, tune_id: String) -> Result<VoiceTuneApplied, VoiceTuneError> {
    app.apply_voice_tune(&tune_id).await
}

#[tauri::command]
pub async fn undo_voice_tune(app: S<'_>, backup_id: String) -> R<ApplyResult> {
    Ok(app.undo_voice_tune(&backup_id).await)
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
pub async fn fix_mic_level(app: S<'_>, device_id: String) -> R<LevelFixResult> {
    let core = app.inner().clone();
    match tokio::spawn(async move { core.fix_mic_level(&device_id).await }).await {
        Ok(r) => r,
        Err(_) => Err("Changing the level hit a snag. Use Undo (Backups) if your mic level sounds different now.".into()),
    }
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

#[tauri::command]
pub async fn list_mic_profiles(app: S<'_>) -> R<Vec<MicProfileOption>> {
    Ok(app.list_mic_profiles())
}

#[tauri::command]
pub async fn get_mic_behind(app: S<'_>, device_id: String) -> R<Option<MicBehind>> {
    Ok(app.mic_choice(&device_id))
}

#[tauri::command]
pub async fn set_mic_behind(app: S<'_>, device_id: String, choice: MicBehindInput) -> R<MicBehindResult> {
    app.set_mic_behind(&device_id, choice).await
}

/// Testers' notes: append one to `%APPDATA%\SetupCoach\feedback.jsonl` (local only, never uploaded).
#[tauri::command]
pub async fn save_feedback(app: S<'_>, input: crate::feedback::FeedbackInput) -> R<crate::feedback::FeedbackSaved> {
    crate::feedback::save(&app.data_dir, input, &crate::paths::now_iso(), env!("CARGO_PKG_VERSION"))
}

/// All saved notes as plain text (for "Copy my notes").
#[tauri::command]
pub async fn get_feedback(app: S<'_>) -> R<crate::feedback::FeedbackExport> {
    Ok(crate::feedback::export(&app.data_dir))
}

/// Write the notes as a readable text file on the Desktop; returns the file path.
#[tauri::command]
pub async fn save_feedback_to_desktop(app: S<'_>) -> R<String> {
    let out = crate::selftest::default_out_dir(&app);
    crate::feedback::export_to_dir(&app.data_dir, &out).map(|p| p.display().to_string())
}