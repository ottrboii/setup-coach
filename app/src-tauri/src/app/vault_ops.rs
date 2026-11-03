//! Reading what is about to change (for the vault) and putting it back.

use super::{AppCore, R};
use crate::goxlr;
use crate::obs::{self, files};
use crate::platform::WinKey;
use crate::types::ApplyResult;
use crate::vault::{BackupInfo, Capture, FilterSnap, Vault, VaultItem, ORIGINAL};
use coach_core::system::Action;
use coach_core::video::UvcSettings;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Something about to change, before we know its value.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    ObsFilters(String),
    ObsInputSettings(String),
    ObsInputMute(String),
    ObsMonitoring(String),
    ObsParam(String, String),
    ObsVideo,
    ObsFile(PathBuf),
    ObsWebsocket,
    Windows(WinKey),
    EndpointGain(String),
    Camera(String),
    ChainJson,
    GoXlr,
}

pub(crate) const CLOSE_OBS: &str = "Close OBS, then press Restore again — OBS only allows this change while it's closed.";

impl AppCore {
    pub fn vault(&self) -> Vault {
        Vault::new(&self.data_dir)
    }

    fn websocket_config_path(&self) -> Option<PathBuf> {
        Some(self.obs_dir()?.join("plugin_config").join("obs-websocket").join("config.json"))
    }

    /// What a core `Action` is about to change (mirrors `execute`).
    pub async fn targets_for_action(&self, a: &Action) -> Vec<Target> {
        let obs_connected = self.obs().await.is_some();
        let profile_files = self.obs_dir().and_then(|d| files::current_profile_and_collection(&d));
        let p = |c: &str, n: &str| Target::ObsParam(c.into(), n.into());
        match a {
            Action::SetObsInputMono { .. } => profile_files.map(|(_, scenes)| vec![Target::ObsFile(scenes)]).unwrap_or_default(),
            Action::SetObsSampleRate { .. } if obs_connected => vec![p("Audio", "SampleRate")],
            Action::SetObsSampleRate { .. } => profile_files.map(|(prof, _)| vec![Target::ObsFile(prof.join("basic.ini"))]).unwrap_or_default(),
            Action::EnableObsWebsocket => vec![Target::ObsWebsocket],
            Action::SetObsInputMuted { input, .. } => vec![Target::ObsInputMute(input.clone())],
            Action::RemoveObsFilter { input, .. }
            | Action::ReorderObsFilters { input, .. }
            | Action::SetObsFilterSettings { input, .. }
            | Action::AddObsFilter { input, .. } => vec![Target::ObsFilters(input.clone())],
            Action::SetObsMonitoring { input, .. } => vec![Target::ObsMonitoring(input.clone())],
            Action::PinObsMicDevice { input, .. } | Action::SetObsVideoCaptureMode { input, .. } => vec![Target::ObsInputSettings(input.clone())],
            Action::SetObsStreamEncoder { .. } => vec![p("SimpleOutput", "StreamEncoder"), p("AdvOut", "Encoder")],
            Action::SetObsStreamBitrate { .. } => vec![p("SimpleOutput", "VBitrate")],
            Action::SetObsAudioBitrate { .. } => vec![p("SimpleOutput", "ABitrate"), p("AdvOut", "Track1Bitrate")],
            Action::SetObsRecordingFormat { .. } => vec![p("SimpleOutput", "RecFormat2"), p("AdvOut", "RecFormat2")],
            Action::SetObsOutputResolution { .. } => vec![Target::ObsVideo],
            Action::SetObsRateControl { .. } | Action::SetObsKeyframeInterval { .. } => Vec::new(),
            other => WinKey::for_action(other).map(|k| vec![Target::Windows(k)]).unwrap_or_default(),
        }
    }

    /// Read the current value of a target (None = nothing to save, e.g. the
    /// source doesn't exist or the value can't be read).
    pub async fn capture(&self, t: &Target) -> Option<Capture> {
        let obs = || async { self.obs().await };
        let item = match t {
            Target::ObsFilters(input) => {
                let o = obs().await?;
                let filters = obs::filters_of(o.as_ref(), input).await.ok()?;
                VaultItem::ObsFilters {
                    input: input.clone(),
                    filters: filters.into_iter().map(|f| FilterSnap { name: f.name, kind: f.kind, enabled: f.enabled, settings: f.settings }).collect(),
                }
            }
            Target::ObsInputSettings(input) => {
                let r = obs().await?.request("GetInputSettings", json!({ "inputName": input })).await.ok()?;
                VaultItem::ObsInputSettings { input: input.clone(), settings: r["inputSettings"].clone() }
            }
            Target::ObsInputMute(input) => {
                let r = obs().await?.request("GetInputMute", json!({ "inputName": input })).await.ok()?;
                VaultItem::ObsInputMute { input: input.clone(), muted: r["inputMuted"].as_bool()? }
            }
            Target::ObsMonitoring(input) => {
                let r = obs().await?.request("GetInputAudioMonitorType", json!({ "inputName": input })).await.ok()?;
                VaultItem::ObsMonitoring { input: input.clone(), monitor_type: r["monitorType"].as_str()?.to_string() }
            }
            Target::ObsParam(c, n) => {
                let r = obs().await?.request("GetProfileParameter", json!({ "parameterCategory": c, "parameterName": n })).await.ok()?;
                VaultItem::ObsProfileParam { category: c.clone(), name: n.clone(), value: r["parameterValue"].as_str().map(str::to_string) }
            }
            Target::ObsVideo => {
                let r = obs().await?.request("GetVideoSettings", Value::Null).await.ok()?;
                VaultItem::ObsVideoSettings { output_width: r["outputWidth"].as_u64()?, output_height: r["outputHeight"].as_u64()? }
            }
            Target::ObsFile(p) => return Some(Capture::file(p)),
            Target::ObsWebsocket => {
                let p = self.websocket_config_path()?;
                let enabled = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok()).and_then(|v| v["server_enabled"].as_bool());
                VaultItem::ObsWebsocketEnabled { path: p.display().to_string(), server_enabled: enabled }
            }
            Target::Windows(k) => VaultItem::Windows { value: self.platform.system.read_setting(k)? },
            Target::EndpointGain(ep) => VaultItem::EndpointGain { endpoint: ep.clone(), db: self.platform.system.endpoint_gain_db(ep)?.current_db },
            Target::Camera(id) => {
                let name = self.platform.camera.list().into_iter().find(|c| &c.id == id)?.name;
                VaultItem::Camera { camera_id: id.clone(), camera_name: name, state: Box::new(self.platform.camera.read_state(id).ok()?) }
            }
            Target::ChainJson => VaultItem::ChainJson { content: std::fs::read_to_string(self.data_dir.join("chain.json")).ok() },
            Target::GoXlr => {
                let (_, d) = self.goxlr_device().await?;
                VaultItem::GoXlr { serial: d.serial, mic_type: d.mic_type, gain_db: d.mic_gain_db }
            }
        };
        Some(Capture::value(item))
    }

    /// Save what `targets` hold right now, before changing them. Fails (and
    /// the caller must not write) when the backup can't be stored.
    pub async fn backup_before(&self, label: &str, targets: &[Target]) -> R<Option<String>> {
        let mut caps = Vec::new();
        for t in targets {
            if let Some(c) = self.capture(t).await {
                if !caps.iter().any(|x: &Capture| x.item.key() == c.item.key()) {
                    caps.push(c);
                }
            }
        }
        self.vault()
            .save(label, &caps)
            .map_err(|e| format!("We couldn't save a backup of your current settings, so nothing was changed. {e}"))
    }

    pub fn list_backups(&self) -> Vec<BackupInfo> {
        self.vault().list()
    }

    /// Put a backup back. `"original"` = everything as it was before Setup
    /// Coach. Items whose device/source is gone are skipped with a note.
    pub async fn restore_backup(&self, id: &str) -> ApplyResult {
        let mut res = ApplyResult::default();
        let (snap, dir) = match self.vault().load(id) {
            Ok(x) => x,
            Err(e) => {
                res.fail(id, e);
                return res;
            }
        };
        // Restoring is a change too: save the current values first.
        let targets: Vec<Target> = snap.items.iter().map(target_of).collect();
        let what = if id == ORIGINAL { "how it was before Setup Coach".to_string() } else { format!("\"{}\"", snap.label) };
        if let Err(e) = self.backup_before(&format!("Before going back to {what}"), &targets).await {
            res.fail("backup", e);
            return res;
        }
        // Newest first, so for `original` the earliest saved value wins.
        for item in snap.items.iter().rev() {
            let key = item.key();
            match self.restore_item(item, &dir).await {
                Ok(Restored::Done) => res.ok(key),
                Ok(Restored::Skipped(note)) => res.skip(key, note),
                Err(e) => res.fail(key, e),
            }
        }
        if snap.items.iter().any(|i| matches!(i, VaultItem::ChainJson { .. })) {
            let mut inner = self.inner.lock().unwrap();
            inner.applied_chain = None;
        }
        res.restart_obs_needed = res.applied.iter().any(|k| k.starts_with("obs_file:") || k == "obs_websocket");
        res
    }

    async fn restore_item(&self, item: &VaultItem, dir: &std::path::Path) -> R<Restored> {
        use Restored::*;
        let obs_closed_note = || Skipped("OBS is closed, so your OBS settings weren't touched. Open OBS and press Restore again.".into());
        let gone_input = |i: &str| Skipped(format!("OBS doesn't have \"{i}\" any more, so it was left alone."));
        let obs = self.obs().await;
        let has_input = |input: String| {
            let o = obs.clone();
            async move {
                let Some(o) = o else { return false };
                // Inputs and scenes both carry filters; GetSourceFilterList answers for either.
                o.request("GetSourceFilterList", json!({ "sourceName": input })).await.is_ok()
            }
        };
        match item {
            VaultItem::ObsFilters { input, filters } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                if !has_input(input.clone()).await {
                    return Ok(gone_input(input));
                }
                let now = obs::filters_of(o.as_ref(), input).await?;
                for f in now.iter().filter(|f| !filters.iter().any(|s| s.name == f.name)) {
                    o.request("RemoveSourceFilter", json!({ "sourceName": input, "filterName": f.name })).await?;
                }
                for f in filters {
                    if now.iter().any(|n| n.name == f.name) {
                        o.request("SetSourceFilterSettings", json!({ "sourceName": input, "filterName": f.name, "filterSettings": f.settings, "overlay": false })).await?;
                    } else {
                        o.request("CreateSourceFilter", json!({ "sourceName": input, "filterName": f.name, "filterKind": f.kind, "filterSettings": f.settings })).await?;
                    }
                    o.request("SetSourceFilterEnabled", json!({ "sourceName": input, "filterName": f.name, "filterEnabled": f.enabled })).await?;
                }
                for (i, f) in filters.iter().enumerate() {
                    o.request("SetSourceFilterIndex", json!({ "sourceName": input, "filterName": f.name, "filterIndex": i })).await?;
                }
                Ok(Done)
            }
            VaultItem::ObsInputSettings { input, settings } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                if o.request("GetInputSettings", json!({ "inputName": input })).await.is_err() {
                    return Ok(gone_input(input));
                }
                o.request("SetInputSettings", json!({ "inputName": input, "inputSettings": settings, "overlay": false })).await.map(|_| Done)
            }
            VaultItem::ObsInputMute { input, muted } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                match o.request("SetInputMute", json!({ "inputName": input, "inputMuted": muted })).await {
                    Ok(_) => Ok(Done),
                    Err(_) => Ok(gone_input(input)),
                }
            }
            VaultItem::ObsMonitoring { input, monitor_type } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                match o.request("SetInputAudioMonitorType", json!({ "inputName": input, "monitorType": monitor_type })).await {
                    Ok(_) => Ok(Done),
                    Err(_) => Ok(gone_input(input)),
                }
            }
            VaultItem::ObsProfileParam { category, name, value } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                // A null value removes the parameter (obs-websocket SetProfileParameter).
                o.request("SetProfileParameter", json!({ "parameterCategory": category, "parameterName": name, "parameterValue": value })).await.map(|_| Done)
            }
            VaultItem::ObsVideoSettings { output_width, output_height } => {
                let Some(o) = obs.as_ref() else { return Ok(obs_closed_note()) };
                o.request("SetVideoSettings", json!({ "outputWidth": output_width, "outputHeight": output_height })).await.map(|_| Done)
            }
            VaultItem::ObsFile { path, file, .. } => {
                if self.platform.system.obs_running() {
                    return Err(CLOSE_OBS.into());
                }
                let p = PathBuf::from(path);
                match file {
                    Some(name) => {
                        let bytes = std::fs::read(dir.join("files").join(name)).map_err(|_| "The saved copy of this file is missing.".to_string())?;
                        crate::wav::write_atomic(&p, &bytes).map(|_| Done)
                    }
                    None => {
                        let _ = std::fs::remove_file(&p);
                        Ok(Done)
                    }
                }
            }
            VaultItem::ObsWebsocketEnabled { path, server_enabled } => {
                if self.platform.system.obs_running() {
                    return Err(CLOSE_OBS.into());
                }
                let p = PathBuf::from(path);
                let Ok(text) = std::fs::read_to_string(&p) else { return Ok(Skipped("OBS's WebSocket settings file is gone, so it was left alone.".into())) };
                let mut v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
                v["server_enabled"] = Value::Bool(server_enabled.unwrap_or(false));
                crate::wav::write_atomic(&p, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?.as_bytes()).map(|_| Done)
            }
            VaultItem::Windows { value } => {
                let endpoint = match &value.key {
                    WinKey::Ducking | WinKey::UsbSelectiveSuspend => None,
                    WinKey::EndpointVolume { endpoint }
                    | WinKey::Enhancements { endpoint }
                    | WinKey::VoiceClarity { endpoint }
                    | WinKey::Listen { endpoint }
                    | WinKey::ExclusiveMode { endpoint }
                    | WinKey::MicBoost { endpoint } => Some(endpoint),
                };
                if let Some(ep) = endpoint {
                    if !self.platform.audio.list_inputs().iter().any(|d| &d.id == ep) {
                        return Ok(Skipped("That microphone isn't connected, so its Windows settings were left alone. Plug it in and press Restore again.".into()));
                    }
                }
                self.platform.system.restore_setting(value).map(|_| Done)
            }
            VaultItem::EndpointGain { endpoint, db } => {
                if !self.platform.audio.list_inputs().iter().any(|d| &d.id == endpoint) {
                    return Ok(Skipped("That microphone isn't connected, so its level was left alone. Plug it in and press Restore again.".into()));
                }
                self.platform.system.set_endpoint_gain_db(endpoint, *db).map(|_| Done)
            }
            VaultItem::Camera { camera_id, camera_name, state } => {
                if !self.platform.camera.list().iter().any(|c| &c.id == camera_id) {
                    return Ok(Skipped(format!("{camera_name} isn't connected, so its settings were left alone. Plug it in and press Restore again.")));
                }
                let uvc = UvcSettings {
                    auto_exposure: state.auto_exposure,
                    ae_priority: state.ae_priority,
                    exposure_log2: state.exposure_log2,
                    target_shutter_s: None,
                    shutter_exact: true,
                    gain: state.gain,
                    auto_wb: state.auto_wb,
                    wb_kelvin: state.wb_kelvin,
                    auto_focus: state.auto_focus,
                    focus: state.focus,
                    sharpness: state.sharpness,
                    saturation: state.saturation,
                    backlight_comp: state.backlight_comp,
                    powerline_hz: state.powerline_hz,
                    hdr: state.hdr,
                };
                let cam = self.platform.camera.clone();
                let id = camera_id.clone();
                let warnings = tokio::task::spawn_blocking(move || cam.apply(&id, &uvc)).await.map_err(|e| e.to_string())??;
                if warnings.is_empty() { Ok(Done) } else { Err(warnings.join(" ")) }
            }
            VaultItem::ChainJson { content } => {
                let p = self.data_dir.join("chain.json");
                match content {
                    Some(c) => crate::wav::write_atomic(&p, c.as_bytes()).map(|_| Done),
                    None => {
                        let _ = std::fs::remove_file(&p);
                        Ok(Done)
                    }
                }
            }
            VaultItem::GoXlr { serial, mic_type, gain_db } => {
                let Some(api) = self.goxlr().await else {
                    return Ok(Skipped("GoXLR Utility isn't running, so the GoXLR was left alone. Start it and press Restore again.".into()));
                };
                if !api.devices().await.map(|d| d.iter().any(|x| &x.serial == serial)).unwrap_or(false) {
                    return Ok(Skipped("That GoXLR isn't connected, so it was left alone.".into()));
                }
                api.command(serial, goxlr::cmd_set_mic_type(*mic_type)).await?;
                api.command(serial, goxlr::cmd_set_mic_gain(*mic_type, *gain_db)).await.map(|_| Done)
            }
        }
    }
}

enum Restored {
    Done,
    Skipped(String),
}

/// The target a saved item came from (to back it up again before restoring).
pub fn target_of(item: &VaultItem) -> Target {
    match item {
        VaultItem::ObsFilters { input, .. } => Target::ObsFilters(input.clone()),
        VaultItem::ObsInputSettings { input, .. } => Target::ObsInputSettings(input.clone()),
        VaultItem::ObsInputMute { input, .. } => Target::ObsInputMute(input.clone()),
        VaultItem::ObsMonitoring { input, .. } => Target::ObsMonitoring(input.clone()),
        VaultItem::ObsProfileParam { category, name, .. } => Target::ObsParam(category.clone(), name.clone()),
        VaultItem::ObsVideoSettings { .. } => Target::ObsVideo,
        VaultItem::ObsFile { path, .. } => Target::ObsFile(PathBuf::from(path)),
        VaultItem::ObsWebsocketEnabled { .. } => Target::ObsWebsocket,
        VaultItem::Windows { value } => Target::Windows(value.key.clone()),
        VaultItem::EndpointGain { endpoint, .. } => Target::EndpointGain(endpoint.clone()),
        VaultItem::Camera { camera_id, .. } => Target::Camera(camera_id.clone()),
        VaultItem::ChainJson { .. } => Target::ChainJson,
        VaultItem::GoXlr { .. } => Target::GoXlr,
    }
}
