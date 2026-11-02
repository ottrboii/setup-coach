//! "Something changed since last time": compare the snapshot saved after the
//! last good setup with today's, so Windows updates, new devices and other
//! apps can't silently undo the user's setup.

use super::ids;
use super::state::{DuckingPreference, SystemSnapshot};
use crate::finding::{Area, Finding, FixKind, Severity};

pub fn snapshot_diff(prev: &SystemSnapshot, cur: &SystemSnapshot) -> Vec<Finding> {
    let mut out = Vec::new();

    if let (Some(pw), Some(cw)) = (&prev.windows, &cur.windows) {
        if let (Some(p), Some(c)) = (pw.default_capture(), cw.default_capture()) {
            if p.id != c.id {
                out.push(Finding::new(
                    ids::CHANGED_DEFAULT_MIC,
                    Area::System,
                    Severity::Major,
                    &format!("Your default mic changed from \"{}\" to \"{}\"", p.name, c.name),
                    "Windows switched the default microphone (often after an update or plugging in a headset or webcam). Apps on \"Default\" now use the wrong mic.",
                    "Switch the default back, or pin OBS and Discord to your real mic.",
                    FixKind::Guided,
                ));
            }
        }
        if pw.ducking == DuckingPreference::DoNothing && cw.ducking != DuckingPreference::DoNothing {
            out.push(Finding::new(
                ids::CHANGED_DUCKING,
                Area::System,
                Severity::Minor,
                "Windows turned call ducking back on",
                "Windows is lowering other sounds during calls again, probably after an update.",
                "Set it back to \"Do nothing\". We can do it for you.",
                FixKind::Auto,
            ));
        }
    }

    // OBS mic device disappeared
    if let (Some(obs), Some(cw)) = (&cur.obs, &cur.windows) {
        if let Some(mic) = obs.main_mic() {
            if mic.device_id != "default" && !mic.device_id.is_empty() && !cw.endpoints.is_empty() && cw.endpoint(&mic.device_id).is_none() {
                out.push(Finding::new(
                    ids::CHANGED_OBS_MIC_MISSING,
                    Area::System,
                    Severity::Critical,
                    "OBS lost your mic",
                    "The mic OBS is set to isn't connected or got a new id (common after Windows updates or changing USB ports), so OBS records silence.",
                    "Plug the mic back into the same USB port, or re-select it in OBS. We can re-pin it once it's connected.",
                    FixKind::Guided,
                ));
            }
        }
    }

    // Same mic endpoint, settings changed
    if let (Some(p), Some(c)) = (prev.mic_endpoint(), cur.mic_endpoint()) {
        if p.id == c.id {
            if p.sample_rate != c.sample_rate {
                out.push(
                    Finding::new(
                        ids::CHANGED_SAMPLE_RATE,
                        Area::System,
                        Severity::Minor,
                        "Your mic's sample rate changed",
                        "Something changed your mic's format in Windows. If it no longer matches OBS (48 kHz), audio can crackle or drift.",
                        "Set the mic back to 48 kHz in Windows Sound settings.",
                        FixKind::Guided,
                    )
                    .with_evidence("previous_hz", p.sample_rate as f64)
                    .with_evidence("current_hz", c.sample_rate as f64),
                );
            }
            if (!p.enhancements_enabled && c.enhancements_enabled) || (!p.voice_clarity_enabled && c.voice_clarity_enabled) {
                out.push(Finding::new(
                    ids::CHANGED_ENHANCEMENTS,
                    Area::System,
                    Severity::Major,
                    "Windows switched voice effects back on",
                    "Audio enhancements or Voice Clarity are on again for your mic, probably after a Windows or driver update.",
                    "Turn them off again. We can do it for you.",
                    FixKind::Auto,
                ));
            }
            let boost_changed = (p.mic_boost_db.unwrap_or(0.0) - c.mic_boost_db.unwrap_or(0.0)).abs() > 0.5;
            let volume_changed = (p.endpoint_volume - c.endpoint_volume).abs() > 0.02;
            if boost_changed || volume_changed {
                out.push(
                    Finding::new(
                        ids::CHANGED_LEVEL,
                        Area::System,
                        Severity::Minor,
                        "Something changed your mic level",
                        "Your Windows mic level or boost is different from your saved setup, so you'll sound louder or quieter than before.",
                        "Re-apply your saved setup.",
                        FixKind::Guided,
                    )
                    .with_evidence("previous_volume_percent", (p.endpoint_volume * 100.0).round() as f64)
                    .with_evidence("current_volume_percent", (c.endpoint_volume * 100.0).round() as f64),
                );
            }
        }
    }

    // OBS mic filters edited
    let filters = |s: &SystemSnapshot| {
        s.obs
            .as_ref()
            .and_then(|o| o.main_mic())
            .map(|m| m.filters.iter().map(|f| (f.name.clone(), f.kind.clone(), f.enabled)).collect::<Vec<_>>())
    };
    if let (Some(p), Some(c)) = (filters(prev), filters(cur)) {
        if p != c {
            out.push(Finding::new(
                ids::CHANGED_OBS_FILTERS,
                Area::System,
                Severity::Info,
                "Your OBS mic filters changed",
                "The filters on your OBS mic are different from your saved setup.",
                "If you didn't change them on purpose, re-apply your saved setup.",
                FixKind::Guided,
            ));
        }
    }

    if let (Some(p), Some(c)) = (&prev.camera, &cur.camera) {
        if !p.studio_effects_on && c.studio_effects_on {
            out.push(Finding::new(
                ids::CHANGED_STUDIO_EFFECTS,
                Area::Camera,
                Severity::Major,
                "Windows switched camera AI effects back on",
                "Windows Studio Effects are active on your camera again, probably after an update.",
                "Turn Studio Effects off in Settings > Bluetooth & devices > Cameras.",
                FixKind::Guided,
            ));
        }
    }

    out
}
