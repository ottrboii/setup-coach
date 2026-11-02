//! Setup Coach core: platform-independent analysis and recommendation engine.
//!
//! Everything here is pure computation on data handed in by the Windows app
//! (audio samples, video frames, OBS/Windows state snapshots, device ids), so it
//! can be tested anywhere. No AI processing: measurements, rules and static
//! settings only.

pub mod audio;
pub mod coach;
pub mod constraints;
pub mod finding;
pub mod health;
pub mod profiles;
pub mod score;
pub mod system;
pub mod video;

pub use constraints::Constraint;
pub use finding::{Area, Finding, FixKind, Severity};
