//! "I can't do this" answers. When a user declines a physical fix, the
//! recommenders recalibrate to get the most out of the current setup.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Constraint {
    // Audio
    /// Mic must stay where it is (e.g. out of camera frame).
    CantMoveMicCloser,
    /// Interface can't give enough clean gain and no booster/new interface now.
    CantAddCleanGain,
    /// Fan/AC/PC noise source must stay.
    CantRemoveNoiseSource,
    /// Desk/monitor reflection can't be fixed (no boom arm, can't angle).
    CantFixReflection,
    /// Room can't be treated (no rugs/blankets/panels).
    CantTreatRoom,
    /// User moves around a lot while streaming.
    MovesALot,
    /// No pop filter / can't angle the mic off-axis.
    CantFixPlosives,
    // Camera
    /// No extra light can be added.
    CantAddLight,
    /// Window/bright source behind the user can't be moved or covered.
    CantMoveBacklight,
    /// Mixed colour temperatures can't be unified.
    CantFixMixedLight,
    /// Camera can't be raised to eye level / moved.
    CantMoveCamera,
    // Stream
    /// Must stay on Wi-Fi.
    MustUseWifi,
}
