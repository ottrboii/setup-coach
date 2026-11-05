//! System configuration audit: Windows audio settings, the OBS mic chain,
//! the camera pipeline and stream encoder settings.
//!
//! Input is a [`state::SystemSnapshot`] collected by the Windows app; output is
//! ranked [`Finding`]s plus [`actions::quick_fixes`] the app can execute.

pub mod actions;
pub mod camera;
pub mod connection;
pub mod diff;
pub mod encoder;
pub mod ids;
pub mod obs_audio;
pub mod state;
mod windows;

#[cfg(test)]
mod tests;

pub use actions::{quick_fixes, Action, PlannedAction};
pub use connection::{ConnectionState, DeviceHolder, HeldDevice, UsbLink, UsbRole, UsbSpeed};
pub use diff::snapshot_diff;
pub use state::*;

use crate::constraints::Constraint;
use crate::finding::{rank, Finding, Severity};

/// Run every configuration rule and return ranked findings.
///
/// `constraints` are the user's "I can't do this" answers; they soften
/// findings that can't be acted on.
pub fn audit(snapshot: &SystemSnapshot, constraints: &[Constraint]) -> Vec<Finding> {
    let mut out = Vec::new();
    windows::check(snapshot, &mut out);
    obs_audio::check(snapshot, &mut out);
    camera::check(snapshot, &mut out);
    connection::check(snapshot, &mut out);
    encoder::check(snapshot, constraints, &mut out);

    if constraints.contains(&Constraint::CantAddCleanGain) {
        // Digital gain is the only option left: keep it, but stop nagging.
        for f in out.iter_mut().filter(|f| f.id == ids::GAIN_STACKED) {
            f.severity = Severity::Info;
            f.fix = "Your interface can't give this mic enough clean gain, so some digital boost is needed. We've set it so the hiss stays as low as possible; an inline booster would remove it.".into();
        }
    }

    // De-duplicate identical findings (same id and title).
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|f| seen.insert((f.id.clone(), f.title.clone())));
    rank(&mut out);
    out
}
