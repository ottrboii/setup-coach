//! App-only data types exchanged with the UI (see `app/API.md`). Shared data
//! types come straight from `coach_core` and serialize as-is.

use coach_core::audio::MicReport;
use coach_core::profiles::{Confidence, Question};
use coach_core::system::Action;
use coach_core::video::{CameraClass, CameraSettings, VideoMetrics};
use coach_core::{Finding, score::Score};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObsStatus {
    pub running: bool,
    pub connected: bool,
    pub version: Option<String>,
    pub websocket_enabled: Option<bool>,
    pub error: Option<String>,
}

/// Owned, serializable view of a profile match (core's `MatchResult` borrows
/// the profile database).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileMatch {
    /// "mic" | "interface" | "camera"
    pub kind: String,
    pub profile_id: String,
    pub display_name: String,
    pub confidence: Confidence,
    pub verified: bool,
    pub questions: Vec<Question>,
    /// For a mic behind an interface: the interface it was found on.
    pub interface: Option<Box<ProfileMatch>>,
    /// The full profile as JSON (for the Advanced drawer).
    pub profile: serde_json::Value,
    /// True when this is the mic the user said is plugged into the device
    /// (`set_mic_behind`), not what we guessed from the Windows device name.
    #[serde(default)]
    pub chosen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_default_comms: bool,
    pub channels: u16,
    pub sample_rate: u32,
    pub profile: Option<ProfileMatch>,
    /// The OBS audio input capturing this device, if any.
    pub obs_input: Option<String>,
    /// What the user told us is plugged into this device (None = never asked).
    #[serde(default)]
    pub mic_choice: Option<MicBehind>,
    /// True when we should ask "What mic is plugged into this?" now: an
    /// interface/mixer-type or generic-named device with no answer yet.
    #[serde(default)]
    pub needs_mic_choice: bool,
}

/// How the user answered "What mic is plugged into this?" for one device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicBehind {
    /// `chosen` (a profile from `list_mic_profiles`), `other` (not in the
    /// list) or `skipped` (Not sure).
    pub status: MicBehindStatus,
    /// The mic profile id when `status` is `chosen`.
    pub profile_id: Option<String>,
    /// `Brand Model` of the chosen profile (for "Your mic: ...").
    pub display_name: Option<String>,
    /// The Windows device name this was answered for.
    pub device_name: String,
    pub chosen_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MicBehindStatus {
    Chosen,
    Other,
    Skipped,
}

/// Argument of `set_mic_behind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MicBehindInput {
    Profile { profile_id: String },
    Other,
    Skipped,
    /// Forget the answer (ask again).
    Clear,
}

/// Result of `set_mic_behind`: the device's new view, so the UI needn't
/// reload the whole environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicBehindResult {
    pub mic_choice: Option<MicBehind>,
    pub profile: Option<ProfileMatch>,
    pub needs_mic_choice: bool,
}

/// One entry of the "What mic is plugged into this?" picker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MicProfileOption {
    pub id: String,
    pub brand: String,
    pub model: String,
    /// `Brand Model`.
    pub display_name: String,
    pub family: String,
    /// `usb` | `xlr` | `usb_xlr` | `headset` | `wireless` | `builtin`
    pub connection: String,
    /// `dynamic` | `condenser` | `electret`
    pub capsule: String,
    pub verified: bool,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraDeviceInfo {
    pub id: String,
    pub name: String,
    pub obs_source: Option<String>,
    pub profile: Option<ProfileMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedProfile {
    pub name: String,
    pub saved_at: String,
    pub mic_chain: Option<coach_core::audio::ProcessingChain>,
    pub camera_settings: Option<CameraSettings>,
    pub camera_id: Option<String>,
    pub obs_input: Option<String>,
    /// Which physical mic is behind which device (key = device id). Added 2026-11-05.
    #[serde(default)]
    pub mic_behind: std::collections::BTreeMap<String, MicBehind>,
    /// What the session is set up for (streaming, voiceover, call). Added 2026-11-06.
    #[serde(default)]
    pub active_profile: coach_core::SetupProfile,
    /// One stored mic setup per profile (key = `streaming`|`voiceover`|`call`) per device id.
    #[serde(default)]
    pub mic_setups: std::collections::BTreeMap<String, std::collections::BTreeMap<String, StoredMicSetup>>,
}

/// The mic setup a finished flow left for one (profile, device). Switching profiles re-applies it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredMicSetup {
    pub profile: coach_core::SetupProfile,
    pub device_id: String,
    pub saved_at: String,
    pub mic_chain: coach_core::audio::ProcessingChain,
    pub obs_input: Option<String>,
    /// "I can't do this" answers belong to this profile only.
    #[serde(default)]
    pub constraints: Vec<coach_core::Constraint>,
    pub score: u8,
    pub targets: Vec<coach_core::audio::targets::TargetCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupProfileSwitch {
    pub profile: coach_core::SetupProfile,
    /// The stored setup for (profile, current mic), re-applied.
    pub applied: Option<ApplyResult>,
    /// True when nothing is stored yet for this pair.
    pub needs_scan: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Environment {
    pub app_version: String,
    pub windows_build: Option<String>,
    pub obs: ObsStatus,
    pub mics: Vec<AudioDeviceInfo>,
    pub cameras: Vec<CameraDeviceInfo>,
    pub saved_profile: Option<SavedProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlannedActionView {
    pub id: String,
    pub label: String,
    pub resolves: Vec<String>,
    pub needs_admin: bool,
    pub needs_obs_closed: bool,
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuickChecks {
    pub findings: Vec<Finding>,
    pub actions: Vec<PlannedActionView>,
    pub score: Vec<Score>,
    /// Cameras close to the time they usually switch off (pre-stream warning).
    #[serde(default)]
    pub runtime_warnings: Vec<coach_core::system::RuntimeWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyFailure {
    pub id: String,
    pub error: String,
    /// When the app can't do this itself but the user can in a couple of
    /// clicks: a ready `CheckSetting` coach step (opens the exact Windows page
    /// for the device and watches for the change). Pass it to `start_coach`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guided: Option<CoachStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ApplyResult {
    pub applied: Vec<String>,
    pub failed: Vec<ApplyFailure>,
    pub restart_obs_needed: bool,
    /// Left alone on purpose, with a plain note (e.g. a device that's
    /// unplugged during `restore_backup`). Empty for other commands.
    #[serde(default)]
    pub skipped: Vec<ApplyFailure>,
}

impl ApplyResult {
    pub fn ok(&mut self, id: impl Into<String>) {
        self.applied.push(id.into());
    }
    pub fn fail(&mut self, id: impl Into<String>, error: impl Into<String>) {
        self.failed.push(ApplyFailure { id: id.into(), error: error.into(), guided: None });
    }
    /// Failed, but the user can finish it in a guided step.
    pub fn fail_guided(&mut self, id: impl Into<String>, error: impl Into<String>, step: CoachStep) {
        self.failed.push(ApplyFailure { id: id.into(), error: error.into(), guided: Some(step) });
    }
    pub fn skip(&mut self, id: impl Into<String>, note: impl Into<String>) {
        self.skipped.push(ApplyFailure { id: id.into(), error: note.into(), guided: None });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MicAnswers {
    pub mic_model_id: Option<String>,
    pub booster: Option<bool>,
    pub gain_at_max: Option<bool>,
    pub onboard_auto_level: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct MicLevel {
    pub rms_db: f64,
    pub peak_db: f64,
    pub clipped: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Start,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewWhich {
    Before,
    After,
    /// The saved first recording (loudness-matched to the final take).
    Original,
    /// The final take through the applied settings (loudness-matched).
    Final,
}

/// One side of the A/B (`get_ab`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbTake {
    /// The WAV on disk (raw take for `original`, processed take for `final`).
    pub path: String,
    /// Integrated loudness of the reading part, LUFS.
    pub lufs: f64,
    /// Mic score 0-100 of that take.
    pub score: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbInfo {
    pub original: AbTake,
    #[serde(rename = "final")]
    pub final_take: Option<AbTake>,
    /// Final LUFS minus original LUFS (0 without a final take). Playback turns
    /// the louder one down by this much so the comparison is fair.
    pub matched_gain_db: f64,
    /// Plain-language note for the result card; set when the two takes differ
    /// by more than the 30 dB the matching can correct.
    #[serde(default)]
    pub note: Option<String>,
    /// Findings of the final take that are still open (minor or worse), as
    /// found when it was recorded. Empty without a final take. The "I can't do
    /// this" path of the A/B screen is built from these.
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// Stable reason a recording/A-B command failed, so the UI can word it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbErrorCode {
    /// No first test saved for this mic yet.
    NoFirstTest,
    /// There is no final take yet (export, playback).
    NoFinalTake,
    /// The mic isn't connected and we have no recordings for it.
    MicMissing,
    /// `record_final` without a running recording.
    NoRecording,
    /// The running recording was made with another mic.
    WrongMic,
    TooShort,
    /// No reading step was marked.
    NoReadStep,
    /// No new settings applied yet, so there is nothing to render.
    NoAppliedChain,
    /// A saved recording is damaged or can't be read.
    Unreadable,
    Other,
}

/// Error of `get_ab`, `record_final` and `export_ab`: `{ code, message }`.
/// `message` is plain English and is only a fallback for unknown codes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbError {
    pub code: AbErrorCode,
    pub message: String,
}

impl AbError {
    pub fn new(code: AbErrorCode, message: impl Into<String>) -> Self {
        AbError { code, message: message.into() }
    }
}

impl std::fmt::Display for AbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for AbError {
    fn from(message: String) -> Self {
        AbError { code: AbErrorCode::Other, message }
    }
}

impl From<&str> for AbError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl From<AbError> for String {
    fn from(e: AbError) -> String {
        e.message
    }
}

/// Why there is no usable camera picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PictureReason {
    /// Frames arrive but they are all black.
    Black,
    /// Frames arrive but are one flat colour / frozen.
    Blank,
    /// The camera could not be opened or read.
    CantOpen,
    /// The camera did not answer in time.
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraErrorCode {
    /// No usable picture (see `reason`): nothing was analysed.
    NoPicture,
    Other,
}

/// Error of `start_camera_test`, `capture_camera_step` and `finish_camera_test`:
/// `{ code, reason?, message }`. `message` is plain English (fallback text).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraError {
    pub code: CameraErrorCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<PictureReason>,
    pub message: String,
}

impl CameraError {
    pub fn no_picture(reason: PictureReason) -> Self {
        let message = match reason {
            PictureReason::Black | PictureReason::Blank => "We can't see your camera: the picture is black. Another app is probably using it.",
            PictureReason::CantOpen => "We can't open your camera. Another app is probably using it.",
            PictureReason::Timeout => "Your camera isn't answering. Another app is probably using it.",
        };
        CameraError { code: CameraErrorCode::NoPicture, reason: Some(reason), message: message.into() }
    }
}

impl CameraError {
    /// Like [`no_picture`](Self::no_picture), but for a real camera behind a
    /// capture card: black or flat frames mean the camera sends no HDMI
    /// picture, so the plain-English fallback points at the HDMI chain.
    pub fn no_picture_hdmi(reason: PictureReason) -> Self {
        let mut e = Self::no_picture(reason);
        if matches!(reason, PictureReason::Black | PictureReason::Blank) {
            e.message = coach_core::video::hdmi::no_signal_finding(if reason == PictureReason::Blank { coach_core::video::NoPicture::Blank } else { coach_core::video::NoPicture::Black }).fix;
        }
        e
    }
}

impl std::fmt::Display for CameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for CameraError {
    fn from(message: String) -> Self {
        CameraError { code: CameraErrorCode::Other, reason: None, message }
    }
}

impl From<&str> for CameraError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl From<CameraError> for String {
    fn from(e: CameraError) -> String {
        e.message
    }
}

/// How OBS stands towards the camera (part of `PictureStatus`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ObsCameraHold {
    pub running: bool,
    /// We can talk to OBS (WebSocket).
    pub connected: bool,
    /// The OBS video capture source that uses this camera.
    pub source: Option<String>,
    /// OBS has that source's camera open right now.
    pub holding: bool,
    /// "Free it for me" can work: connected, source found, and OBS holds it.
    pub can_free: bool,
}

/// `camera_picture_status`: can we see the camera, and if not, who has it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PictureStatus {
    pub live: bool,
    pub reason: Option<PictureReason>,
    /// Apps that have or probably have the camera, confirmed ones first.
    pub users: Vec<crate::platform::CameraUser>,
    pub obs: ObsCameraHold,
    /// We released an OBS source and have not put it back yet.
    pub freed_source: Option<String>,
}

/// `free_camera` / `restore_freed_camera` result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FreeCameraResult {
    pub ok: bool,
    pub source: Option<String>,
    /// Plain-language outcome (shown when `ok` is false).
    pub message: String,
}

/// `get_mains` / `set_mains`: the power-grid frequency used for anti-flicker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MainsInfo {
    /// In use: 50 or 60.
    pub hz: u8,
    /// "override" | "time_zone" | "region" | "default".
    pub source: crate::platform::MainsSource,
    /// What we would use without an override.
    pub detected_hz: u8,
}

/// What `restore_backup` would do with one saved item (`preview_restore`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestorePreviewItem {
    /// Vault key, the same id `restore_backup` reports it under.
    pub key: String,
    /// Same values as `BackupItem.kind`.
    pub kind: String,
    /// Plain item name, as in `BackupInfo.items`.
    pub label: String,
    /// `"change"` = will be put back; `"skip"` = will be left alone (see
    /// `note`); `"needs_obs_closed"` = an OBS file that can only be put back
    /// while OBS is closed.
    pub action: String,
    /// Plain reason for `skip` / `needs_obs_closed`; null for `change`.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestorePreview {
    pub id: String,
    pub label: String,
    /// Newest item first, in the order restore goes through them.
    pub items: Vec<RestorePreviewItem>,
    /// OBS (the program) is running right now.
    pub obs_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifyResult {
    pub before: MicReport,
    pub after: MicReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureStepResult {
    pub frames: usize,
    pub faces_found: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraReport {
    pub metrics: VideoMetrics,
    pub findings: Vec<Finding>,
    pub settings: CameraSettings,
    pub score: Score,
    pub class: CameraClass,
}

/// Payload of the `health-sample` event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthSampleView {
    pub sample: coach_core::health::StatsSample,
    pub window: coach_core::health::WindowStats,
    pub active: Vec<coach_core::health::AlertKind>,
}

/// Which coaching plan to build.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoachArea {
    Mic,
    Camera,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenKind {
    WindowsSettings,
    VendorApp,
    Obs,
}

/// Something the user has to click in another app. We open it next to us,
/// show `steps` in the always-on-top mini coach, and come back to the front
/// when the step's check passes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenTarget {
    pub kind: OpenKind,
    /// `ms-settings:` URI, a URL, or an app executable / protocol.
    pub uri_or_app: String,
    pub label: String,
    pub steps: Vec<String>,
}

/// One step of a coaching plan (see API.md "Live coaching").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoachStep {
    pub id: String,
    pub finding_id: Option<String>,
    pub task: coach_core::coach::CoachTaskKind,
    pub title: String,
    /// Plain words + why it matters.
    pub explain: String,
    /// UI animation key.
    pub animation: String,
    /// The app adjusts it itself (Windows mic level, GoXLR, camera settings).
    pub auto: bool,
    pub open_target: Option<OpenTarget>,
    /// `CheckSetting` predicate id (see `AppCore::check_passes`).
    pub check: Option<String>,
}

/// Payload of `coach-camera-overlay`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraOverlay {
    pub frame: String,
    pub face: Option<coach_core::video::Rect>,
    pub paper: Option<coach_core::video::Rect>,
    pub clipped_mask: Option<String>,
    /// Arrows in normalised 0..1 frame coordinates (origin top-left).
    pub arrows: Vec<coach_core::coach::Arrow>,
    /// Face exposure is OK right now (box turns green).
    pub face_ok: bool,
    /// Frame size the rectangles refer to.
    pub width: usize,
    pub height: usize,
}

/// Event names (keep in sync with API.md).
pub mod events {
    pub const COACH_UPDATE: &str = "coach-update";
    pub const COACH_CAMERA_OVERLAY: &str = "coach-camera-overlay";
    pub const MIC_LEVEL: &str = "mic-level";
    pub const CAMERA_PREVIEW: &str = "camera-preview";
    /// `{ live: bool, reason?: PictureReason }`, sent when the camera picture is lost or comes back.
    pub const CAMERA_PICTURE: &str = "camera-picture";
    /// `{ t_s, level_db, claps, clap: bool, frames }`, ~10/s during the lip-sync clap test.
    pub const SYNC_PROGRESS: &str = "sync-progress";
    /// Drift check: the wait is over, time for the next clap check. `{ points }`.
    pub const DRIFT_DUE: &str = "drift-due";
    pub const HEALTH_ALERT: &str = "health-alert";
    pub const HEALTH_SAMPLE: &str = "health-sample";
}
