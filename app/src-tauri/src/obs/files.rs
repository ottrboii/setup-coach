//! Offline edits to OBS's own config files, for settings obs-websocket can't
//! change (mono downmix flag, audio sample rate). Only used while OBS is
//! closed, otherwise OBS overwrites them on exit.

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `OBS_SOURCE_FLAG_FORCE_MONO`.
pub const FLAG_FORCE_MONO: u64 = 1 << 1;

/// Minimal INI reader: (section, key) → value.
pub fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut cur = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            cur = l[1..l.len() - 1].to_string();
        } else if cur.eq_ignore_ascii_case(section) {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// Set (or add) a key in a section, preserving everything else.
pub fn ini_set(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut done = false;
    let mut section_seen = false;
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            if cur.eq_ignore_ascii_case(section) && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            cur = l[1..l.len() - 1].to_string();
            if cur.eq_ignore_ascii_case(section) {
                section_seen = true;
            }
            out.push(line.to_string());
            continue;
        }
        if cur.eq_ignore_ascii_case(section) && !done {
            if let Some((k, _)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    out.push(format!("{key}={value}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
        let _ = i;
    }
    if !done {
        if !section_seen {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key}={value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn read(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    // OBS writes UTF-8 with an optional BOM.
    let t = String::from_utf8_lossy(&bytes).to_string();
    Some(t.trim_start_matches('\u{feff}').to_string())
}

/// (profile dir, scene collection file) from user.ini (OBS 31+) or global.ini.
pub fn current_profile_and_collection(obs_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let ini = read(&obs_dir.join("user.ini")).filter(|t| ini_get(t, "Basic", "ProfileDir").is_some()).or_else(|| read(&obs_dir.join("global.ini")))?;
    let profile = ini_get(&ini, "Basic", "ProfileDir").or_else(|| ini_get(&ini, "Basic", "Profile"))?;
    let collection = ini_get(&ini, "Basic", "SceneCollectionFile").or_else(|| ini_get(&ini, "Basic", "SceneCollection"))?;
    Some((
        obs_dir.join("basic").join("profiles").join(profile),
        obs_dir.join("basic").join("scenes").join(format!("{collection}.json")),
    ))
}

/// Source name → OBS source flags from the current scene collection.
pub fn source_flags(obs_dir: &Path) -> Option<HashMap<String, u64>> {
    let (_, scenes) = current_profile_and_collection(obs_dir)?;
    let v: Value = serde_json::from_str(&read(&scenes)?).ok()?;
    Some(
        v["sources"]
            .as_array()?
            .iter()
            .filter_map(|s| Some((s["name"].as_str()?.to_string(), s["flags"].as_u64().unwrap_or(0))))
            .collect(),
    )
}

/// Advanced-mode stream encoder settings (`streamEncoder.json`).
pub fn advanced_stream_encoder(obs_dir: &Path) -> Option<Value> {
    let (profile, _) = current_profile_and_collection(obs_dir)?;
    serde_json::from_str(&read(&profile.join("streamEncoder.json"))?).ok()
}

/// Turn on "Downmix to Mono" for a source. OBS must be closed.
pub fn set_force_mono(obs_dir: &Path, source: &str, on: bool) -> Result<(), String> {
    let (_, scenes) = current_profile_and_collection(obs_dir).ok_or("Couldn't find OBS's scene collection.")?;
    let text = read(&scenes).ok_or("Couldn't read OBS's scene collection.")?;
    let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let src = v["sources"]
        .as_array_mut()
        .and_then(|a| a.iter_mut().find(|s| s["name"].as_str() == Some(source)))
        .ok_or_else(|| format!("Couldn't find \"{source}\" in OBS."))?;
    let flags = src["flags"].as_u64().unwrap_or(0);
    src["flags"] = Value::from(if on { flags | FLAG_FORCE_MONO } else { flags & !FLAG_FORCE_MONO });
    backup(&scenes)?;
    std::fs::write(&scenes, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Set the profile's audio sample rate. OBS must be closed.
pub fn set_sample_rate(obs_dir: &Path, hz: u32) -> Result<(), String> {
    let (profile, _) = current_profile_and_collection(obs_dir).ok_or("Couldn't find OBS's profile.")?;
    let p = profile.join("basic.ini");
    let text = read(&p).unwrap_or_default();
    backup(&p)?;
    std::fs::write(&p, ini_set(&text, "Audio", "SampleRate", &hz.to_string())).map_err(|e| e.to_string())
}

/// Turn the WebSocket server on in its config. OBS must be closed.
pub fn enable_websocket(obs_dir: &Path) -> Result<(), String> {
    let p = obs_dir.join("plugin_config").join("obs-websocket").join("config.json");
    let mut v: Value = read(&p).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| serde_json::json!({}));
    v["server_enabled"] = Value::Bool(true);
    if v.get("server_port").is_none() {
        v["server_port"] = Value::from(4455);
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn backup(p: &Path) -> Result<(), String> {
    if p.exists() {
        let b = p.with_extension(format!("{}.setupcoach-backup", p.extension().and_then(|e| e.to_str()).unwrap_or("bak")));
        if !b.exists() {
            std::fs::copy(p, b).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_obs() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let o = d.path();
        std::fs::write(o.join("global.ini"), "\u{feff}[General]\nX=1\n[Basic]\nProfile=Main\nProfileDir=Main\nSceneCollection=Stream\nSceneCollectionFile=Stream\n").unwrap();
        std::fs::create_dir_all(o.join("basic/profiles/Main")).unwrap();
        std::fs::create_dir_all(o.join("basic/scenes")).unwrap();
        std::fs::write(o.join("basic/profiles/Main/basic.ini"), "[Video]\nBaseCX=1920\n[Audio]\nSampleRate=44100\nChannelSetup=Stereo\n").unwrap();
        std::fs::write(o.join("basic/profiles/Main/streamEncoder.json"), r#"{"bitrate":6000,"rate_control":"CBR","keyint_sec":2}"#).unwrap();
        std::fs::write(o.join("basic/scenes/Stream.json"), r#"{"sources":[{"name":"Mic/Aux","flags":0},{"name":"Desktop Audio","flags":0}]}"#).unwrap();
        d
    }

    #[test]
    fn ini_roundtrip() {
        let t = "[A]\nx=1\n[B]\ny=2\n";
        assert_eq!(ini_get(t, "B", "y").as_deref(), Some("2"));
        let t2 = ini_set(t, "A", "x", "5");
        assert_eq!(ini_get(&t2, "A", "x").as_deref(), Some("5"));
        let t3 = ini_set(&t2, "C", "z", "9");
        assert_eq!(ini_get(&t3, "C", "z").as_deref(), Some("9"));
        let t4 = ini_set(&t3, "B", "new", "1");
        assert_eq!(ini_get(&t4, "B", "new").as_deref(), Some("1"));
        assert_eq!(ini_get(&t4, "B", "y").as_deref(), Some("2"));
    }

    #[test]
    fn mono_and_sample_rate_edits() {
        let d = fake_obs();
        set_force_mono(d.path(), "Mic/Aux", true).unwrap();
        assert_eq!(source_flags(d.path()).unwrap()["Mic/Aux"] & FLAG_FORCE_MONO, FLAG_FORCE_MONO);
        set_sample_rate(d.path(), 48000).unwrap();
        let ini = std::fs::read_to_string(d.path().join("basic/profiles/Main/basic.ini")).unwrap();
        assert_eq!(ini_get(&ini, "Audio", "SampleRate").as_deref(), Some("48000"));
        assert_eq!(ini_get(&ini, "Audio", "ChannelSetup").as_deref(), Some("Stereo"));
        assert!(d.path().join("basic/profiles/Main/basic.ini.setupcoach-backup").exists());
        assert_eq!(advanced_stream_encoder(d.path()).unwrap()["bitrate"], 6000);
        assert!(set_force_mono(d.path(), "Nope", true).is_err());
    }

    #[test]
    fn websocket_enable() {
        let d = fake_obs();
        enable_websocket(d.path()).unwrap();
        let c = super::super::read_websocket_config(d.path()).unwrap();
        assert!(c.server_enabled);
        assert_eq!(c.server_port, 4455);
    }
}
