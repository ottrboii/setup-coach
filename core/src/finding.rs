//! The shared "problem found" type every analyser produces.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which part of the setup a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    Mic,
    Camera,
    Stream,
    System,
}

/// How much a finding hurts what viewers hear/see. Drives ranking and the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing, no score impact.
    Info,
    /// Noticeable to a careful listener/viewer.
    Minor,
    /// Clearly noticeable to viewers.
    Major,
    /// Breaks the stream experience (silence, one ear, constant clipping, no camera).
    Critical,
}

impl Severity {
    /// Points deducted from a 100-point area score.
    pub fn weight(self) -> f64 {
        match self {
            Severity::Info => 0.0,
            Severity::Minor => 5.0,
            Severity::Major => 12.0,
            Severity::Critical => 25.0,
        }
    }
}

/// Who carries out the fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixKind {
    /// The app can apply it itself (OBS filter, Windows setting, UVC control).
    Auto,
    /// The user clicks something the app points to.
    Guided,
    /// A physical change (move the mic, add a light, swap a cable).
    Manual,
}

/// One problem found during a scan, in plain language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable machine id, e.g. `mic.left_channel_only`. Used for UI keys,
    /// tests, de-duplication and constraint handling.
    pub id: String,
    pub area: Area,
    pub severity: Severity,
    /// What the user/viewer would say, e.g. "Viewers only hear you in your left ear".
    pub title: String,
    /// Why it happens, one or two plain sentences.
    pub why: String,
    /// What to do, physical fix first, then the setting.
    pub fix: String,
    pub fix_kind: FixKind,
    /// The user may answer "I can't do this", which triggers recalibration.
    pub can_decline: bool,
    /// Measured numbers behind the finding (shown in the Advanced drawer).
    #[serde(default)]
    pub evidence: BTreeMap<String, f64>,
    /// Product category to suggest if the limit is physical (affiliate moment),
    /// e.g. `inline_booster`, `pop_filter`, `key_light`.
    #[serde(default)]
    pub suggest_product: Option<String>,
}

impl Finding {
    pub fn new(
        id: &str,
        area: Area,
        severity: Severity,
        title: &str,
        why: &str,
        fix: &str,
        fix_kind: FixKind,
    ) -> Self {
        Finding {
            id: id.to_string(),
            area,
            severity,
            title: title.to_string(),
            why: why.to_string(),
            fix: fix.to_string(),
            fix_kind,
            can_decline: matches!(fix_kind, FixKind::Manual),
            evidence: BTreeMap::new(),
            suggest_product: None,
        }
    }

    pub fn with_evidence(mut self, key: &str, value: f64) -> Self {
        self.evidence.insert(key.to_string(), value);
        self
    }

    pub fn suggesting(mut self, product: &str) -> Self {
        self.suggest_product = Some(product.to_string());
        self
    }

    pub fn declinable(mut self, yes: bool) -> Self {
        self.can_decline = yes;
        self
    }
}

/// Sort findings most severe first, then auto-fixable first (quick wins), then by id.
pub fn rank(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| fix_order(a.fix_kind).cmp(&fix_order(b.fix_kind)))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn fix_order(k: FixKind) -> u8 {
    match k {
        FixKind::Auto => 0,
        FixKind::Guided => 1,
        FixKind::Manual => 2,
    }
}
