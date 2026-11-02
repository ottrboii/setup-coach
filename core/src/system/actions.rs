//! Auto-applicable changes the Windows app executes, each tied to the
//! finding(s) it resolves.

use super::encoder::{safe_bitrate_for_upload, suggested_output_height, TWITCH_GUIDELINE_KBPS, AUDIO_BITRATE_RECOMMENDED};
use super::ids;
use super::obs_audio::{
    is_ai_suppressor, num, recommended_compressor, recommended_expander, recommended_gate, recommended_limiter,
    recommended_order,
};
use super::state::{Connection, DuckingPreference, MonitoringType, Platform, SystemSnapshot};
use crate::finding::{Finding, FixKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// One concrete change. `input`/`filter` are OBS source/filter names,
/// `endpoint` is a Windows endpoint id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    // OBS audio
    SetObsInputMono { input: String },
    SetObsInputMuted { input: String, muted: bool },
    RemoveObsFilter { input: String, filter: String },
    ReorderObsFilters { input: String, order: Vec<String> },
    SetObsFilterSettings { input: String, filter: String, settings: Value },
    AddObsFilter { input: String, name: String, kind: String, settings: Value },
    SetObsSampleRate { hz: u32 },
    SetObsMonitoring { input: String, monitoring: MonitoringType },
    PinObsMicDevice { input: String, device_id: String },
    EnableObsWebsocket,
    // OBS video / output
    SetObsVideoCaptureMode { input: String, width: u32, height: u32, fps: f32, format: String },
    SetObsStreamEncoder { encoder_id: String },
    SetObsRateControl { mode: String },
    SetObsKeyframeInterval { seconds: u32 },
    SetObsStreamBitrate { kbps: u32 },
    SetObsOutputResolution { width: u32, height: u32 },
    SetObsAudioBitrate { kbps: u32 },
    SetObsRecordingFormat { format: String },
    // Windows
    SetWindowsDucking { preference: DuckingPreference },
    DisableEnhancements { endpoint: String },
    DisableVoiceClarity { endpoint: String },
    SetMicBoost { endpoint: String, db: f32 },
    SetEndpointVolume { endpoint: String, volume: f32 },
    DisableExclusiveMode { endpoint: String },
    DisableListen { endpoint: String },
    DisableUsbSelectiveSuspend,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedAction {
    /// Finding ids this action resolves.
    pub resolves: Vec<String>,
    pub action: Action,
}

fn planned(id: &str, action: Action) -> PlannedAction {
    PlannedAction { resolves: vec![id.to_string()], action }
}

/// Turn the auto-fixable findings into concrete actions, ordered so they can
/// be executed top to bottom (removals → reorder → settings → additions).
pub fn quick_fixes(s: &SystemSnapshot, findings: &[Finding]) -> Vec<PlannedAction> {
    let has: BTreeSet<&str> = findings
        .iter()
        .filter(|f| f.fix_kind == FixKind::Auto)
        .map(|f| f.id.as_str())
        .collect();
    let on = |id: &str| has.contains(id);

    let mut removals = Vec::new();
    let mut reorder = Vec::new();
    let mut settings = Vec::new();
    let mut additions = Vec::new();

    let mic_ep = s.mic_endpoint().map(|e| e.id.clone());

    // ---- Windows
    if let Some(ep) = &mic_ep {
        if on(ids::VOICE_CLARITY) || (on(ids::CHANGED_ENHANCEMENTS) && s.mic_endpoint().is_some_and(|e| e.voice_clarity_enabled)) {
            settings.push(planned(ids::VOICE_CLARITY, Action::DisableVoiceClarity { endpoint: ep.clone() }));
        }
        if on(ids::ENHANCEMENTS) || (on(ids::CHANGED_ENHANCEMENTS) && s.mic_endpoint().is_some_and(|e| e.enhancements_enabled)) {
            settings.push(planned(ids::ENHANCEMENTS, Action::DisableEnhancements { endpoint: ep.clone() }));
        }
        if on(ids::MIC_BOOST) {
            let onboard = s.mic_endpoint().is_some_and(|e| e.is_onboard);
            settings.push(planned(ids::MIC_BOOST, Action::SetMicBoost { endpoint: ep.clone(), db: if onboard { 20.0 } else { 0.0 } }));
        }
        if on(ids::ENDPOINT_VOLUME) {
            settings.push(planned(ids::ENDPOINT_VOLUME, Action::SetEndpointVolume { endpoint: ep.clone(), volume: 1.0 }));
        }
        if on(ids::EXCLUSIVE_MODE) {
            settings.push(planned(ids::EXCLUSIVE_MODE, Action::DisableExclusiveMode { endpoint: ep.clone() }));
        }
        if on(ids::LISTEN_TO_DEVICE) {
            settings.push(planned(ids::LISTEN_TO_DEVICE, Action::DisableListen { endpoint: ep.clone() }));
        }
    }
    if on(ids::DUCKING) {
        settings.push(planned(ids::DUCKING, Action::SetWindowsDucking { preference: DuckingPreference::DoNothing }));
    }
    if on(ids::USB_SUSPEND) {
        settings.push(planned(ids::USB_SUSPEND, Action::DisableUsbSelectiveSuspend));
    }
    if on(ids::SAMPLE_RATE_MISMATCH) && s.obs.as_ref().is_some_and(|o| o.sample_rate != 48_000) {
        settings.push(planned(ids::SAMPLE_RATE_MISMATCH, Action::SetObsSampleRate { hz: 48_000 }));
    }

    // ---- OBS
    if let Some(obs) = &s.obs {
        if on(ids::WEBSOCKET_OFF) {
            settings.push(planned(ids::WEBSOCKET_OFF, Action::EnableObsWebsocket));
        }

        if let Some(mic) = obs.main_mic() {
            let input = mic.name.clone();
            let ctx = &s.mic;
            let mut removed = BTreeSet::new();

            if on(ids::AI_SUPPRESSION) {
                for f in mic.filters.iter().filter(|f| f.enabled && is_ai_suppressor(f)) {
                    removed.insert(f.name.clone());
                    removals.push(planned(ids::AI_SUPPRESSION, Action::RemoveObsFilter { input: input.clone(), filter: f.name.clone() }));
                }
            }
            if on(ids::FILTER_ORDER) {
                let order: Vec<String> = recommended_order(mic).into_iter().filter(|n| !removed.contains(n)).collect();
                reorder.push(planned(ids::FILTER_ORDER, Action::ReorderObsFilters { input: input.clone(), order }));
            }
            if on(ids::MIC_MUTED) {
                settings.push(planned(ids::MIC_MUTED, Action::SetObsInputMuted { input: input.clone(), muted: false }));
            }
            if on(ids::OBS_DEFAULT_DEVICE) {
                if let Some(id) = s.windows.as_ref().and_then(|w| w.default_capture()).map(|e| e.id.clone()) {
                    settings.push(planned(ids::OBS_DEFAULT_DEVICE, Action::PinObsMicDevice { input: input.clone(), device_id: id }));
                }
            }
            if on(ids::NOT_MONO) {
                settings.push(planned(ids::NOT_MONO, Action::SetObsInputMono { input: input.clone() }));
            }
            for id in [ids::MONITORING_DOUBLED, ids::MONITORING_DELAYED] {
                if on(id) {
                    settings.push(planned(id, Action::SetObsMonitoring { input: input.clone(), monitoring: MonitoringType::Off }));
                }
            }
            let find = |kind: &str| mic.filters.iter().find(|f| f.enabled && f.kind == kind);
            if on(ids::GATE_CHOPS) {
                if let Some(g) = find("noise_gate_filter") {
                    let rec = recommended_gate(ctx.measured_noise_floor_dbfs, ctx.measured_quiet_speech_dbfs);
                    settings.push(planned(ids::GATE_CHOPS, Action::SetObsFilterSettings { input: input.clone(), filter: g.name.clone(), settings: merge(&g.settings, rec) }));
                }
            } else if on(ids::GATE_SLOW) {
                if let Some(g) = find("noise_gate_filter") {
                    settings.push(planned(ids::GATE_SLOW, Action::SetObsFilterSettings {
                        input: input.clone(),
                        filter: g.name.clone(),
                        settings: merge(&g.settings, json!({ "attack_time": 5, "hold_time": 200 })),
                    }));
                }
            }
            if on(ids::EXPANDER_CHOPS) {
                if let Some(e) = find("expander_filter") {
                    let rec = recommended_expander(ctx.measured_noise_floor_dbfs, ctx.measured_quiet_speech_dbfs, &e.settings);
                    settings.push(planned(ids::EXPANDER_CHOPS, Action::SetObsFilterSettings { input: input.clone(), filter: e.name.clone(), settings: rec }));
                }
            }
            if on(ids::COMPRESSOR_EXTREME) {
                if let Some(c) = find("compressor_filter") {
                    settings.push(planned(ids::COMPRESSOR_EXTREME, Action::SetObsFilterSettings { input: input.clone(), filter: c.name.clone(), settings: recommended_compressor(&c.settings) }));
                }
            }
            if on(ids::SPEEX_SUPPRESSION) {
                if let Some(f) = mic.filters.iter().find(|f| f.enabled && f.kind.starts_with("noise_suppress_filter") && !is_ai_suppressor(f)) {
                    let level = num(&f.settings, "suppress_level", -30.0).max(-10.0);
                    settings.push(planned(ids::SPEEX_SUPPRESSION, Action::SetObsFilterSettings {
                        input: input.clone(),
                        filter: f.name.clone(),
                        settings: merge(&f.settings, json!({ "method": "speex", "suppress_level": level })),
                    }));
                }
            }
            if on(ids::LIMITER_LEVEL) {
                if let Some(l) = find("limiter_filter") {
                    settings.push(planned(ids::LIMITER_LEVEL, Action::SetObsFilterSettings { input: input.clone(), filter: l.name.clone(), settings: merge(&l.settings, recommended_limiter()) }));
                }
            }
            if on(ids::LIMITER_MISSING) {
                additions.push(planned(ids::LIMITER_MISSING, Action::AddObsFilter {
                    input: input.clone(),
                    name: "Limiter (Setup Coach)".into(),
                    kind: "limiter_filter".into(),
                    settings: recommended_limiter(),
                }));
            }
        }

        // Camera sources
        for c in &obs.video_captures {
            if on(ids::CAM_DEVICE_DEFAULT) && !c.res_type_custom {
                settings.push(planned(ids::CAM_DEVICE_DEFAULT, Action::SetObsVideoCaptureMode {
                    input: c.name.clone(), width: 1920, height: 1080, fps: 30.0, format: "MJPEG".into(),
                }));
            } else if on(ids::CAM_YUY2_HIGH_RES) && super::camera::uncompressed_too_heavy(c) {
                settings.push(planned(ids::CAM_YUY2_HIGH_RES, Action::SetObsVideoCaptureMode {
                    input: c.name.clone(), width: c.width, height: c.height, fps: c.fps.unwrap_or(30.0), format: "MJPEG".into(),
                }));
            }
        }

        // Encoder
        if let Some(e) = &obs.encoder {
            if on(ids::ENC_CPU_WITH_GPU) {
                if let Some(id) = s.hardware.as_ref().and_then(|h| h.best_hw_encoder()) {
                    settings.push(planned(ids::ENC_CPU_WITH_GPU, Action::SetObsStreamEncoder { encoder_id: id.into() }));
                }
            }
            if on(ids::ENC_NOT_CBR) {
                settings.push(planned(ids::ENC_NOT_CBR, Action::SetObsRateControl { mode: "CBR".into() }));
            }
            if on(ids::ENC_KEYFRAME) {
                settings.push(planned(ids::ENC_KEYFRAME, Action::SetObsKeyframeInterval { seconds: 2 }));
            }
            // One bitrate change satisfying every bitrate finding.
            let mut bitrate = e.bitrate_kbps;
            let mut resolves = Vec::new();
            if on(ids::ENC_OVER_PLATFORM) && obs.platform == Platform::Twitch {
                bitrate = bitrate.min(TWITCH_GUIDELINE_KBPS);
                resolves.push(ids::ENC_OVER_PLATFORM.to_string());
            }
            if on(ids::ENC_OVER_UPLOAD) {
                if let Some(up) = s.network.as_ref().and_then(|n| n.measured_upload_kbps) {
                    let wifi = s.network.as_ref().is_some_and(|n| n.connection == Connection::Wifi);
                    bitrate = bitrate.min(safe_bitrate_for_upload(up, e.audio_bitrate_kbps, wifi));
                    resolves.push(ids::ENC_OVER_UPLOAD.to_string());
                }
            }
            if bitrate != e.bitrate_kbps {
                settings.push(PlannedAction { resolves, action: Action::SetObsStreamBitrate { kbps: bitrate } });
            }
            if on(ids::ENC_LOW_FOR_RES) || bitrate != e.bitrate_kbps {
                let h = suggested_output_height(bitrate, e.fps);
                if h < e.output_height {
                    let w = ((h as f64 * 16.0 / 9.0) / 2.0).round() as u32 * 2;
                    settings.push(planned(ids::ENC_LOW_FOR_RES, Action::SetObsOutputResolution { width: w, height: h }));
                }
            }
            if on(ids::ENC_AUDIO_LOW) {
                settings.push(planned(ids::ENC_AUDIO_LOW, Action::SetObsAudioBitrate { kbps: AUDIO_BITRATE_RECOMMENDED }));
            }
        }
        if on(ids::REC_MP4) {
            let fmt = if obs.major_version().is_some_and(|v| v >= 30) { "hybrid_mp4" } else { "mkv" };
            settings.push(planned(ids::REC_MP4, Action::SetObsRecordingFormat { format: fmt.into() }));
        }
    }

    removals.into_iter().chain(reorder).chain(settings).chain(additions).collect()
}

fn merge(existing: &Value, new: Value) -> Value {
    let mut out = existing.as_object().cloned().unwrap_or_default();
    if let Value::Object(n) = new {
        for (k, v) in n {
            out.insert(k, v);
        }
    }
    Value::Object(out)
}
