//! "What mic is plugged into this?": the user names the physical mic behind a
//! Windows input (a GoXLR or audio interface hides it). The answer is kept per
//! device in `setup.json` (`mic_behind`), written through the settings vault,
//! and then drives the profile used for thresholds, questions and advice.

use super::*;
use crate::platform::AudioDevice;
use coach_core::profiles::{Confidence, MicClass, MicConnection, MicProfile};

impl AppCore {
    /// Every specific mic we know, for the picker (generic fallbacks left out).
    pub fn list_mic_profiles(&self) -> Vec<MicProfileOption> {
        let mut v: Vec<MicProfileOption> = self
            .profiles
            .mics
            .iter()
            .filter(|m| m.generic_class.is_none())
            .map(|m| MicProfileOption {
                id: m.id.clone(),
                brand: m.brand.clone(),
                model: m.model.clone(),
                display_name: mic_label(m),
                family: m.family.clone(),
                connection: match &m.connection {
                    MicConnection::Usb => "usb",
                    MicConnection::Xlr => "xlr",
                    MicConnection::UsbXlrHybrid => "usb_xlr",
                    MicConnection::Headset { .. } => "headset",
                    MicConnection::Wireless => "wireless",
                    MicConnection::Builtin => "builtin",
                }
                .into(),
                capsule: format!("{:?}", m.capsule).to_ascii_lowercase(),
                verified: m.verified,
                aliases: m.aliases.clone(),
            })
            .collect();
        v.sort_by_cached_key(|a| (a.brand.to_lowercase(), a.model.to_lowercase()));
        v
    }

    /// The saved answer for one device, if any.
    pub fn mic_choice(&self, device_id: &str) -> Option<MicBehind> {
        self.load_setup()?.mic_behind.get(device_id).cloned()
    }

    /// The mic profile the user named for this device (not `other`/`skipped`).
    pub(super) fn chosen_profile(&self, device_id: &str) -> Option<&MicProfile> {
        let c = self.mic_choice(device_id)?;
        if c.status != MicBehindStatus::Chosen {
            return None;
        }
        self.profiles.mic(c.profile_id.as_deref()?)
    }

    /// The profile to coach with: the user's choice, else the name match.
    pub(super) fn effective_mic(&self, dev: &AudioDevice) -> Option<&MicProfile> {
        self.chosen_profile(&dev.id).or_else(|| self.profiles.match_input(&Self::identity(&dev.name, dev.usb)).map(|m| m.mic.profile))
    }

    /// A mic profile by id, or by the "Brand Model" label a question offered.
    pub(super) fn find_mic(&self, id_or_label: &str) -> Option<&MicProfile> {
        if let Some(p) = self.profiles.mic(id_or_label) {
            return Some(p);
        }
        let want = coach_core::profiles::normalize(id_or_label);
        self.profiles.mics.iter().filter(|m| m.generic_class.is_none()).find(|m| coach_core::profiles::normalize(&mic_label(m)) == want)
    }

    /// True for devices that hide the real mic or that we can't name: an
    /// interface/mixer input, an unknown USB mic, a wireless receiver. Not for
    /// headsets, built-in and Bluetooth mics (the name already says it).
    pub(super) fn asks_for_mic(&self, name: &str, usb: Option<(u16, u16)>) -> bool {
        let Some(m) = self.profiles.match_input(&Self::identity(name, usb)) else { return false };
        matches!(
            m.mic.profile.generic_class,
            Some(MicClass::XlrDynamicLowOutput | MicClass::XlrCondenser | MicClass::UsbCondenserSide | MicClass::UsbCondenserEnd | MicClass::HybridDynamicEnd | MicClass::WirelessLav)
        )
    }

    /// `match_mic_view`, but with the user's choice applied: when they named
    /// the mic, that profile (and its questions) replaces the name guess.
    pub fn mic_view_for(&self, name: &str, usb: Option<(u16, u16)>, choice: Option<&MicBehind>) -> Option<ProfileMatch> {
        let detected = self.match_mic_view(name, usb);
        let chosen = choice
            .filter(|c| c.status == MicBehindStatus::Chosen)
            .and_then(|c| c.profile_id.as_deref())
            .and_then(|id| self.profiles.mic(id));
        let Some(p) = chosen else { return detected };
        Some(ProfileMatch {
            kind: "mic".into(),
            profile_id: p.id.clone(),
            display_name: mic_label(p),
            confidence: Confidence::Exact,
            verified: p.verified,
            questions: self.profiles.questions_for_mic(&p.id),
            // The interface it sits behind is still what the Windows name told us.
            interface: detected.and_then(|d| d.interface),
            profile: serde_json::to_value(p).unwrap_or(Value::Null),
            chosen: true,
        })
    }

    /// Save (or clear) the answer for one device. Backed up in the vault first;
    /// if the backup can't be written nothing changes.
    pub async fn set_mic_behind(&self, device_id: &str, input: MicBehindInput) -> R<MicBehindResult> {
        let dev = self.platform.audio.list_inputs().into_iter().find(|d| d.id == device_id).ok_or("That microphone isn't connected any more.")?;
        let now = crate::paths::now_iso();
        let make = |status, p: Option<&MicProfile>| MicBehind {
            status,
            profile_id: p.map(|p| p.id.clone()),
            display_name: p.map(mic_label),
            device_name: dev.name.clone(),
            chosen_at: now.clone(),
        };
        let choice = match input {
            MicBehindInput::Profile { profile_id } => {
                let p = self
                    .profiles
                    .mic(&profile_id)
                    .filter(|p| p.generic_class.is_none())
                    .ok_or("We don't know that microphone. Pick another one from the list, or choose \"Other\".")?;
                Some(make(MicBehindStatus::Chosen, Some(p)))
            }
            MicBehindInput::Other => Some(make(MicBehindStatus::Other, None)),
            MicBehindInput::Skipped => Some(make(MicBehindStatus::Skipped, None)),
            MicBehindInput::Clear => None,
        };
        self.backup_before(&format!("Before choosing which mic is plugged into {}", plain_device_name(&dev.name)), &[Target::MicChoice(device_id.to_string())])
            .await?;
        self.put_mic_choice(device_id, choice.clone())?;
        Ok(MicBehindResult {
            profile: self.mic_view_for(&dev.name, dev.usb, choice.as_ref()),
            needs_mic_choice: choice.is_none() && self.asks_for_mic(&dev.name, dev.usb),
            mic_choice: choice,
        })
    }

    /// Write one device's answer into `setup.json`, keeping everything else.
    /// (Also what restoring a vault item does.)
    pub(super) fn put_mic_choice(&self, device_id: &str, choice: Option<MicBehind>) -> R<()> {
        self.edit_setup(|p| match choice {
            Some(c) => {
                p.mic_behind.insert(device_id.to_string(), c);
            }
            None => {
                p.mic_behind.remove(device_id);
            }
        })
    }

    /// Read `setup.json` (blank if missing, set aside if unreadable), change it, write it back atomically.
    pub(super) fn edit_setup(&self, f: impl FnOnce(&mut SavedProfile)) -> R<()> {
        let path = self.setup_path();
        let mut p = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<SavedProfile>(&text) {
                Ok(p) => p,
                Err(_) => {
                    // Unreadable file: keep it aside rather than lose it, start fresh.
                    let _ = std::fs::rename(&path, path.with_extension("json.unreadable"));
                    blank_setup()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => blank_setup(),
            Err(e) => return Err(format!("Couldn't read your saved setup ({e}).")),
        };
        f(&mut p);
        std::fs::create_dir_all(&self.data_dir).map_err(|e| format!("Couldn't save ({e})."))?;
        let text = serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?;
        crate::wav::write_atomic(&path, text.as_bytes()).map_err(|e| format!("Couldn't save ({e})."))
    }
}

/// `setup.json` that only holds answers, no applied setup yet (`saved_at` empty).
fn blank_setup() -> SavedProfile {
    SavedProfile {
        name: String::new(),
        saved_at: String::new(),
        mic_chain: None,
        mic_behind: Default::default(),
        camera_settings: None,
        camera_id: None,
        obs_input: None,
        active_profile: Default::default(),
        mic_setups: Default::default(),
    }
}

fn mic_label(p: &MicProfile) -> String {
    format!("{} {}", p.brand, p.model).trim().to_string()
}
