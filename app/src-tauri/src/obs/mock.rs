//! In-memory fake OBS for tests and the non-Windows demo build.

use super::ObsApi;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct FakeInput {
    pub name: String,
    pub kind: String,
    pub settings: Value,
    pub monitor: String,
    pub muted: bool,
    pub volume_db: f64,
}

#[derive(Debug, Clone)]
pub struct FakeFilter {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub settings: Value,
}

#[derive(Debug, Default)]
pub struct FakeObsState {
    pub inputs: Vec<FakeInput>,
    pub filters: HashMap<String, Vec<FakeFilter>>,
    pub filter_kinds: Vec<String>,
    pub scenes: Vec<String>,
    pub params: HashMap<(String, String), String>,
    pub stats: Value,
    pub stream: Value,
    pub log: Vec<String>,
    /// Request types that fail with an error (to test half-finished changes).
    pub fail: Vec<String>,
    /// Output (scaled) resolution, `SetVideoSettings`.
    pub output_size: (u32, u32),
    /// Inputs whose device is released ("Deactivate" pressed in OBS).
    pub inactive: std::collections::HashSet<String>,
    /// Screenshots come back black even for an active source.
    pub black_screenshot: bool,
}

pub struct FakeObs {
    pub state: Mutex<FakeObsState>,
}

impl FakeObs {
    /// A typical OBS: mic with an RNNoise filter and a harsh gate, desktop
    /// audio, a C920 webcam, Twitch in simple mode.
    pub fn typical() -> Self {
        let mut st = FakeObsState::default();
        st.inputs = vec![
            FakeInput { name: "Mic/Aux".into(), kind: "wasapi_input_capture".into(), settings: json!({ "device_id": "default" }), monitor: "OBS_MONITORING_TYPE_MONITOR_AND_OUTPUT".into(), muted: false, volume_db: 0.0 },
            FakeInput { name: "Desktop Audio".into(), kind: "wasapi_output_capture".into(), settings: json!({ "device_id": "default" }), monitor: "OBS_MONITORING_TYPE_NONE".into(), muted: false, volume_db: 0.0 },
            FakeInput { name: "Webcam".into(), kind: "dshow_input".into(), settings: json!({ "video_device_id": "HD Pro Webcam C920:\\\\?\\usb#vid_046d&pid_082d", "res_type": 0 }), monitor: "OBS_MONITORING_TYPE_NONE".into(), muted: false, volume_db: 0.0 },
        ];
        st.filters.insert(
            "Mic/Aux".into(),
            vec![
                FakeFilter { name: "Noise Suppression".into(), kind: "noise_suppress_filter_v2".into(), enabled: true, settings: json!({ "method": "rnnoise" }) },
                FakeFilter { name: "Noise Gate".into(), kind: "noise_gate_filter".into(), enabled: true, settings: json!({ "open_threshold": -26.0, "close_threshold": -32.0 }) },
            ],
        );
        st.scenes = vec!["Main".into()];
        st.output_size = (1920, 1080);
        st.filter_kinds = ["gain_filter", "noise_gate_filter", "noise_suppress_filter_v2", "compressor_filter", "expander_filter", "limiter_filter", "vst_filter", "basic_eq_filter"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (c, n, v) in [
            ("Audio", "SampleRate", "44100"),
            ("Output", "Mode", "Simple"),
            ("SimpleOutput", "StreamEncoder", "x264"),
            ("SimpleOutput", "VBitrate", "8000"),
            ("SimpleOutput", "ABitrate", "160"),
            ("SimpleOutput", "RecFormat2", "mp4"),
        ] {
            st.params.insert((c.into(), n.into()), v.into());
        }
        st.stats = json!({ "cpuUsage": 20.0, "activeFps": 60.0, "averageFrameRenderTime": 3.0, "renderSkippedFrames": 0, "renderTotalFrames": 0, "outputSkippedFrames": 0, "outputTotalFrames": 0 });
        st.stream = json!({ "outputActive": true, "outputReconnecting": false, "outputSkippedFrames": 0, "outputTotalFrames": 0, "outputCongestion": 0.0, "outputBytes": 0 });
        FakeObs { state: Mutex::new(st) }
    }

    pub fn filter_names(&self, source: &str) -> Vec<String> {
        self.state.lock().unwrap().filters.get(source).map(|v| v.iter().map(|f| f.name.clone()).collect()).unwrap_or_default()
    }

    /// Advance the fake stream by `secs` with the given loss fractions.
    pub fn advance(&self, secs: f64, encode_loss: f64, net_loss: f64, kbps: f64) {
        let mut st = self.state.lock().unwrap();
        let frames = (60.0 * secs) as u64;
        let add = |v: &mut Value, k: &str, n: u64| {
            v[k] = json!(v[k].as_u64().unwrap_or(0) + n);
        };
        add(&mut st.stats, "renderTotalFrames", frames);
        add(&mut st.stats, "outputTotalFrames", frames);
        add(&mut st.stats, "outputSkippedFrames", (frames as f64 * encode_loss) as u64);
        add(&mut st.stream, "outputTotalFrames", frames);
        add(&mut st.stream, "outputSkippedFrames", (frames as f64 * net_loss) as u64);
        add(&mut st.stream, "outputBytes", (kbps * 1000.0 / 8.0 * secs) as u64);
        st.stream["outputCongestion"] = json!(if net_loss > 0.0 { 0.6 } else { 0.0 });
    }
}

fn err(m: &str) -> Result<Value, String> {
    Err(m.to_string())
}

#[async_trait::async_trait]
impl ObsApi for FakeObs {
    async fn request(&self, t: &str, d: Value) -> Result<Value, String> {
        let mut st = self.state.lock().unwrap();
        st.log.push(t.to_string());
        if st.fail.iter().any(|f| f == t) {
            return Err(format!("{t} failed (test)"));
        }
        let name = d["inputName"].as_str().or(d["sourceName"].as_str()).unwrap_or_default().to_string();
        let input_idx = st.inputs.iter().position(|i| i.name == name);
        match t {
            "GetVersion" => Ok(json!({ "obsVersion": "32.0.1", "obsWebSocketVersion": "5.6.0" })),
            "GetInputList" => Ok(json!({ "inputs": st.inputs.iter().map(|i| json!({ "inputName": i.name, "inputKind": i.kind, "unversionedInputKind": i.kind })).collect::<Vec<_>>() })),
            "GetInputSettings" => input_idx.map(|i| json!({ "inputSettings": st.inputs[i].settings, "inputKind": st.inputs[i].kind })).ok_or("no input".into()),
            "SetInputSettings" => {
                let i = input_idx.ok_or("no input")?;
                if d["overlay"] == json!(false) {
                    st.inputs[i].settings = d["inputSettings"].clone();
                } else if let (Some(o), Some(n)) = (st.inputs[i].settings.as_object_mut(), d["inputSettings"].as_object()) {
                    for (k, v) in n {
                        o.insert(k.clone(), v.clone());
                    }
                }
                Ok(Value::Null)
            }
            "GetInputAudioMonitorType" => input_idx.map(|i| json!({ "monitorType": st.inputs[i].monitor })).ok_or("no input".into()),
            "SetInputAudioMonitorType" => {
                let i = input_idx.ok_or("no input")?;
                st.inputs[i].monitor = d["monitorType"].as_str().unwrap_or_default().into();
                Ok(Value::Null)
            }
            "GetInputAudioSyncOffset" => {
                input_idx.ok_or("no input")?;
                let ms = st.params.get(&("sync_offset".to_string(), name)).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                Ok(json!({ "inputAudioSyncOffset": ms }))
            }
            "SetInputAudioSyncOffset" => {
                input_idx.ok_or("no input")?;
                let ms = d["inputAudioSyncOffset"].as_i64().ok_or("no offset")?;
                st.params.insert(("sync_offset".to_string(), name), ms.to_string());
                Ok(Value::Null)
            }
            "GetInputVolume" => input_idx.map(|i| json!({ "inputVolumeDb": st.inputs[i].volume_db })).ok_or("no input".into()),
            "GetInputMute" => input_idx.map(|i| json!({ "inputMuted": st.inputs[i].muted })).ok_or("no input".into()),
            "SetInputMute" => {
                let i = input_idx.ok_or("no input")?;
                st.inputs[i].muted = d["inputMuted"].as_bool().unwrap_or(false);
                Ok(Value::Null)
            }
            "GetSourceFilterKindList" => Ok(json!({ "sourceFilterKinds": st.filter_kinds })),
            "GetSourceFilterList" => Ok(json!({ "filters": st.filters.get(&name).cloned().unwrap_or_default().iter().enumerate().map(|(i, f)| json!({ "filterName": f.name, "filterKind": f.kind, "filterIndex": i, "filterEnabled": f.enabled, "filterSettings": f.settings })).collect::<Vec<_>>() })),
            "CreateSourceFilter" => {
                let fl = st.filters.entry(name).or_default();
                let fname = d["filterName"].as_str().unwrap_or_default().to_string();
                if fl.iter().any(|f| f.name == fname) {
                    return err("filter exists");
                }
                fl.push(FakeFilter { name: fname, kind: d["filterKind"].as_str().unwrap_or_default().into(), enabled: true, settings: d["filterSettings"].clone() });
                Ok(Value::Null)
            }
            "SetSourceFilterSettings" | "SetSourceFilterEnabled" | "SetSourceFilterIndex" | "RemoveSourceFilter" => {
                let fl = st.filters.entry(name).or_default();
                let fname = d["filterName"].as_str().unwrap_or_default();
                let pos = fl.iter().position(|f| f.name == fname).ok_or("no filter")?;
                match t {
                    "SetSourceFilterSettings" => fl[pos].settings = d["filterSettings"].clone(),
                    "SetSourceFilterEnabled" => fl[pos].enabled = d["filterEnabled"].as_bool().unwrap_or(true),
                    "SetSourceFilterIndex" => {
                        let f = fl.remove(pos);
                        let idx = (d["filterIndex"].as_u64().unwrap_or(0) as usize).min(fl.len());
                        fl.insert(idx, f);
                    }
                    _ => {
                        fl.remove(pos);
                    }
                }
                Ok(Value::Null)
            }
            "GetProfileParameter" => {
                let k = (d["parameterCategory"].as_str().unwrap_or_default().to_string(), d["parameterName"].as_str().unwrap_or_default().to_string());
                Ok(json!({ "parameterValue": st.params.get(&k), "defaultParameterValue": Value::Null }))
            }
            "SetProfileParameter" => {
                let k = (d["parameterCategory"].as_str().unwrap_or_default().to_string(), d["parameterName"].as_str().unwrap_or_default().to_string());
                match d["parameterValue"].as_str() {
                    Some(v) => st.params.insert(k, v.into()),
                    None => st.params.remove(&k),
                };
                Ok(Value::Null)
            }
            "GetStreamServiceSettings" => Ok(json!({ "streamServiceType": "rtmp_common", "streamServiceSettings": { "service": "Twitch", "server": "auto" } })),
            "GetVideoSettings" => Ok(json!({ "fpsNumerator": 60, "fpsDenominator": 1, "baseWidth": 1920, "baseHeight": 1080, "outputWidth": st.output_size.0, "outputHeight": st.output_size.1 })),
            "SetVideoSettings" => {
                let w = d["outputWidth"].as_u64().unwrap_or(st.output_size.0 as u64) as u32;
                let h = d["outputHeight"].as_u64().unwrap_or(st.output_size.1 as u64) as u32;
                st.output_size = (w, h);
                Ok(Value::Null)
            }
            "GetSceneList" => Ok(json!({ "currentProgramSceneName": st.scenes.first(), "scenes": st.scenes.iter().map(|s| json!({ "sceneName": s })).collect::<Vec<_>>() })),
            "CreateScene" => {
                let s = d["sceneName"].as_str().unwrap_or_default().to_string();
                if st.scenes.contains(&s) || st.inputs.iter().any(|i| i.name == s) {
                    return err("a source with that name already exists");
                }
                st.scenes.push(s);
                Ok(json!({ "sceneUuid": "fake" }))
            }
            "RemoveScene" => {
                let s = d["sceneName"].as_str().unwrap_or_default();
                let pos = st.scenes.iter().position(|x| x == s).ok_or("no scene")?;
                st.scenes.remove(pos);
                Ok(Value::Null)
            }
            "CreateInput" => {
                let scene = d["sceneName"].as_str().unwrap_or_default().to_string();
                if !st.scenes.contains(&scene) {
                    return err("no scene");
                }
                if input_idx.is_some() || st.scenes.contains(&name) {
                    return err("a source with that name already exists");
                }
                st.inputs.push(FakeInput {
                    name,
                    kind: d["inputKind"].as_str().unwrap_or_default().into(),
                    settings: d["inputSettings"].clone(),
                    monitor: "OBS_MONITORING_TYPE_NONE".into(),
                    muted: false,
                    volume_db: 0.0,
                });
                Ok(json!({ "inputUuid": "fake", "sceneItemId": 1 }))
            }
            "RemoveInput" => {
                let i = input_idx.ok_or("no input")?;
                st.inputs.remove(i);
                st.filters.remove(&name);
                Ok(Value::Null)
            }
            "GetStats" => Ok(st.stats.clone()),
            "GetStreamStatus" => Ok(st.stream.clone()),
            "GetSourceActive" => {
                input_idx.ok_or("no input")?;
                Ok(json!({ "videoActive": !st.inactive.contains(&name), "videoShowing": true }))
            }
            "PressInputPropertiesButton" => {
                input_idx.ok_or("no input")?;
                if d["propertyName"].as_str() != Some("activate") {
                    return Err("no such button".into());
                }
                if !st.inactive.remove(&name) {
                    st.inactive.insert(name);
                }
                Ok(Value::Null)
            }
            "GetSourceScreenshot" => {
                let f = if st.black_screenshot || st.inactive.contains(&name) {
                    coach_core::video::Frame::solid(320, 240, [0, 0, 0], 0.0)
                } else {
                    crate::platform::mock::synthetic_frame(0.9, 0.0)
                };
                let jpeg = crate::camera::encode_jpeg(&f, 80).map_err(|e| e.to_string())?;
                Ok(json!({ "imageData": format!("data:image/jpg;base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, jpeg)) }))
            }
            other => Err(format!("fake OBS doesn't know {other}")),
        }
    }
}
