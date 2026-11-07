//! "Make it sound better": tune the processing chain's EQ / de-esser to the
//! mic's published response and the user's voice (core `tune::compose`),
//! check it offline, let the user hear a loudness-matched Before / After and
//! apply it through the settings vault (undoable). Plain filters only: no AI
//! processing. Verification is offline only (docs/voice-tuning-design.md,
//! decision 3 of 2026-11-07): it re-renders the recording, it does not watch
//! the live path.
//!
//! Flow: the UI runs the usual mic test (its Read step is the reading; one
//! younger than `voice_tune_max_age` with the same mic and profile is reused),
//! then `start_voice_tune` -> Before / After -> `apply_voice_tune` ("Keep it"),
//! `gentler_voice_tune`, `undo_voice_tune`.

use super::*;
use coach_core::audio::chain::EqBand;
use coach_core::audio::tune::{self, BandOrigin, FallbackReason, RenderCheck, TuneInput, Verdict, VoiceTune};
use coach_core::audio::voice::{Confidence, PitchClass, TiltClass};
use coach_core::{SetupProfile, VoiceoverTarget};

/// Strength steps of "Gentler": all moves, half, a quarter.
const STRENGTHS: [f64; 3] = [1.0, 0.5, 0.25];
/// Same tolerance as the composer's V1.
const TONE_TOLERANCE_DB: f64 = 3.0;

pub(super) struct TuneSession {
    id: String,
    device_id: String,
    profile: SetupProfile,
    target: VoiceoverTarget,
    ctx: MicContext,
    test: MicTest,
    tune: VoiceTune,
    /// What the Before is rendered with: the verification's reference: the chain the user has now (`tune.base`, its EQ and de-esser included).
    reference: ProcessingChain,
    mic: TuneMicInfo,
    strength: f64,
    /// Loudness-matched (Before, After) of the current strength; None when nothing changes.
    audio: Option<(AudioBuffer, AudioBuffer)>,
    /// Strength that is applied right now and the vault id from before the first apply.
    applied_strength: Option<f64>,
    backup_id: Option<String>,
}

/// What undoing a tune also puts back (the stored setup), keyed by its vault backup.
pub(super) struct TuneUndo {
    backup_id: String,
    profile: SetupProfile,
    device_id: String,
    prev: Option<StoredMicSetup>,
}

fn err(code: VoiceTuneErrorCode, msg: &str) -> VoiceTuneError {
    VoiceTuneError::new(code, msg)
}

fn stale() -> VoiceTuneError {
    err(VoiceTuneErrorCode::Stale, "That result is out of date. Please start again.")
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// The chain for a strength: today's chain when nothing verified, the tune as
/// composed at strength 1, otherwise its EQ and de-esser moves scaled down.
fn chain_at(t: &VoiceTune, strength: f64) -> ProcessingChain {
    if !t.verdict.applied() {
        return t.base.clone();
    }
    let mut c = t.chain.clone();
    if strength >= 1.0 {
        return c;
    }
    for b in c.eq.iter_mut() {
        b.gain_db = round1(b.gain_db * strength);
    }
    c.eq.retain(|b| b.gain_db != 0.0);
    if c.deesser != t.base.deesser {
        if let Some(d) = c.deesser.as_mut() {
            d.max_reduction_db = round1(d.max_reduction_db * strength);
        }
    }
    c
}

fn region_label(id: &str) -> &'static str {
    match id {
        "boom" => "Boominess",
        "body" => "Body",
        "mud" => "Muddiness",
        "box" => "Boxiness",
        "nasal" => "Nasal tone",
        "presence" => "Clarity",
        _ => "Air",
    }
}

fn state(d: f64) -> ToneState {
    if d > TONE_TOLERANCE_DB {
        ToneState::TooMuch
    } else if d < -TONE_TOLERANCE_DB {
        ToneState::TooLittle
    } else {
        ToneState::OnTarget
    }
}

fn phrase(id: &str, s: ToneState) -> &'static str {
    match (s, id) {
        (ToneState::OnTarget, _) => "about right",
        (ToneState::TooMuch, _) => "a bit much",
        (ToneState::TooLittle, "presence" | "air") => "a bit dull",
        (ToneState::TooLittle, _) => "a bit thin",
    }
}

fn tone_lines(before: &RenderCheck, after: &RenderCheck) -> Vec<ToneLine> {
    tune::REGIONS
        .iter()
        .enumerate()
        .filter_map(|(i, (id, _, _))| {
            let (b, a) = (state(*before.regions_db.get(i)?), state(*after.regions_db.get(i)?));
            let label = region_label(id);
            let text = if a == b { format!("{label}: {}", phrase(id, a)) } else { format!("{label}: {}, now {}", phrase(id, b), phrase(id, a)) };
            Some(ToneLine { id: (*id).to_string(), label: label.to_string(), before: b, after: a, text })
        })
        .collect()
}

fn voice_fact(v: &coach_core::audio::voice::VoiceType) -> String {
    if v.confidence == Confidence::Insufficient {
        return "Your voice: we didn't hear enough of it to describe it yet".into();
    }
    let pitch = match v.pitch {
        PitchClass::Deep => Some("deep"),
        PitchClass::LowMid => Some("fairly low"),
        PitchClass::HighMid => Some("mid-range"),
        PitchClass::High => Some("high"),
        PitchClass::Unknown => None,
    };
    let tilt = match v.tilt {
        TiltClass::Dark => Some("warm"),
        TiltClass::Neutral => Some("balanced"),
        TiltClass::Bright => Some("bright"),
        TiltClass::Unknown => None,
    };
    match (pitch, tilt) {
        (Some(p), Some(t)) => format!("Your voice: {p} and {t}"),
        (Some(w), None) | (None, Some(w)) => format!("Your voice: {w}"),
        (None, None) => "Your voice: we couldn't tell yet".into(),
    }
}

fn mic_fact(m: &TuneMicInfo) -> String {
    match (&m.name, m.has_curve) {
        (Some(n), true) => format!("Your mic: {n}, we know how it colours sound ({})", if m.verified { "from measurements" } else { "from the maker's chart" }),
        (Some(n), false) => format!("Your mic: {n}, we don't have its sound chart, so we only listen to your voice"),
        (None, _) => "Your mic: we don't know which one it is, so we only listen to your voice".into(),
    }
}

fn target_option(t: VoiceoverTarget) -> VoiceoverTargetOption {
    let (label, summary) = match t {
        VoiceoverTarget::Youtube => ("YouTube video", "Loud and clear, the level video sites play at."),
        VoiceoverTarget::Podcast => ("Podcast", "A little quieter, the level podcast apps aim for."),
        VoiceoverTarget::Audiobook => ("Audiobook (ACX)", "Quiet and even, with extra room at the loud moments, as audiobook stores ask."),
    };
    VoiceoverTargetOption { id: t, label: label.into(), summary: summary.into(), tuning: SetupProfile::Voiceover.tuning_for(t) }
}

impl AppCore {
    // ------------------------------------------------------------ voiceover goal

    /// The user's voiceover loudness goal (YouTube video until they choose).
    pub fn voiceover_target(&self) -> VoiceoverTarget {
        self.load_setup().map(|p| p.voiceover_target).unwrap_or_default()
    }

    pub fn voiceover_target_info(&self) -> VoiceoverTargetInfo {
        VoiceoverTargetInfo {
            selected: self.voiceover_target(),
            options: [VoiceoverTarget::Youtube, VoiceoverTarget::Podcast, VoiceoverTarget::Audiobook].into_iter().map(target_option).collect(),
            applies_now: self.active_profile() == SetupProfile::Voiceover,
            needs_rescan: false,
        }
    }

    /// Save the goal with the setup profile (`setup.json`) and restamp the
    /// session's context. A running voice tune is dropped: it was made for the
    /// old goal.
    pub fn set_voiceover_target(&self, target: VoiceoverTarget) -> R<VoiceoverTargetInfo> {
        let changed = self.voiceover_target() != target;
        self.edit_setup(|p| p.voiceover_target = target)?;
        let mut info = self.voiceover_target_info();
        let mut inner = self.inner.lock().unwrap();
        if let Some(c) = inner.last_context.as_mut() {
            c.voiceover_target = target;
        }
        if changed {
            inner.voice_tune = None;
        }
        info.needs_rescan = changed && info.applies_now && inner.last_report.is_some();
        Ok(info)
    }

    // ------------------------------------------------------------ tune

    /// Compose and verify the tune for the last mic test's reading.
    pub fn start_voice_tune(&self, device_id: &str) -> Result<VoiceTuneResult, VoiceTuneError> {
        use VoiceTuneErrorCode::*;
        let dev = self.platform.audio.list_inputs().into_iter().find(|d| d.id == device_id).ok_or(err(MicMissing, "That microphone isn't connected any more."))?;
        let (test, ctx, constraints, fresh) = {
            let i = self.inner.lock().unwrap();
            let fresh = i.last_device.as_deref() == Some(device_id) && i.last_test_at.is_some_and(|t| t.elapsed() <= self.voice_tune_max_age);
            (i.last_test.clone(), i.last_context.clone(), i.last_constraints.clone(), fresh)
        };
        let needs_read = || err(NeedsRead, "Read the lines out loud first, so we can hear your voice.");
        let (test, mut ctx) = match (test, ctx) {
            (Some(t), Some(c)) if fresh => (t, c),
            _ => return Err(needs_read()),
        };
        let profile = self.active_profile();
        if ctx.profile != profile {
            return Err(needs_read());
        }
        let target = self.voiceover_target();
        ctx.voiceover_target = target;
        let mic = self.effective_mic(&dev);
        let response = mic.and_then(|m| m.response.as_ref());
        let mic_info = TuneMicInfo {
            name: ctx.mic_name.clone().or_else(|| mic.map(|m| format!("{} {}", m.brand, m.model).trim().to_string())),
            has_curve: response.is_some(),
            verified: response.is_some_and(|r| r.verified),
            source: response.map(|r| r.source.clone()),
        };
        let tune = tune::compose(&TuneInput { test: &test, ctx: &ctx, constraints: &constraints, mic: response, switch: None });
        let mut reference = tune.base.clone();
        reference.eq.clear();
        reference.deesser = None;
        process::calibrate_makeup_for(&test, &mut reference, &profile.tuning_for(target));
        let id = {
            let mut i = self.inner.lock().unwrap();
            i.voice_tune_seq += 1;
            format!("tune-{}", i.voice_tune_seq)
        };
        let mut s = TuneSession { id, device_id: device_id.to_string(), profile, target, ctx, test, tune, reference, mic: mic_info, strength: 1.0, audio: None, applied_strength: None, backup_id: None };
        let result = self.render_tune(&mut s)?;
        self.inner.lock().unwrap().voice_tune = Some(s);
        Ok(result)
    }

    /// "Gentler": the same tune at half the strength, then a quarter. Nothing is
    /// applied: call `apply_voice_tune` again (it re-applies over the earlier
    /// one and keeps the original undo point).
    pub fn gentler_voice_tune(&self, tune_id: &str) -> Result<VoiceTuneResult, VoiceTuneError> {
        let mut s = {
            let mut i = self.inner.lock().unwrap();
            if i.voice_tune.as_ref().is_none_or(|s| s.id != tune_id) {
                return Err(stale());
            }
            i.voice_tune.take().ok_or_else(stale)?
        };
        let out = (|| {
            if !s.tune.verdict.applied() {
                return Err(err(VoiceTuneErrorCode::NothingToApply, "There's nothing to make gentler: no changes were needed."));
            }
            let next = STRENGTHS.iter().copied().find(|x| *x < s.strength).unwrap_or(s.strength);
            s.strength = next;
            self.render_tune(&mut s)
        })();
        self.inner.lock().unwrap().voice_tune = Some(s);
        out
    }

    /// "Keep it": write the tuned chain (backup first). Refuses when the tune
    /// did not verify, so nothing is ever applied that did not measure better.
    pub async fn apply_voice_tune(&self, tune_id: &str) -> Result<VoiceTuneApplied, VoiceTuneError> {
        let (chain, strength, input, profile, device_id, first_backup) = {
            let i = self.inner.lock().unwrap();
            let s = i.voice_tune.as_ref().filter(|s| s.id == tune_id).ok_or_else(stale)?;
            if !s.tune.verdict.applied() {
                return Err(err(VoiceTuneErrorCode::NothingToApply, "Nothing was changed: the new settings didn't measure better than your current ones."));
            }
            (chain_at(&s.tune, s.strength), s.strength, i.applied_input.clone().unwrap_or_default(), s.profile, s.device_id.clone(), s.backup_id.clone())
        };
        let prev_setup = self.stored_mic_setup(profile, &device_id);
        let (res, new_backup) = self.apply_chain_with(chain, &input, Some("Before making your voice sound better".into()), true).await;
        if res.failed.iter().any(|f| f.id == "backup") {
            return Ok(VoiceTuneApplied {
                tune_id: tune_id.to_string(),
                strength,
                applied: res,
                backup_id: first_backup,
                line: "We couldn't save a backup of your current settings, so nothing was changed.".into(),
            });
        }
        let backup_id = first_backup.or(new_backup);
        let line = if res.failed.is_empty() {
            "Done. Your voice is tuned toward the targets. You can undo this any time.".to_string()
        } else {
            format!("Your new settings are saved, but not everything is switched on yet: {} You can undo this any time.", res.failed[0].error)
        };
        {
            let mut i = self.inner.lock().unwrap();
            if let Some(s) = i.voice_tune.as_mut().filter(|s| s.id == tune_id) {
                s.applied_strength = Some(strength);
                s.backup_id = backup_id.clone();
            }
            if let Some(b) = backup_id.clone() {
                if !i.tune_undo.iter().any(|u| u.backup_id == b) {
                    i.tune_undo.push(TuneUndo { backup_id: b, profile, device_id, prev: prev_setup });
                }
            }
        }
        Ok(VoiceTuneApplied { tune_id: tune_id.to_string(), strength, applied: res, backup_id, line })
    }

    /// Undo: put the vault backup back, and the stored setup that went with it.
    pub async fn undo_voice_tune(&self, backup_id: &str) -> ApplyResult {
        let res = self.restore_backup(backup_id).await;
        if !res.failed.is_empty() {
            return res;
        }
        let undo = {
            let mut i = self.inner.lock().unwrap();
            if let Some(s) = i.voice_tune.as_mut().filter(|s| s.backup_id.as_deref() == Some(backup_id)) {
                s.applied_strength = None;
                s.backup_id = None;
            }
            i.tune_undo.iter().position(|u| u.backup_id == backup_id).map(|p| i.tune_undo.remove(p))
        };
        if let Some(u) = undo {
            let chain = u.prev.as_ref().map(|p| p.mic_chain.clone());
            let input = u.prev.as_ref().and_then(|p| p.obs_input.clone());
            let _ = self.edit_setup(|p| {
                let m = p.mic_setups.entry(u.profile.id().to_string()).or_default();
                match u.prev {
                    Some(s) => {
                        m.insert(u.device_id.clone(), s);
                    }
                    None => {
                        m.remove(&u.device_id);
                    }
                }
            });
            let mut i = self.inner.lock().unwrap();
            i.applied_chain = chain;
            i.applied_input = input;
        }
        res
    }

    /// Before / After for `play_preview`.
    pub(super) fn tune_preview(&self, after: bool) -> R<AudioBuffer> {
        let i = self.inner.lock().unwrap();
        let (b, a) = i.voice_tune.as_ref().and_then(|s| s.audio.as_ref()).ok_or("There's no before and after to play: nothing was changed.")?;
        Ok(if after { a.clone() } else { b.clone() })
    }

    // ------------------------------------------------------------ result

    /// Render Before / After at the session's strength, write the WAVs and
    /// describe it all.
    fn render_tune(&self, s: &mut TuneSession) -> Result<VoiceTuneResult, VoiceTuneError> {
        let t = &s.tune;
        let chain = chain_at(t, s.strength);
        let tuning = s.profile.tuning_for(s.target);
        let can_apply = t.verdict.applied();

        // The reading, through today's chain and through the tune, equally loud.
        let (mut before_path, mut after_path, mut match_db) = (None, None, 0.0);
        s.audio = None;
        if can_apply {
            let mut channels = vec![Vec::new(); s.test.audio.channels.len()];
            for seg in s.test.segments.iter().filter(|x| x.kind == SegmentKind::Read) {
                for (c, ch) in s.test.audio.channels.iter().enumerate() {
                    channels[c].extend_from_slice(&ch[s.test.range(seg)]);
                }
            }
            let clip = AudioBuffer { sample_rate: s.test.audio.sample_rate, channels };
            let before = process::render(&clip, &s.reference);
            let raw_after = process::render(&clip, &chain);
            let after = tune::loudness_match(&before, &raw_after);
            let sr = before.sample_rate;
            let lufs = |b: &AudioBuffer| coach_core::audio::dsp::integrated_lufs(&super::ab::mono(b), sr);
            let (lb, la) = (lufs(&before), lufs(&raw_after));
            if lb.is_finite() && la.is_finite() {
                match_db = round1(lb - la);
            }
            let dir = self.data_dir.join("voice-tune");
            std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't save the before and after ({e})."))?;
            let (bp, ap) = (dir.join("before.wav"), dir.join("after.wav"));
            crate::wav::write(&bp, sr, &super::ab::mono(&before))?;
            crate::wav::write(&ap, sr, &super::ab::mono(&after))?;
            before_path = Some(bp.display().to_string());
            after_path = Some(ap.display().to_string());
            s.audio = Some((before, after));
        }

        // Summary and checks.
        let after_check = t.verification.as_ref().map(|v| if s.strength >= 1.0 || !can_apply { v.after.clone() } else { tune::check_render(&s.test, &s.ctx, &chain, &tuning) });
        let summary = match (&t.verification, &after_check) {
            (Some(v), Some(a)) if can_apply => tone_lines(&v.before, a),
            _ => Vec::new(),
        };
        let mut changes: Vec<String> = Vec::new();
        if can_apply {
            for b in &chain.eq {
                let origin = t.proposed.eq.iter().position(|p| p.kind == b.kind && p.freq_hz == b.freq_hz).and_then(|i| t.origins.get(i)).copied().unwrap_or(BandOrigin::Voice);
                let line = match origin {
                    BandOrigin::Mic => "Evens out how your mic colours your voice".to_string(),
                    _ => {
                        let r = b.reason.trim();
                        let mut cs = r.chars();
                        match cs.next() {
                            Some(c) if !r.is_empty() => c.to_uppercase().collect::<String>() + cs.as_str(),
                            _ => "A small tone adjustment".to_string(),
                        }
                    }
                };
                if !changes.contains(&line) {
                    changes.push(line);
                }
            }
            if chain.deesser != t.base.deesser && chain.deesser.is_some() {
                changes.push("Softens sharp \"s\" sounds".into());
            }
        }

        let headline = match (t.verdict, t.fallback) {
            (Verdict::Pass, _) => "Checked: it now sounds like we planned.".to_string(),
            (Verdict::Close, Some(FallbackReason::Reflection)) => "Close: we kept it gentle because the room adds echo - something soft on the wall would help.".to_string(),
            (Verdict::Close, _) => "Close: we kept it gentle, because part of what we saw is better fixed in the room or with your mic position than in settings.".to_string(),
            (Verdict::NothingToChange, Some(FallbackReason::NoSignal)) => "We couldn't hear you, so nothing was changed. Check that your mic is on and read the lines again.".to_string(),
            (Verdict::NothingToChange, _) => "Nothing to change: your voice already matches the targets for this setup.".to_string(),
            (Verdict::NotImproved, _) => "We tried a change, but it didn't measure better, so your settings stay as they are.".to_string(),
        };

        let bands = chain
            .eq
            .iter()
            .map(|b: &EqBand| {
                let origin = t.proposed.eq.iter().position(|p| p.kind == b.kind && p.freq_hz == b.freq_hz).and_then(|i| t.origins.get(i)).copied().unwrap_or(BandOrigin::Voice);
                TuneBandView { band: b.clone(), origin }
            })
            .collect();
        let goal = match s.profile {
            SetupProfile::Streaming => "Goal: Streaming".to_string(),
            SetupProfile::Call => "Goal: Calls and meetings".to_string(),
            SetupProfile::Voiceover => format!("Goal: Voiceover, {}", target_option(s.target).label),
        };
        Ok(VoiceTuneResult {
            tune_id: s.id.clone(),
            device_id: s.device_id.clone(),
            profile: s.profile,
            voiceover_target: (s.profile == SetupProfile::Voiceover).then_some(s.target),
            verdict: t.verdict,
            can_apply,
            strength: s.strength,
            can_be_gentler: can_apply && s.strength > STRENGTHS[STRENGTHS.len() - 1],
            headline,
            facts: TuneFacts { voice: voice_fact(&t.voice), mic: mic_fact(&s.mic), goal },
            changes,
            summary,
            notes: t.notes.clone(),
            before_path,
            after_path,
            applied: s.applied_strength == Some(s.strength),
            backup_id: s.backup_id.clone(),
            advanced: VoiceTuneAdvanced {
                voice: t.voice.clone(),
                curves: t.curves.clone(),
                hpf: t.hpf,
                bands,
                deesser: chain.deesser.clone(),
                verification: t.verification.clone(),
                after_check,
                mic: s.mic.clone(),
                tuning,
                fallback: t.fallback,
                loudness_match_db: match_db,
                verification_note: "Checked offline: we re-rendered your recording through the new settings and measured it again. We did not measure the live signal.".into(),
            },
        })
    }
}

