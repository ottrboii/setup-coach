//! Windows audio settings rules.

use super::ids;
use super::state::{DuckingPreference, SystemSnapshot};
use crate::finding::{Area, Finding, FixKind, Severity};

pub(crate) fn check(s: &SystemSnapshot, out: &mut Vec<Finding>) {
    // GoXLR owners: the open-source GoXLR Utility comes first.
    if s.processing.goxlr_app_running && !s.processing.goxlr_utility_running {
        out.push(Finding::new(
            ids::GOXLR_OFFICIAL_APP,
            Area::System,
            Severity::Major,
            "Switch your GoXLR to GoXLR Utility",
            "The official GoXLR App is no longer maintained and its firmware is out of date. GoXLR Utility is the free, open-source replacement — and it lets Setup Coach set your mic gain and phantom power for you.",
            "Install GoXLR Utility and uninstall the official GoXLR App (the two can't run at the same time). We'll walk you through it step by step.",
            FixKind::Guided,
        ));
    }
    let Some(win) = &s.windows else { return };

    if !win.mic_privacy_desktop_allowed {
        out.push(Finding::new(
            ids::MIC_PRIVACY,
            Area::System,
            Severity::Critical,
            "Windows is blocking apps from using your mic",
            "The Windows privacy setting \"Let desktop apps access your microphone\" is off, so OBS hears silence.",
            "Open Settings > Privacy > Microphone and turn on access for desktop apps. We'll open the page for you.",
            FixKind::Guided,
        ));
    }

    if let Some(mic) = s.mic_endpoint() {
        if mic.voice_clarity_enabled {
            out.push(
                Finding::new(
                    ids::VOICE_CLARITY,
                    Area::System,
                    Severity::Major,
                    "Windows is reshaping your voice with an AI effect",
                    "Windows 11 \"Voice Clarity\" runs AI processing on your mic. It can make you sound muffled, warbly or pumping, and it fights your own settings.",
                    "Turn Voice Clarity off for this mic. We can do it for you.",
                    FixKind::Auto,
                ),
            );
        }
        if mic.enhancements_enabled {
            out.push(Finding::new(
                ids::ENHANCEMENTS,
                Area::System,
                Severity::Major,
                "Windows audio enhancements are changing your mic",
                "\"Audio enhancements\" add the driver's own noise and echo processing on top of everything else, which can sound muffled or pumping.",
                "Turn audio enhancements off for this mic. We can do it for you.",
                FixKind::Auto,
            ));
        }

        if let Some(obs) = &s.obs {
            let obs_rate = obs.sample_rate;
            if mic.sample_rate != obs_rate || obs_rate != 48_000 {
                let (fix, kind) = if obs_rate != 48_000 {
                    (
                        "Set everything to 48 kHz. We'll switch OBS to 48 kHz; then set your mic to 48 kHz in Windows Sound settings (we'll show you where) and restart OBS.",
                        FixKind::Auto,
                    )
                } else {
                    (
                        "Set your mic to 48 kHz in Windows Sound settings (we'll show you where), then restart OBS.",
                        FixKind::Guided,
                    )
                };
                out.push(
                    Finding::new(
                        ids::SAMPLE_RATE_MISMATCH,
                        Area::System,
                        Severity::Major,
                        "Your mic and OBS run at different sample rates",
                        "When Windows, your mic and OBS don't all use 48 kHz, audio can crackle, sound robotic or slowly drift out of sync.",
                        fix,
                        kind,
                    )
                    .with_evidence("mic_sample_rate_hz", mic.sample_rate as f64)
                    .with_evidence("obs_sample_rate_hz", obs_rate as f64),
                );
            }
        }

        if let Some(boost) = mic.mic_boost_db {
            // Onboard inputs have weak preamps: up to +20 dB boost is legitimate there.
            let limit = if mic.is_onboard { 20.0 } else { 0.0 };
            if boost > limit + 0.01 {
                out.push(
                    Finding::new(
                        ids::MIC_BOOST,
                        Area::System,
                        if boost >= 20.0 { Severity::Major } else { Severity::Minor },
                        "Windows \"Microphone Boost\" is adding hiss",
                        "Microphone Boost is digital gain added after your mic. It turns up the hiss together with your voice.",
                        &format!("Set Microphone Boost to 0 dB and get your level from {}. We can set it for you.", s.mic.level_source()),
                        FixKind::Auto,
                    )
                    .with_evidence("mic_boost_db", boost as f64),
                );
            }
        }

        if mic.endpoint_volume < 0.98 && !s.mic.windows_slider_is_hardware_gain {
            out.push(
                Finding::new(
                    ids::ENDPOINT_VOLUME,
                    Area::System,
                    Severity::Minor,
                    "Your Windows mic level is turned down",
                    "Turning the Windows mic level down throws away quality before OBS even gets your voice. Gain belongs on the mic or interface.",
                    "Set the Windows mic level to 100 and adjust gain on the mic or interface. We can set it for you.",
                    FixKind::Auto,
                )
                .with_evidence("endpoint_volume_percent", (mic.endpoint_volume * 100.0).round() as f64),
            );
        }

        if mic.exclusive_mode_allowed {
            out.push(Finding::new(
                ids::EXCLUSIVE_MODE,
                Area::System,
                Severity::Minor,
                "One app can grab your mic and silence it for the others",
                "\"Allow applications to take exclusive control\" lets a game or app lock the mic so OBS or Discord suddenly hear nothing.",
                "Turn off exclusive control for this mic. We can do it for you.",
                FixKind::Auto,
            ));
        }

        if mic.listen_to_device {
            out.push(Finding::new(
                ids::LISTEN_TO_DEVICE,
                Area::System,
                Severity::Major,
                "You (and maybe viewers) hear your voice twice",
                "\"Listen to this device\" plays your mic back through your speakers or headphones, slightly late. If desktop audio is captured, viewers hear an echo too.",
                "Turn off \"Listen to this device\". We can do it for you.",
                FixKind::Auto,
            ));
        }

        if mic.is_bluetooth_handsfree {
            out.push(
                Finding::new(
                    ids::BLUETOOTH_HANDSFREE,
                    Area::System,
                    Severity::Critical,
                    "Your mic is a Bluetooth headset in phone-call mode",
                    "Bluetooth headsets switch to \"Hands-Free\" when their mic is used: your voice drops to phone-call quality and your headphones go mono and dull.",
                    "Use a wired headset, a 2.4 GHz wireless dongle, or a separate USB mic, then select it here.",
                    FixKind::Guided,
                )
                .suggesting("usb_mic")
                .declinable(true),
            );
        }
    }

    if let Some(render) = win.default_render() {
        if render.is_bluetooth_handsfree && !s.mic_endpoint().is_some_and(|m| m.is_bluetooth_handsfree) {
            out.push(
                Finding::new(
                    ids::BLUETOOTH_HANDSFREE,
                    Area::System,
                    Severity::Major,
                    "Your headphones are stuck in Bluetooth phone-call mode",
                    "Your Bluetooth headphones are in \"Hands-Free\" mode, so everything you hear (game, music, friends) is mono and muffled.",
                    "Pick the headphones' \"Stereo\" output instead of \"Hands-Free\", and don't use the headset's own mic.",
                    FixKind::Guided,
                ),
            );
        }
    }

    match win.ducking {
        DuckingPreference::DoNothing => {}
        d => {
            out.push(Finding::new(
                ids::DUCKING,
                Area::System,
                if matches!(d, DuckingPreference::MuteOthers | DuckingPreference::Reduce80) {
                    Severity::Major
                } else {
                    Severity::Minor
                },
                "Windows turns your game and music down when Discord is active",
                "The Windows Communications setting lowers other sounds whenever it thinks you're on a call, so your stream audio jumps up and down.",
                "Set Windows to \"Do nothing\" when it detects communications. We can set it for you.",
                FixKind::Auto,
            ));
        }
    }

    if win.usb_selective_suspend_enabled {
        out.push(Finding::new(
            ids::USB_SUSPEND,
            Area::System,
            Severity::Minor,
            "Windows may switch off your USB mic or camera mid-stream",
            "USB power saving (selective suspend) can briefly cut USB audio and video devices, causing dropouts.",
            "Turn off USB selective suspend in your power plan. We can do it for you.",
            FixKind::Auto,
        ));
    }

    if win.level_changed_by_other_app {
        out.push(Finding::new(
            ids::LEVEL_HIJACKED,
            Area::System,
            Severity::Major,
            "Another app keeps changing your mic level",
            "Something (usually Discord's \"Automatic gain control\") moved your mic level during the test, so your volume will keep shifting on stream.",
            "In Discord: Settings > Voice & Video, turn off \"Automatic Gain Control\". Turn off auto-level in other voice apps too.",
            FixKind::Guided,
        ));
    }
}
