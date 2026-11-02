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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApplyFailure {
    pub id: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ApplyResult {
    pub applied: Vec<String>,
    pub failed: Vec<ApplyFailure>,
    pub restart_obs_needed: bool,
}

impl ApplyResult {
    pub fn ok(&mut self, id: impl Into<String>) {
        self.applied.push(id.into());
    }
    pub fn fail(&mut self, id: impl Into<String>, error: impl Into<String>) {
        self.failed.push(ApplyFailure { id: id.into(), error: error.into() });
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

/// Event names (keep in sync with API.md).
pub mod events {
    pub const MIC_LEVEL: &str = "mic-level";
    pub const CAMERA_PREVIEW: &str = "camera-preview";
    pub const HEALTH_ALERT: &str = "health-alert";
    pub const HEALTH_SAMPLE: &str = "health-sample";
}
