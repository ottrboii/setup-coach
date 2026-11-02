//! Stream encoder / output configuration rules.

use super::ids;
use super::state::{Connection, ObsEncoder, Platform, SystemSnapshot};
use crate::constraints::Constraint;
use crate::finding::{Area, Finding, FixKind, Severity};

/// Twitch's published broadcasting guideline for 1080p60 H.264 (kbps).
/// Enhanced Broadcasting / HEVC may allow more; that is unconfirmed, so
/// Enhanced Broadcasting setups are exempt from this rule.
pub const TWITCH_GUIDELINE_KBPS: u32 = 6000;
/// Below this many bits per pixel H.264 gets visibly blocky in motion.
pub const BPP_MINOR: f64 = 0.045;
pub const BPP_MAJOR: f64 = 0.03;
/// Target bits per pixel when suggesting a lower output resolution.
pub const BPP_TARGET: f64 = 0.05;
/// Share of measured upload a stream may use (wired / Wi-Fi).
pub const UPLOAD_SHARE_WIRED: f64 = 0.75;
pub const UPLOAD_SHARE_WIFI: f64 = 0.6;
pub const AUDIO_BITRATE_MIN: u32 = 128;
pub const AUDIO_BITRATE_RECOMMENDED: u32 = 160;

pub fn bits_per_pixel(e: &ObsEncoder) -> Option<f64> {
    let px = e.output_width as f64 * e.output_height as f64 * e.fps as f64;
    (px > 0.0 && e.bitrate_kbps > 0).then(|| e.bitrate_kbps as f64 * 1000.0 / px)
}

/// Largest common 16:9 output height that reaches the target bits per pixel.
pub fn suggested_output_height(bitrate_kbps: u32, fps: f32) -> u32 {
    for h in [1080u32, 936, 864, 720, 540, 480] {
        let w = (h as f64 * 16.0 / 9.0).round();
        if bitrate_kbps as f64 * 1000.0 / (w * h as f64 * fps.max(1.0) as f64) >= BPP_TARGET {
            return h;
        }
    }
    480
}

/// Safe stream video bitrate for the measured upload, rounded down to 250 kbps.
pub fn safe_bitrate_for_upload(upload_kbps: u32, audio_kbps: u32, wifi: bool) -> u32 {
    let share = if wifi { UPLOAD_SHARE_WIFI } else { UPLOAD_SHARE_WIRED };
    let budget = (upload_kbps as f64 * share - audio_kbps as f64).max(1000.0);
    ((budget / 250.0).floor() * 250.0) as u32
}

pub(crate) fn check(s: &SystemSnapshot, constraints: &[Constraint], out: &mut Vec<Finding>) {
    let must_wifi = constraints.contains(&Constraint::MustUseWifi);
    let on_wifi = s.network.as_ref().is_some_and(|n| n.connection == Connection::Wifi);

    if on_wifi {
        let sev = if must_wifi { Severity::Info } else { Severity::Major };
        out.push(
            Finding::new(
                ids::NET_WIFI,
                Area::Stream,
                sev,
                "You're streaming over Wi-Fi",
                "Wi-Fi speed jumps around and drops packets, which shows up as dropped frames and buffering for viewers.",
                "Plug your PC into the router with an Ethernet cable (or a powerline adapter). If you can't, we'll keep your bitrate lower to leave headroom.",
                FixKind::Manual,
            )
            .suggesting("ethernet_cable"),
        );
    }

    let Some(obs) = &s.obs else { return };
    let Some(e) = &obs.encoder else {
        recording(obs, out);
        return;
    };

    if !e.enhanced_broadcasting {
        if let Some(hw) = &s.hardware {
            if e.is_cpu_encoder() && hw.best_hw_encoder().is_some() {
                out.push(
                    Finding::new(
                        ids::ENC_CPU_WITH_GPU,
                        Area::Stream,
                        if hw.cpu_cores <= 8 { Severity::Major } else { Severity::Minor },
                        "Your processor is encoding the stream while your graphics card could",
                        "Software (x264) encoding competes with your game for the CPU and causes encoding lag. Your graphics card has a dedicated encoder that does this almost for free.",
                        "Switch the stream encoder to your graphics card's encoder (NVENC / AMF / QuickSync). We can switch it for you.",
                        FixKind::Auto,
                    )
                    .with_evidence("cpu_cores", hw.cpu_cores as f64),
                );
            }
        }
        if !e.rate_control.eq_ignore_ascii_case("cbr") && !e.rate_control.is_empty() {
            out.push(Finding::new(
                ids::ENC_NOT_CBR,
                Area::Stream,
                Severity::Major,
                "Your stream bitrate isn't constant",
                "Live platforms expect a constant bitrate (CBR). Variable bitrate spikes in busy scenes and can cause buffering and dropped frames.",
                "Set rate control to CBR. We can set it for you.",
                FixKind::Auto,
            ));
        }
        if e.keyframe_sec != 2 {
            out.push(
                Finding::new(
                    ids::ENC_KEYFRAME,
                    Area::Stream,
                    Severity::Minor,
                    "Your keyframe interval isn't 2 seconds",
                    "Twitch and most platforms expect a keyframe every 2 seconds. Other values cause slow starts, quality drops or warnings.",
                    "Set the keyframe interval to 2 s. We can set it for you.",
                    FixKind::Auto,
                )
                .with_evidence("keyframe_sec", e.keyframe_sec as f64),
            );
        }
        if obs.platform == Platform::Twitch && e.bitrate_kbps > TWITCH_GUIDELINE_KBPS {
            out.push(
                Finding::new(
                    ids::ENC_OVER_PLATFORM,
                    Area::Stream,
                    Severity::Major,
                    "Your bitrate is above Twitch's guideline",
                    "Twitch recommends at most 6000 kbps. Above that, viewers without transcoding options may buffer.",
                    "Lower the stream bitrate to 6000 kbps (or use Twitch Enhanced Broadcasting). We can set it for you.",
                    FixKind::Auto,
                )
                .with_evidence("bitrate_kbps", e.bitrate_kbps as f64),
            );
        }
        if let Some(bpp) = bits_per_pixel(e) {
            if bpp < BPP_MINOR {
                let suggest = suggested_output_height(e.bitrate_kbps.min(platform_cap(obs.platform)), e.fps);
                out.push(
                    Finding::new(
                        ids::ENC_LOW_FOR_RES,
                        Area::Stream,
                        if bpp < BPP_MAJOR { Severity::Major } else { Severity::Minor },
                        "Your stream will look blocky in fast scenes",
                        "Your bitrate is too low for the resolution and frame rate you send, so the encoder has to blur and block fast motion.",
                        &format!("Send a lower resolution that your bitrate can carry ({}p at {} fps). We can set it for you.", suggest, e.fps.round()),
                        FixKind::Auto,
                    )
                    .with_evidence("bits_per_pixel", (bpp * 1000.0).round() / 1000.0)
                    .with_evidence("suggested_height", suggest as f64),
                );
            }
        }
    }

    if let Some(up) = s.network.as_ref().and_then(|n| n.measured_upload_kbps) {
        let total = e.bitrate_kbps + e.audio_bitrate_kbps;
        let share = if on_wifi { UPLOAD_SHARE_WIFI } else { UPLOAD_SHARE_WIRED };
        if total as f64 > up as f64 * share {
            let safe = safe_bitrate_for_upload(up, e.audio_bitrate_kbps, on_wifi);
            out.push(
                Finding::new(
                    ids::ENC_OVER_UPLOAD,
                    Area::Stream,
                    Severity::Major,
                    "Your internet upload can't reliably carry this bitrate",
                    "Your stream uses most of your measured upload speed. Any dip (someone else online, Wi-Fi hiccup) drops frames for viewers.",
                    &format!("Lower the stream bitrate to about {safe} kbps. We can set it for you."),
                    FixKind::Auto,
                )
                .with_evidence("upload_kbps", up as f64)
                .with_evidence("stream_kbps", total as f64)
                .with_evidence("safe_video_kbps", safe as f64),
            );
        }
    }

    if e.audio_bitrate_kbps > 0 && e.audio_bitrate_kbps < AUDIO_BITRATE_MIN {
        out.push(
            Finding::new(
                ids::ENC_AUDIO_LOW,
                Area::Stream,
                Severity::Minor,
                "Your stream audio quality is set low",
                "Below 128 kbps AAC your voice and music get swirly, especially with game audio underneath.",
                "Set audio bitrate to 160 kbps. We can set it for you.",
                FixKind::Auto,
            )
            .with_evidence("audio_bitrate_kbps", e.audio_bitrate_kbps as f64),
        );
    }

    recording(obs, out);
}

fn platform_cap(p: Platform) -> u32 {
    if p == Platform::Twitch { TWITCH_GUIDELINE_KBPS } else { u32::MAX }
}

fn recording(obs: &super::state::ObsState, out: &mut Vec<Finding>) {
    if obs.recording_format.eq_ignore_ascii_case("mp4") {
        out.push(Finding::new(
            ids::REC_MP4,
            Area::Stream,
            Severity::Minor,
            "A crash would destroy your whole recording",
            "Plain MP4 files are unreadable if OBS or your PC crashes before the recording stops.",
            "Record as Hybrid MP4 (OBS 30+) or MKV instead. We can switch it for you.",
            FixKind::Auto,
        ));
    }
}
