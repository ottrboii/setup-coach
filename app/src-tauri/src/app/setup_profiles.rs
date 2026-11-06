//! Setup profiles (streaming / voiceover / call): the session's profile lives in
//! `setup.json` (`active_profile`), and each (profile, mic) keeps its own stored
//! setup (`mic_setups`). A profile only moves the pass bars and the physical
//! guidance (core); the processing chain is the same for every profile.

use super::*;
use coach_core::SetupProfile;

impl AppCore {
    /// The session's profile: last used, streaming when nothing is saved.
    pub fn active_profile(&self) -> SetupProfile {
        self.load_setup().map(|p| p.active_profile).unwrap_or_default()
    }

    /// All three profiles, with `placement` worded for this device's mic.
    pub fn list_setup_profiles(&self, device_id: &str) -> R<Vec<coach_core::setup_profile::ProfileInfo>> {
        let ctx = self.mic_context(device_id, &MicAnswers::default())?;
        Ok(SetupProfile::ALL.iter().map(|p| p.info(&ctx)).collect())
    }

    /// The stored setup for (profile, device). An old `setup.json` with a
    /// `mic_chain` and no stored setups counts as the streaming setup.
    pub fn stored_mic_setup(&self, profile: SetupProfile, device_id: &str) -> Option<StoredMicSetup> {
        let p = self.load_setup()?;
        if let Some(s) = p.mic_setups.get(profile.id()).and_then(|m| m.get(device_id)) {
            return Some(s.clone());
        }
        let legacy_ok = profile == SetupProfile::Streaming && p.mic_setups.get(profile.id()).is_none_or(|m| m.is_empty());
        let chain = p.mic_chain.filter(|_| legacy_ok)?;
        Some(StoredMicSetup {
            profile,
            device_id: device_id.to_string(),
            saved_at: p.saved_at,
            mic_chain: chain,
            obs_input: p.obs_input,
            constraints: Vec::new(),
            score: 0,
            targets: Vec::new(),
        })
    }

    /// Switch the session's profile. A stored setup for (profile, current mic)
    /// is re-applied (backup first, so "go back" works); otherwise the UI is
    /// told to scan. Hardware gain and mic position are never claimed as done.
    pub async fn set_setup_profile(&self, profile: SetupProfile) -> R<SetupProfileSwitch> {
        let device = self.coach_mic_device();
        let stored = device.as_deref().and_then(|d| self.stored_mic_setup(profile, d));
        self.edit_setup(|p| p.active_profile = profile)?;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(c) = inner.last_context.as_mut() {
                c.profile = profile;
            }
            // "I can't do this" answers belong to the profile that gave them.
            inner.last_constraints = stored.as_ref().map(|s| s.constraints.clone()).unwrap_or_default();
        }
        let Some(s) = stored else {
            return Ok(SetupProfileSwitch { profile, applied: None, needs_scan: true });
        };
        let input = s.obs_input.clone().unwrap_or_default();
        let applied = self.apply_mic_chain_labeled(s.mic_chain, &input, Some(format!("Before switching to {}", profile.label()))).await;
        Ok(SetupProfileSwitch { profile, applied: Some(applied), needs_scan: false })
    }

    /// Store what a finished mic flow applied under the active profile and mic.
    pub(super) fn remember_mic_setup(&self, chain: &ProcessingChain, obs_input: &str) {
        let (device, report, constraints) = {
            let i = self.inner.lock().unwrap();
            (i.last_device.clone(), i.last_report.clone(), i.last_constraints.clone())
        };
        let Some(device_id) = device else { return };
        let profile = self.active_profile();
        let setup = StoredMicSetup {
            profile,
            device_id: device_id.clone(),
            saved_at: crate::paths::now_iso(),
            mic_chain: chain.clone(),
            obs_input: Some(obs_input.to_string()),
            constraints,
            score: report.as_ref().map(|r| r.score.value).unwrap_or(0),
            targets: report.map(|r| r.targets).unwrap_or_default(),
        };
        // Best effort: not being able to remember must not fail the apply.
        let _ = self.edit_setup(|p| {
            p.mic_setups.entry(profile.id().to_string()).or_default().insert(device_id, setup);
        });
    }

    /// After "did it work?": the stored setup carries the re-measured score and bars.
    pub(super) fn update_stored_result(&self, after: &MicReport) {
        let (device, applied) = {
            let i = self.inner.lock().unwrap();
            (i.last_device.clone(), i.applied_chain.is_some())
        };
        let (Some(device_id), true) = (device, applied) else { return };
        let profile = self.active_profile();
        if self.load_setup().is_none_or(|p| !p.mic_setups.get(profile.id()).is_some_and(|m| m.contains_key(&device_id))) {
            return;
        }
        let _ = self.edit_setup(|p| {
            if let Some(s) = p.mic_setups.get_mut(profile.id()).and_then(|m| m.get_mut(&device_id)) {
                s.score = after.score.value;
                s.targets = after.targets.clone();
            }
        });
    }
}
