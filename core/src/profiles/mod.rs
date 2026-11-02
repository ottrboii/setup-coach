//! Device profile database: what we know about each mic, interface, booster,
//! webcam and camera body, plus matching a Windows device to a profile.
//!
//! Data lives in `setup-coach/profiles/*.json` and is embedded into the
//! binary with `include_str!`, so the app ships it. `load_dir` reads the same
//! files from disk (for profile development and hot updates).

mod matcher;
mod schema;

#[cfg(test)]
mod tests;

pub use matcher::*;
pub use schema::*;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

/// The embedded profile files, in load order.
pub const EMBEDDED_FILES: [(&str, &str); 5] = [
    ("generic.json", include_str!("../../../profiles/generic.json")),
    ("mics.json", include_str!("../../../profiles/mics.json")),
    ("interfaces.json", include_str!("../../../profiles/interfaces.json")),
    ("cameras.json", include_str!("../../../profiles/cameras.json")),
    ("camera_bodies.json", include_str!("../../../profiles/camera_bodies.json")),
];

/// One JSON file. Every file has the same shape; empty sections are omitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileFile {
    pub schema_version: u32,
    #[serde(default)]
    pub mics: Vec<MicProfile>,
    #[serde(default)]
    pub interfaces: Vec<InterfaceProfile>,
    #[serde(default)]
    pub boosters: Vec<BoosterProfile>,
    #[serde(default)]
    pub cameras: Vec<CameraProfile>,
    #[serde(default)]
    pub camera_bodies: Vec<CameraBodyProfile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileError {
    Io { file: String, message: String },
    Parse { file: String, message: String },
    SchemaVersion { file: String, found: u32 },
    DuplicateId(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Io { file, message } => write!(f, "{file}: {message}"),
            ProfileError::Parse { file, message } => write!(f, "{file}: invalid JSON: {message}"),
            ProfileError::SchemaVersion { file, found } => write!(
                f,
                "{file}: schema_version {found}, this build understands {SCHEMA_VERSION}"
            ),
            ProfileError::DuplicateId(id) => write!(f, "duplicate profile id '{id}'"),
        }
    }
}

impl std::error::Error for ProfileError {}

/// All profiles, merged from every file.
#[derive(Debug, Clone, Default)]
pub struct ProfileDb {
    pub mics: Vec<MicProfile>,
    pub interfaces: Vec<InterfaceProfile>,
    pub boosters: Vec<BoosterProfile>,
    pub cameras: Vec<CameraProfile>,
    pub camera_bodies: Vec<CameraBodyProfile>,
}

impl ProfileDb {
    /// The profiles compiled into the app.
    pub fn from_embedded() -> Result<Self, ProfileError> {
        Self::from_sources(EMBEDDED_FILES.iter().map(|(n, c)| (n.to_string(), c.to_string())))
    }

    /// Load every `*.json` file in `dir` (sorted by name).
    pub fn load_dir(dir: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let dir = dir.as_ref();
        let io = |message: String| ProfileError::Io { file: dir.display().to_string(), message };
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| io(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        let mut sources = Vec::new();
        for p in paths {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&p)
                .map_err(|e| ProfileError::Io { file: name.clone(), message: e.to_string() })?;
            sources.push((name, text));
        }
        Self::from_sources(sources)
    }

    /// Parse and merge `(file name, JSON text)` pairs. Ids must be unique
    /// across all files and categories.
    pub fn from_sources(
        sources: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ProfileError> {
        let mut db = ProfileDb::default();
        for (name, text) in sources {
            let file: ProfileFile = serde_json::from_str(&text)
                .map_err(|e| ProfileError::Parse { file: name.clone(), message: e.to_string() })?;
            if file.schema_version != SCHEMA_VERSION {
                return Err(ProfileError::SchemaVersion { file: name, found: file.schema_version });
            }
            db.mics.extend(file.mics);
            db.interfaces.extend(file.interfaces);
            db.boosters.extend(file.boosters);
            db.cameras.extend(file.cameras);
            db.camera_bodies.extend(file.camera_bodies);
        }
        db.check_unique_ids()?;
        Ok(db)
    }

    fn check_unique_ids(&self) -> Result<(), ProfileError> {
        let mut seen = std::collections::HashSet::new();
        let ids = self
            .mics
            .iter()
            .map(|p| &p.id)
            .chain(self.interfaces.iter().map(|p| &p.id))
            .chain(self.boosters.iter().map(|p| &p.id))
            .chain(self.cameras.iter().map(|p| &p.id))
            .chain(self.camera_bodies.iter().map(|p| &p.id));
        for id in ids {
            if !seen.insert(id.as_str()) {
                return Err(ProfileError::DuplicateId(id.clone()));
            }
        }
        Ok(())
    }

    pub fn mic(&self, id: &str) -> Option<&MicProfile> {
        self.mics.iter().find(|p| p.id == id)
    }
    pub fn interface(&self, id: &str) -> Option<&InterfaceProfile> {
        self.interfaces.iter().find(|p| p.id == id)
    }
    pub fn booster(&self, id: &str) -> Option<&BoosterProfile> {
        self.boosters.iter().find(|p| p.id == id)
    }
    pub fn camera(&self, id: &str) -> Option<&CameraProfile> {
        self.cameras.iter().find(|p| p.id == id)
    }
    pub fn camera_body(&self, id: &str) -> Option<&CameraBodyProfile> {
        self.camera_bodies.iter().find(|p| p.id == id)
    }

    /// The generic fallback profile for a mic class.
    pub fn generic_mic(&self, class: MicClass) -> Option<&MicProfile> {
        self.mics.iter().find(|p| p.generic_class == Some(class))
    }

    /// The generic fallback profile for a camera class.
    pub fn generic_camera(&self, class: CameraClass) -> Option<&CameraProfile> {
        self.cameras.iter().find(|p| p.generic && p.class == class)
    }
}
