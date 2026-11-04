//! The settings vault: before the app changes anything, what is about to
//! change is saved to `<data>\backups\`, so the user can always go back.
//!
//! * `backups/original/` â€” the first value of every item Setup Coach ever
//!   touched. An item is added the first time it is touched and never
//!   overwritten, so "how it was before Setup Coach" survives many sessions.
//! * `backups/<id>/` â€” one snapshot per change (newest 30 kept).
//!
//! Each holds `snapshot.json` plus `files/` (copies of OBS files). Writes are
//! atomic: files are content-addressed, json goes through temp + rename, and a
//! new snapshot folder is renamed into place only when complete.
//!
//! This module is storage only; `app::vault_ops` reads and restores values.

use crate::goxlr::MicType;
use crate::platform::WinValue;
use crate::wav::write_atomic;
use coach_core::video::CameraState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const ORIGINAL: &str = "original";
pub const KEEP: usize = 30;
pub const ORIGINAL_LABEL: &str = "How it was before Setup Coach";

/// One OBS filter with everything needed to recreate it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterSnap {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub settings: Value,
}

/// One saved value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultItem {
    /// All filters of an OBS source, in order.
    ObsFilters { input: String, filters: Vec<FilterSnap> },
    /// The full settings object of an OBS input (mic device, webcam mode).
    ObsInputSettings { input: String, settings: Value },
    ObsInputMute { input: String, muted: bool },
    ObsMonitoring { input: String, monitor_type: String },
    /// An OBS profile parameter (None = not set).
    ObsProfileParam { category: String, name: String, value: Option<String> },
    ObsVideoSettings { output_width: u64, output_height: u64 },
    /// A copy of an OBS file (`file` = name in `files/`; None = didn't exist).
    ObsFile { path: String, file: Option<String>, sha256: Option<String> },
    /// obs-websocket's on/off switch. Only this flag is saved: the file also
    /// holds the WebSocket password, which never leaves memory.
    ObsWebsocketEnabled { path: String, server_enabled: Option<bool> },
    Windows { value: WinValue },
    /// Windows endpoint level in dB as moved by auto gain.
    EndpointGain { endpoint: String, db: f64 },
    /// Camera control values (UVC) as read before we wrote ours.
    Camera { camera_id: String, camera_name: String, state: Box<CameraState> },
    /// The previous `chain.json` (None = there was none).
    ChainJson { content: Option<String> },
    GoXlr { serial: String, mic_type: MicType, gain_db: f64 },
}

impl VaultItem {
    /// Stable identity: the same setting has the same key across sessions.
    pub fn key(&self) -> String {
        match self {
            VaultItem::ObsFilters { input, .. } => format!("obs_filters:{input}"),
            VaultItem::ObsInputSettings { input, .. } => format!("obs_input:{input}"),
            VaultItem::ObsInputMute { input, .. } => format!("obs_mute:{input}"),
            VaultItem::ObsMonitoring { input, .. } => format!("obs_monitoring:{input}"),
            VaultItem::ObsProfileParam { category, name, .. } => format!("obs_param:{category}/{name}"),
            VaultItem::ObsVideoSettings { .. } => "obs_video".into(),
            VaultItem::ObsFile { path, .. } => format!("obs_file:{path}"),
            VaultItem::ObsWebsocketEnabled { .. } => "obs_websocket".into(),
            VaultItem::Windows { value } => value.key.id(),
            VaultItem::EndpointGain { endpoint, .. } => format!("windows:endpoint_gain_db:{endpoint}"),
            VaultItem::Camera { camera_id, .. } => format!("camera:{camera_id}"),
            VaultItem::ChainJson { .. } => "chain_json".into(),
            VaultItem::GoXlr { serial, .. } => format!("goxlr:{serial}"),
        }
    }

    /// Plain words for the backup list.
    pub fn label(&self) -> String {
        match self {
            VaultItem::ObsFilters { input, .. } => format!("OBS: filters on \"{input}\""),
            VaultItem::ObsInputSettings { input, .. } => format!("OBS: \"{input}\" settings"),
            VaultItem::ObsInputMute { input, .. } => format!("OBS: \"{input}\" mute"),
            VaultItem::ObsMonitoring { input, .. } => format!("OBS: \"{input}\" monitoring"),
            VaultItem::ObsProfileParam { name, .. } => format!("OBS: {} setting", plain_param(name)),
            VaultItem::ObsVideoSettings { .. } => "OBS: stream resolution".into(),
            VaultItem::ObsFile { path, .. } => {
                let f = Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                format!("OBS: settings file {f}")
            }
            VaultItem::ObsWebsocketEnabled { .. } => "OBS: WebSocket server on/off".into(),
            VaultItem::Windows { value } => value.key.label().into(),
            VaultItem::EndpointGain { .. } => "Windows: mic level".into(),
            VaultItem::Camera { camera_name, .. } => format!("Camera: {camera_name} settings"),
            VaultItem::ChainJson { .. } => "Setup Coach voice settings".into(),
            VaultItem::GoXlr { .. } => "GoXLR: mic type and gain".into(),
        }
    }
}

fn plain_param(name: &str) -> &str {
    match name {
        "SampleRate" => "audio sample rate",
        "StreamEncoder" | "Encoder" => "stream encoder",
        "VBitrate" => "stream bitrate",
        "ABitrate" | "Track1Bitrate" => "audio bitrate",
        "RecFormat2" => "recording format",
        other => other,
    }
}

/// A value captured right now, with the file content for `ObsFile`.
#[derive(Debug, Clone)]
pub struct Capture {
    pub item: VaultItem,
    pub content: Option<Vec<u8>>,
}

impl Capture {
    pub fn value(item: VaultItem) -> Self {
        Capture { item, content: None }
    }

    /// Copy of a file about to be edited. Only "not found" means "didn't exist";
    /// any other read error (locked, permission denied, not a file) is an error,
    /// so a later restore never deletes a real file we merely failed to read.
    pub fn file(path: &Path) -> Result<Self, String> {
        let content = match std::fs::read(path) {
            Ok(c) => Some(c),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("We couldn't read {} ({e}).", path.display())),
        };
        let sha = content.as_ref().map(|c| hex(&Sha256::digest(c)));
        let base = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let file = sha.as_ref().map(|s| format!("{}-{}", &s[..12], base));
        Ok(Capture { item: VaultItem::ObsFile { path: path.display().to_string(), file, sha256: sha }, content })
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub created_at: String,
    pub label: String,
    pub items: Vec<VaultItem>,
}

/// One row of `list_backups`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupInfo {
    pub id: String,
    pub created_at: String,
    pub label: String,
    pub items: Vec<String>,
}

pub struct Vault {
    pub dir: PathBuf,
}

fn err(e: impl std::fmt::Display) -> String {
    format!("Couldn't save a backup ({e}).")
}

impl Vault {
    pub fn new(data_dir: &Path) -> Self {
        Vault { dir: data_dir.join("backups") }
    }

    fn snapshot_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    pub fn load(&self, id: &str) -> Result<(Snapshot, PathBuf), String> {
        if id.is_empty() || id.contains(['/', '\\', '.']) {
            return Err("That backup doesn't exist.".into());
        }
        let d = self.snapshot_dir(id);
        let text = std::fs::read_to_string(d.join("snapshot.json")).map_err(|_| "That backup doesn't exist any more.".to_string())?;
        let s = serde_json::from_str(&text).map_err(|_| "That backup is damaged.".to_string())?;
        Ok((s, d))
    }

    /// Per-change snapshot ids, oldest first.
    fn change_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().join("snapshot.json").is_file())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n != ORIGINAL && !n.ends_with(".tmp"))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }

    /// Save `captures` before a change: a new per-change snapshot (unless it
    /// would be identical to the newest one) and any first-time items into
    /// `original`. Returns the snapshot id (None if there was nothing to save).
    pub fn save(&self, label: &str, captures: &[Capture]) -> Result<Option<String>, String> {
        if captures.is_empty() {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.dir).map_err(err)?;
        let items: Vec<VaultItem> = captures.iter().map(|c| c.item.clone()).collect();
        self.merge_original(captures)?;
        let ids = self.change_ids();
        if let Some(last) = ids.last() {
            if let Ok((s, _)) = self.load(last) {
                if s.items == items && s.label == label {
                    return Ok(Some(s.id));
                }
            }
        }
        let now = crate::paths::now_iso();
        let stamp: String = now.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let mut n = 0;
        let id = loop {
            let id = format!("{stamp}-{n:03}");
            if !self.snapshot_dir(&id).exists() && ids.last().is_none_or(|l| id.as_str() > l.as_str()) {
                break id;
            }
            n += 1;
        };
        let tmp = self.dir.join(format!("{id}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        write_files(&tmp.join("files"), captures)?;
        let snap = Snapshot { id: id.clone(), created_at: now, label: label.to_string(), items };
        write_atomic(&tmp.join("snapshot.json"), serde_json::to_string_pretty(&snap).map_err(err)?.as_bytes())?;
        std::fs::rename(&tmp, self.snapshot_dir(&id)).map_err(err)?;
        self.prune();
        Ok(Some(id))
    }

    fn merge_original(&self, captures: &[Capture]) -> Result<(), String> {
        let d = self.snapshot_dir(ORIGINAL);
        let mut orig = match self.load(ORIGINAL) {
            Ok((s, _)) => s,
            Err(_) if !d.join("snapshot.json").exists() => {
                Snapshot { id: ORIGINAL.into(), created_at: crate::paths::now_iso(), label: ORIGINAL_LABEL.into(), items: Vec::new() }
            }
            // Never replace an original we can't read.
            Err(e) => return Err(err(e)),
        };
        let new: Vec<&Capture> = captures.iter().filter(|c| !orig.items.iter().any(|i| i.key() == c.item.key())).collect();
        if new.is_empty() {
            return Ok(());
        }
        let mut seen = std::collections::HashSet::new();
        let new: Vec<Capture> = new.into_iter().filter(|c| seen.insert(c.item.key())).cloned().collect();
        write_files(&d.join("files"), &new)?;
        orig.items.extend(new.into_iter().map(|c| c.item));
        write_atomic(&d.join("snapshot.json"), serde_json::to_string_pretty(&orig).map_err(err)?.as_bytes())
    }

    fn prune(&self) {
        let ids = self.change_ids();
        if ids.len() > KEEP {
            for id in &ids[..ids.len() - KEEP] {
                let _ = std::fs::remove_dir_all(self.snapshot_dir(id));
            }
        }
    }

    /// `original` first, then per-change snapshots newest first.
    pub fn list(&self) -> Vec<BackupInfo> {
        let info = |s: Snapshot| BackupInfo { id: s.id, created_at: s.created_at, label: s.label, items: s.items.iter().map(VaultItem::label).collect() };
        let mut out = Vec::new();
        if let Ok((s, _)) = self.load(ORIGINAL) {
            out.push(info(s));
        }
        for id in self.change_ids().into_iter().rev() {
            if let Ok((s, _)) = self.load(&id) {
                out.push(info(s));
            }
        }
        out
    }

    /// Newest per-change snapshot id.
    pub fn latest_id(&self) -> Option<String> {
        self.change_ids().pop()
    }
}

fn write_files(dir: &Path, captures: &[Capture]) -> Result<(), String> {
    for c in captures {
        if let (VaultItem::ObsFile { file: Some(name), .. }, Some(bytes)) = (&c.item, &c.content) {
            let p = dir.join(name);
            if !p.exists() {
                write_atomic(&p, bytes)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(c: &str) -> Capture {
        Capture::value(VaultItem::ChainJson { content: Some(c.into()) })
    }

    #[test]
    fn original_is_never_overwritten_and_retention_is_30() {
        let d = tempfile::tempdir().unwrap();
        let v = Vault::new(d.path());
        v.save("first", &[chain("a")]).unwrap();
        for i in 0..40 {
            v.save(&format!("change {i}"), &[chain(&format!("v{i}")), Capture::value(VaultItem::ObsInputMute { input: "Mic".into(), muted: i % 2 == 0 })]).unwrap();
        }
        let (o, _) = v.load(ORIGINAL).unwrap();
        assert_eq!(o.items.len(), 2);
        assert_eq!(o.items[0], VaultItem::ChainJson { content: Some("a".into()) });
        assert_eq!(o.items[1], VaultItem::ObsInputMute { input: "Mic".into(), muted: true });
        let list = v.list();
        assert_eq!(list[0].id, ORIGINAL);
        assert_eq!(list.len(), 1 + KEEP);
        assert_eq!(list[1].label, "change 39");
        assert_eq!(list.last().unwrap().label, "change 10");
    }

    #[test]
    fn identical_snapshot_is_not_repeated_and_files_are_copied() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("basic.ini");
        std::fs::write(&f, "[Audio]\nSampleRate=44100\n").unwrap();
        let v = Vault::new(&d.path().join("data"));
        let a = v.save("x", &[Capture::file(&f).unwrap()]).unwrap().unwrap();
        let b = v.save("x", &[Capture::file(&f).unwrap()]).unwrap().unwrap();
        assert_eq!(a, b);
        let (s, dir) = v.load(&a).unwrap();
        let VaultItem::ObsFile { file: Some(name), .. } = &s.items[0] else { panic!() };
        assert_eq!(std::fs::read_to_string(dir.join("files").join(name)).unwrap(), "[Audio]\nSampleRate=44100\n");
        assert!(v.dir.join(ORIGINAL).join("files").join(name).exists());
        assert!(v.load("../x").is_err());
    }

    #[test]
    fn unreadable_file_is_an_error_not_missing() {
        let d = tempfile::tempdir().unwrap();
        // A directory can't be read as a file: stands in for a locked file.
        assert!(Capture::file(d.path()).is_err());
        let missing = Capture::file(&d.path().join("nope.ini")).unwrap();
        assert!(missing.content.is_none());
    }
}
