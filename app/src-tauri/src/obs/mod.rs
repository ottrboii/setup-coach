//! OBS integration over obs-websocket v5: read the setup into a core
//! `ObsState`, apply filters/settings, read stats and screenshots.

pub mod client;
pub mod files;
pub mod mock;

use coach_core::audio::ProcessingChain;
use coach_core::health::StatsSample;
use coach_core::system::{
    ColorRange, MonitoringType, ObsAudioInput, ObsEncoder, ObsFilter, ObsState, ObsVideoCapture, Platform,
    OBS_KIND_APP_AUDIO, OBS_KIND_DESKTOP, OBS_KIND_MIC,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// One raw request: everything else is built on top, so tests only mock this.
#[async_trait::async_trait]
pub trait ObsApi: Send + Sync {
    async fn request(&self, request_type: &str, data: Value) -> Result<Value, String>;
    fn is_alive(&self) -> bool {
        true
    }
}

/// `plugin_config/obs-websocket/config.json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct WebsocketConfig {
    #[serde(default)]
    pub server_enabled: bool,
    #[serde(default = "default_port")]
    pub server_port: u16,
    #[serde(default)]
    pub server_password: String,
    #[serde(default)]
    pub auth_required: bool,
}

fn default_port() -> u16 {
    4455
}

pub fn read_websocket_config(obs_dir: &Path) -> Option<WebsocketConfig> {
    let p = obs_dir.join("plugin_config").join("obs-websocket").join("config.json");
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

// Names of the filters Setup Coach owns on the mic input.
pub const F_VOICE: &str = "Setup Coach Voice";
pub const F_GAIN: &str = "Setup Coach Gain";
pub const F_EXPANDER: &str = "Setup Coach Expander";
pub const F_COMPRESSOR: &str = "Setup Coach Compressor";
pub const F_LIMITER: &str = "Setup Coach Limiter";
pub const VST3_FILE: &str = "SetupCoachVoice.vst3";

pub fn monitoring_from_obs(s: &str) -> MonitoringType {
    match s {
        "OBS_MONITORING_TYPE_MONITOR_ONLY" => MonitoringType::MonitorOnly,
        "OBS_MONITORING_TYPE_MONITOR_AND_OUTPUT" => MonitoringType::MonitorAndOutput,
        _ => MonitoringType::Off,
    }
}

pub fn monitoring_to_obs(m: MonitoringType) -> &'static str {
    match m {
        MonitoringType::Off => "OBS_MONITORING_TYPE_NONE",
        MonitoringType::MonitorOnly => "OBS_MONITORING_TYPE_MONITOR_ONLY",
        MonitoringType::MonitorAndOutput => "OBS_MONITORING_TYPE_MONITOR_AND_OUTPUT",
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

async fn profile_param(obs: &dyn ObsApi, cat: &str, name: &str) -> Option<String> {
    let r = obs.request("GetProfileParameter", json!({ "parameterCategory": cat, "parameterName": name })).await.ok()?;
    r.get("parameterValue")
        .and_then(Value::as_str)
        .or_else(|| r.get("defaultParameterValue").and_then(Value::as_str))
        .map(str::to_string)
}

pub async fn filters_of(obs: &dyn ObsApi, source: &str) -> Result<Vec<ObsFilter>, String> {
    let r = obs.request("GetSourceFilterList", json!({ "sourceName": source })).await?;
    Ok(r["filters"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| ObsFilter {
                    name: s(f, "filterName"),
                    kind: s(f, "filterKind"),
                    enabled: f["filterEnabled"].as_bool().unwrap_or(true),
                    settings: f.get("filterSettings").cloned().unwrap_or(json!({})),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Read OBS's setup. `obs_dir` (when known) lets us fill in what the
/// WebSocket API can't report (advanced-mode encoder settings, mono flag).
pub async fn read_state(obs: &dyn ObsApi, obs_dir: Option<&Path>) -> Result<ObsState, String> {
    let ver = obs.request("GetVersion", Value::Null).await?;
    let mut st = ObsState { version: s(&ver, "obsVersion"), websocket_enabled: true, ..Default::default() };
    let inputs = obs.request("GetInputList", Value::Null).await?;
    let flags = obs_dir.and_then(files::source_flags);
    for i in inputs["inputs"].as_array().cloned().unwrap_or_default() {
        let name = s(&i, "inputName");
        let kind = {
            let u = s(&i, "unversionedInputKind");
            if u.is_empty() { s(&i, "inputKind") } else { u }
        };
        let settings = obs
            .request("GetInputSettings", json!({ "inputName": name }))
            .await
            .map(|r| r["inputSettings"].clone())
            .unwrap_or(json!({}));
        if [OBS_KIND_MIC, OBS_KIND_DESKTOP, OBS_KIND_APP_AUDIO].contains(&kind.as_str()) {
            let monitoring = obs
                .request("GetInputAudioMonitorType", json!({ "inputName": name }))
                .await
                .map(|r| monitoring_from_obs(r["monitorType"].as_str().unwrap_or_default()))
                .unwrap_or_default();
            let volume_db = obs
                .request("GetInputVolume", json!({ "inputName": name }))
                .await
                .ok()
                .and_then(|r| r["inputVolumeDb"].as_f64())
                .unwrap_or(0.0) as f32;
            let muted = obs.request("GetInputMute", json!({ "inputName": name })).await.ok().and_then(|r| r["inputMuted"].as_bool()).unwrap_or(false);
            let mono = flags.as_ref().and_then(|f| f.get(&name)).map(|f| f & files::FLAG_FORCE_MONO != 0).unwrap_or(false);
            st.audio_inputs.push(ObsAudioInput {
                name: name.clone(),
                kind: kind.clone(),
                device_id: s(&settings, "device_id"),
                device_name: s(&settings, "device_name"),
                mono_downmix: mono,
                monitoring,
                volume_db,
                muted,
                filters: filters_of(obs, &name).await.unwrap_or_default(),
                app_target: settings.get("window").and_then(Value::as_str).map(str::to_string),
            });
        } else if kind == "dshow_input" {
            let res = s(&settings, "resolution");
            let (w, h) = res.split_once('x').map(|(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(0))).unwrap_or((0, 0));
            let interval = settings["frame_interval"].as_f64();
            st.video_captures.push(ObsVideoCapture {
                name: name.clone(),
                device_id: s(&settings, "video_device_id"),
                device_name: s(&settings, "video_device_id").split(':').next().unwrap_or_default().to_string(),
                res_type_custom: settings["res_type"].as_i64() == Some(1),
                width: w,
                height: h,
                fps: interval.filter(|v| *v > 0.0).map(|v| (1e7 / v) as f32),
                video_format: settings["video_format"].as_i64().map(dshow_format_name).unwrap_or_default().to_string(),
                color_range: match s(&settings, "color_range").as_str() {
                    "partial" => ColorRange::Partial,
                    "full" => ColorRange::Full,
                    _ => ColorRange::Default,
                },
            });
        }
    }
    st.mic_input = st.audio_inputs.iter().find(|i| i.kind == OBS_KIND_MIC).map(|i| i.name.clone());
    st.sample_rate = profile_param(obs, "Audio", "SampleRate").await.and_then(|v| v.parse().ok()).unwrap_or(48_000);
    st.output_color_range = match profile_param(obs, "Video", "ColorRange").await.as_deref() {
        Some("Full") => ColorRange::Full,
        _ => ColorRange::Partial,
    };
    // Stream service → platform.
    if let Ok(svc) = obs.request("GetStreamServiceSettings", Value::Null).await {
        let service = s(&svc["streamServiceSettings"], "service").to_lowercase();
        let server = s(&svc["streamServiceSettings"], "server").to_lowercase();
        st.platform = if service.contains("twitch") || server.contains("twitch") {
            Platform::Twitch
        } else if service.contains("youtube") || server.contains("youtube") {
            Platform::YouTube
        } else if service.contains("kick") || server.contains("kick") {
            Platform::Kick
        } else if service.contains("tiktok") || server.contains("tiktok") {
            Platform::TikTok
        } else if service.contains("facebook") {
            Platform::Facebook
        } else {
            Platform::Other
        };
        st.encoder = None;
        let mode = profile_param(obs, "Output", "Mode").await.unwrap_or_else(|| "Simple".into());
        let video = obs.request("GetVideoSettings", Value::Null).await.unwrap_or(Value::Null);
        let fps = video["fpsNumerator"].as_f64().unwrap_or(30.0) / video["fpsDenominator"].as_f64().unwrap_or(1.0).max(1.0);
        let mut enc = ObsEncoder {
            output_width: video["outputWidth"].as_u64().unwrap_or(0) as u32,
            output_height: video["outputHeight"].as_u64().unwrap_or(0) as u32,
            fps: fps as f32,
            rate_control: "CBR".into(),
            keyframe_sec: 2,
            enhanced_broadcasting: svc["streamServiceSettings"]["multitrack_video_configuration_url"].is_string()
                || svc["streamServiceSettings"]["enhanced_broadcasting"].as_bool() == Some(true),
            ..Default::default()
        };
        if mode == "Advanced" {
            enc.encoder_id = profile_param(obs, "AdvOut", "Encoder").await.unwrap_or_default();
            enc.audio_bitrate_kbps = profile_param(obs, "AdvOut", "Track1Bitrate").await.and_then(|v| v.parse().ok()).unwrap_or(160);
            if let Some(dir) = obs_dir {
                if let Some(e) = files::advanced_stream_encoder(dir) {
                    enc.bitrate_kbps = e["bitrate"].as_u64().unwrap_or(0) as u32;
                    enc.rate_control = e["rate_control"].as_str().unwrap_or("CBR").to_string();
                    enc.keyframe_sec = e["keyint_sec"].as_u64().unwrap_or(0) as u32;
                    enc.preset = e["preset2"].as_str().or(e["preset"].as_str()).unwrap_or_default().to_string();
                }
            }
            st.recording_format = profile_param(obs, "AdvOut", "RecFormat2").await.or(profile_param(obs, "AdvOut", "RecFormat").await).unwrap_or_else(|| "mkv".into());
        } else {
            enc.encoder_id = simple_encoder_id(&profile_param(obs, "SimpleOutput", "StreamEncoder").await.unwrap_or_default()).into();
            enc.bitrate_kbps = profile_param(obs, "SimpleOutput", "VBitrate").await.and_then(|v| v.parse().ok()).unwrap_or(2500);
            enc.audio_bitrate_kbps = profile_param(obs, "SimpleOutput", "ABitrate").await.and_then(|v| v.parse().ok()).unwrap_or(160);
            enc.preset = profile_param(obs, "SimpleOutput", "Preset").await.unwrap_or_default();
            st.recording_format = profile_param(obs, "SimpleOutput", "RecFormat2").await.or(profile_param(obs, "SimpleOutput", "RecFormat").await).unwrap_or_else(|| "mkv".into());
        }
        st.encoder = Some(enc);
    }
    Ok(st)
}

/// Simple-mode encoder names → encoder ids used by the audit.
pub fn simple_encoder_id(simple: &str) -> &'static str {
    match simple.to_ascii_lowercase().as_str() {
        s if s.contains("nvenc") => "obs_nvenc_h264_tex",
        s if s.contains("amd") || s.contains("amf") => "h264_texture_amf",
        s if s.contains("qsv") => "obs_qsv11_v2",
        s if s.contains("x264") || s.is_empty() => "obs_x264",
        _ => "other",
    }
}

/// DirectShow `video_format` enum values used by OBS's dshow plugin.
pub fn dshow_format_name(v: i64) -> &'static str {
    match v {
        0 => "any",
        100..=199 => "rgb",
        200 => "i420",
        201 => "nv12",
        202 => "yv12",
        203 => "y800",
        300 => "yvyu",
        301 => "yuy2",
        302 => "uyvy",
        303 => "hdyc",
        400 => "mjpeg",
        401 => "h264",
        _ => "other",
    }
}

/// GetStats + GetStreamStatus → one health sample.
pub async fn stats_sample(obs: &dyn ObsApi, t_ms: u64) -> Result<StatsSample, String> {
    let st = obs.request("GetStats", Value::Null).await?;
    let ss = obs.request("GetStreamStatus", Value::Null).await.unwrap_or(Value::Null);
    let u = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
    Ok(StatsSample {
        t_ms,
        cpu_percent: st["cpuUsage"].as_f64().unwrap_or(0.0) as f32,
        active_fps: st["activeFps"].as_f64().unwrap_or(0.0) as f32,
        avg_frame_render_ms: st["averageFrameRenderTime"].as_f64().unwrap_or(0.0) as f32,
        render_skipped: u(&st, "renderSkippedFrames"),
        render_total: u(&st, "renderTotalFrames"),
        output_skipped: u(&st, "outputSkippedFrames"),
        output_total: u(&st, "outputTotalFrames"),
        // GetStreamStatus reports the stream output's dropped (network) frames;
        // GetStats' output counters are encoder-skipped frames.
        network_dropped: ss.get("outputSkippedFrames").and_then(Value::as_u64),
        network_total: ss.get("outputTotalFrames").and_then(Value::as_u64),
        congestion: ss["outputCongestion"].as_f64().unwrap_or(0.0) as f32,
        bytes_sent: u(&ss, "outputBytes"),
        reconnecting: ss["outputReconnecting"].as_bool().unwrap_or(false),
    })
}

/// Screenshot of a source as raw image bytes (JPEG).
pub async fn screenshot(obs: &dyn ObsApi, source: &str, width: u32) -> Result<Vec<u8>, String> {
    let r = obs
        .request(
            "GetSourceScreenshot",
            json!({ "sourceName": source, "imageFormat": "jpg", "imageWidth": width, "imageCompressionQuality": 80 }),
        )
        .await?;
    let data = r["imageData"].as_str().ok_or("OBS returned no image.")?;
    let b64 = data.split_once(',').map(|x| x.1).unwrap_or(data);
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).map_err(|e| e.to_string())
}

/// The OBS filter settings for our chain (built-in filters).
pub fn chain_filter_settings(chain: &ProcessingChain) -> Vec<(&'static str, &'static str, Value)> {
    let e = &chain.expander;
    let c = &chain.compressor;
    vec![
        (F_GAIN, "gain_filter", json!({ "db": chain.input_gain_db })),
        (
            F_EXPANDER,
            "expander_filter",
            json!({ "presets": "expander", "ratio": e.ratio, "threshold": e.threshold_db, "attack_time": e.attack_ms.round() as i64, "release_time": e.release_ms.round() as i64, "output_gain": 0.0, "detector": "RMS" }),
        ),
        (
            F_COMPRESSOR,
            "compressor_filter",
            json!({ "ratio": c.ratio, "threshold": c.threshold_db, "attack_time": c.attack_ms.round() as i64, "release_time": c.release_ms.round() as i64, "output_gain": c.makeup_db }),
        ),
        (F_LIMITER, "limiter_filter", json!({ "threshold": chain.limiter.ceiling_db, "release_time": chain.limiter.release_ms.round() as i64 })),
    ]
}

/// Which filter kind can host our VST3, if any (native VST3 support or a
/// VST3 host plugin such as atkAudio).
pub fn vst3_host_kind(kinds: &[String]) -> Option<String> {
    kinds
        .iter()
        .find(|k| k.contains("vst3"))
        .or_else(|| kinds.iter().find(|k| k.contains("atkaudio") || k.contains("atk_audio")))
        .cloned()
}

/// Make the mic input's filter chain match `chain`: our VST3 first (if a host
/// exists), then gain → expander → compressor → limiter; AI suppressors removed.
/// Returns (applied ids, failures).
pub async fn apply_chain(
    obs: &dyn ObsApi,
    input: &str,
    chain: &ProcessingChain,
    vst3_path: Option<&str>,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut ok = Vec::new();
    let mut fail = Vec::new();
    let existing = match filters_of(obs, input).await {
        Ok(f) => f,
        Err(e) => {
            fail.push(("obs_filters".into(), e));
            return (ok, fail);
        }
    };
    // Remove AI noise suppression: it fights a properly set-up mic.
    for f in existing.iter().filter(|f| coach_core::system::obs_audio::is_ai_suppressor(f)) {
        match obs.request("RemoveSourceFilter", json!({ "sourceName": input, "filterName": f.name })).await {
            Ok(_) => ok.push(format!("removed:{}", f.name)),
            Err(e) => fail.push((format!("removed:{}", f.name), e)),
        }
    }
    let mut order: Vec<String> = Vec::new();
    // Our VST3 (HPF, EQ, de-esser, mono).
    let kinds: Vec<String> = obs
        .request("GetSourceFilterKindList", Value::Null)
        .await
        .ok()
        .and_then(|r| r["sourceFilterKinds"].as_array().cloned())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    match (vst3_host_kind(&kinds), vst3_path) {
        (Some(kind), Some(path)) => {
            let settings = json!({ "plugin_path": path });
            let r = upsert(obs, input, &existing, F_VOICE, &kind, settings).await;
            match r {
                Ok(_) => {
                    ok.push("vst3".into());
                    order.push(F_VOICE.into());
                }
                Err(e) => fail.push(("vst3".into(), e)),
            }
        }
        (None, _) => fail.push((
            "vst3".into(),
            "Your OBS can't load our voice plugin yet (it needs VST3 support). Install the free atkAudio plugin for OBS, then press Apply again. Everything else was applied.".into(),
        )),
        (Some(_), None) => fail.push(("vst3".into(), "The Setup Coach voice plugin isn't installed. Re-run the installer.".into())),
    }
    for (name, kind, settings) in chain_filter_settings(chain) {
        match upsert(obs, input, &existing, name, kind, settings).await {
            Ok(_) => {
                ok.push(name.to_string());
                order.push(name.to_string());
            }
            Err(e) => fail.push((name.to_string(), e)),
        }
    }
    // Put our filters first, in order; leave the user's other filters after.
    for (idx, name) in order.iter().enumerate() {
        if let Err(e) = obs.request("SetSourceFilterIndex", json!({ "sourceName": input, "filterName": name, "filterIndex": idx })).await {
            fail.push((format!("order:{name}"), e));
        }
    }
    // Disable user filters that duplicate a stage we now own (old gates/compressors).
    for f in existing.iter().filter(|f| !f.name.starts_with("Setup Coach") && f.enabled) {
        if ["noise_gate_filter", "expander_filter", "compressor_filter", "limiter_filter"].contains(&f.kind.as_str()) {
            if obs.request("SetSourceFilterEnabled", json!({ "sourceName": input, "filterName": f.name, "filterEnabled": false })).await.is_ok() {
                ok.push(format!("disabled:{}", f.name));
            }
        }
    }
    (ok, fail)
}

async fn upsert(obs: &dyn ObsApi, input: &str, existing: &[ObsFilter], name: &str, kind: &str, settings: Value) -> Result<(), String> {
    if existing.iter().any(|f| f.name == name) {
        obs.request("SetSourceFilterSettings", json!({ "sourceName": input, "filterName": name, "filterSettings": settings, "overlay": true })).await?;
        obs.request("SetSourceFilterEnabled", json!({ "sourceName": input, "filterName": name, "filterEnabled": true })).await?;
    } else {
        obs.request("CreateSourceFilter", json!({ "sourceName": input, "filterName": name, "filterKind": kind, "filterSettings": settings })).await?;
    }
    Ok(())
}
