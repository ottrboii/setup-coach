//! Measurements → plain-language mic findings, physical fix first.

use super::analyze::{AudioMetrics, NoiseCharacter};
use crate::{Area, Constraint, Finding, FixKind, Severity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capsule {
    Dynamic,
    Condenser,
    Electret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Address {
    /// Speak into the side (Yeti, QuadCast, AT2020…).
    Side,
    /// Speak into the end/top (SM7B, PodMic, MV7…).
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    Usb,
    Xlr,
    Headset,
    Bluetooth,
    Wireless,
    BuiltIn,
}

/// What we know about the mic from its profile and the user's answers.
/// Everything is optional: unknown gear still gets a full scan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MicContext {
    pub mic_name: Option<String>,
    pub capsule: Option<Capsule>,
    pub address: Option<Address>,
    pub connection: Option<Connection>,
    /// Typical clean gain this mic needs for speech (dB).
    pub gain_needed_db: Option<f64>,
    /// Interface/preamp maximum gain (dB).
    pub interface_max_gain_db: Option<f64>,
    /// Inline booster gain if present (dB).
    pub booster_db: Option<f64>,
    /// The user/device reports the gain knob is (near) maximum.
    pub gain_at_max: Option<bool>,
    /// The mic has onboard auto level / auto gain enabled or unknown.
    pub onboard_auto_level: Option<bool>,
    /// Ideal distance range from the profile (cm).
    pub distance_cm: Option<(f64, f64)>,
}

impl MicContext {
    fn distance_text(&self) -> String {
        match (self.distance_cm, self.capsule) {
            (Some((a, b)), _) => format!("{a:.0}–{b:.0} cm"),
            (None, Some(Capsule::Dynamic)) => "3–10 cm (about a fist away)".into(),
            (None, Some(_)) => "10–20 cm".into(),
            (None, None) => "about a fist to a hand-span away".into(),
        }
    }
    fn mic(&self) -> String {
        self.mic_name.clone().unwrap_or_else(|| "your mic".into())
    }
    fn gain_shortfall_db(&self) -> Option<f64> {
        match (self.gain_needed_db, self.interface_max_gain_db) {
            (Some(need), Some(max)) => Some(need - (max + self.booster_db.unwrap_or(0.0))),
            _ => None,
        }
    }
}

fn f(id: &str, sev: Severity, title: &str, why: &str, fix: &str, kind: FixKind) -> Finding {
    Finding::new(id, Area::Mic, sev, title, why, fix, kind)
}

/// Rules over the measurements. Thresholds are starting values from the
/// research brief and must be tuned on real recordings.
pub fn mic_findings(m: &AudioMetrics, ctx: &MicContext) -> Vec<Finding> {
    let mut out = Vec::new();

    if m.no_signal {
        out.push(
            f(
                "mic.no_signal",
                Severity::Critical,
                "We can't hear your mic at all",
                "The recording is silent. The mic may be muted, on the wrong input, or (for condenser mics) missing 48V phantom power.",
                "Check the mute button and that the mic is plugged into the input you picked. Condenser mic on an interface? Turn on 48V.",
                FixKind::Guided,
            ),
        );
        return out;
    }

    // Channels -------------------------------------------------------------
    if let Some(dead) = m.dead_channel {
        let side = if dead == 1 { "left" } else { "right" };
        out.push(
            f(
                "mic.one_channel_only",
                Severity::Critical,
                &format!("Viewers only hear you in their {side} ear"),
                "Your interface sends input 1 as one side of a stereo signal, so the other side is silent.",
                "We'll switch your mic to mono in OBS so both ears get your voice.",
                FixKind::Auto,
            )
            .with_evidence("dead_channel", dead as f64),
        );
    } else if let Some(c) = m.stereo_correlation {
        if c < 0.6 {
            out.push(
                f(
                    "mic.stereo_mode",
                    Severity::Minor,
                    "Your mic is in stereo mode",
                    "Stereo picks up the room and keyboard as much as your voice, and you move around in the image.",
                    "Set the mic's pattern to cardioid (the heart symbol). On a Yeti it's the knob on the back.",
                    FixKind::Manual,
                )
                .with_evidence("correlation", c),
            );
        }
    }

    // Level ------------------------------------------------------------------
    let shortfall = ctx.gain_shortfall_db();
    if m.speech_peak_dbfs < -24.0 {
        let sev = if m.speech_peak_dbfs < -32.0 { Severity::Critical } else { Severity::Major };
        let fix = if ctx.gain_at_max == Some(true) || shortfall.map(|s| s > 0.0).unwrap_or(false) {
            format!("Move closer ({}) first. Your gain is already near max, so we'll add clean digital gain and suggest a fix for the missing power.", ctx.distance_text())
        } else {
            format!("Move closer ({}), then turn the gain knob up until the meter hits the green zone while you talk.", ctx.distance_text())
        };
        out.push(
            f("mic.too_quiet", sev, "You're too quiet — viewers have to turn you up", "Your voice is reaching the computer at a low level.", &fix, FixKind::Manual)
                .with_evidence("speech_peak_dbfs", m.speech_peak_dbfs),
        );
    } else if m.speech_peak_dbfs > -6.0 {
        out.push(
            f(
                "mic.too_hot",
                Severity::Major,
                "Your normal talking is too loud for the mic",
                "There's no room left for laughs and shouts, so those will distort.",
                "Turn the gain knob down a little until normal talking sits in the middle of the green zone.",
                FixKind::Guided,
            )
            .with_evidence("speech_peak_dbfs", m.speech_peak_dbfs),
        );
    }
    if m.clipped_fraction > 1e-5 {
        out.push(
            f(
                "mic.clipping",
                Severity::Major,
                "You crackle and distort when you get loud",
                "Loud moments hit the mic's maximum level and get cut off (clipping). This can't be fixed afterwards.",
                "Turn the gain down until your loudest laugh stays out of the red. We'll add a limiter as a safety net.",
                FixKind::Guided,
            )
            .with_evidence("clipped_fraction", m.clipped_fraction),
        );
    } else if m.loud_peak_dbfs.map(|p| p > -3.0).unwrap_or(false) {
        out.push(
            f(
                "mic.near_clipping",
                Severity::Minor,
                "Your loudest moments are close to distorting",
                "Your laugh/shout gets within a hair of the maximum level.",
                "Turn the gain down a touch. The limiter we add will catch the rest.",
                FixKind::Guided,
            )
            .with_evidence("loud_peak_dbfs", m.loud_peak_dbfs.unwrap_or(0.0)),
        );
    }

    // Gain shortfall (SM7B on a small interface) -----------------------------------
    if let Some(s) = shortfall {
        if s > 2.0 && (m.noise_character == NoiseCharacter::Hiss || m.snr_db < 45.0 || ctx.gain_at_max == Some(true)) {
            out.push(
                f(
                    "mic.not_enough_gain",
                    Severity::Major,
                    &format!("{} needs more power than your interface can give", ctx.mic()),
                    &format!("This mic needs about {:.0} dB of clean gain; your setup gives about {:.0} dB short. Turning it up all the way adds hiss.", ctx.gain_needed_db.unwrap_or(0.0), s),
                    &format!("Talk closer ({}). For the full fix, add an inline mic booster (about +25 dB) or use an interface with more gain.", ctx.distance_text()),
                    FixKind::Manual,
                )
                .with_evidence("gain_shortfall_db", s)
                .suggesting("inline_booster"),
            );
        }
    }

    // Noise --------------------------------------------------------------------------
    if m.snr_db < 40.0 || m.noise_floor_dbfs > -50.0 {
        let sev = if m.snr_db < 30.0 || m.noise_floor_dbfs > -45.0 { Severity::Major } else { Severity::Minor };
        let (title, why, fix) = match m.noise_character {
            NoiseCharacter::Hiss => (
                "There's a hiss behind your voice",
                "That's usually the mic gain turned up very high to make up for distance or a quiet mic.",
                format!("Move closer ({}) and lower the gain. We'll add a gentle expander so the hiss disappears between sentences.", ctx.distance_text()),
            ),
            NoiseCharacter::Rumble => (
                "There's a rumble/whoosh behind your voice",
                "A fan, PC or air conditioner is being picked up.",
                "Point the back of the mic at the noise, move the mic away from the PC, and talk closer so you can lower the gain. We'll filter the low rumble and quiet the gaps.".to_string(),
            ),
            NoiseCharacter::Broadband => (
                "Background noise is loud compared to your voice",
                "The mic hears the room almost as much as you.",
                format!("Move closer ({}), lower the gain, and turn off or move noisy things. We'll quiet the gaps between sentences.", ctx.distance_text()),
            ),
        };
        out.push(
            f("mic.background_noise", sev, title, why, &fix, FixKind::Manual)
                .with_evidence("noise_floor_dbfs", m.noise_floor_dbfs)
                .with_evidence("snr_db", m.snr_db),
        );
    }
    if let Some(h) = m.hum {
        out.push(
            f(
                "mic.hum",
                if h.prominence_db > 20.0 { Severity::Major } else { Severity::Minor },
                "There's an electrical hum or buzz",
                &format!("A {:.0} Hz hum from the power grid is getting in, usually a ground loop or a cable near a power supply.", h.mains_hz),
                "Plug the PC and audio gear into the same power strip, keep audio cables away from power bricks, and try another USB port. A laptop? Test on battery.",
                FixKind::Manual,
            )
            .with_evidence("hum_hz", h.mains_hz)
            .with_evidence("prominence_db", h.prominence_db),
        );
    }

    // Bandwidth -------------------------------------------------------------------------
    if m.upper_bandwidth_hz < 8000.0 {
        let bt = ctx.connection == Some(Connection::Bluetooth);
        out.push(
            f(
                "mic.phone_quality",
                if m.upper_bandwidth_hz < 4500.0 { Severity::Critical } else { Severity::Major },
                "You sound like a phone call",
                if bt {
                    "Bluetooth headsets switch to a low-quality 'hands-free' mode when the mic is used."
                } else {
                    "The highs of your voice are cut off — usually a Bluetooth headset, a low sample rate, or a very cheap mic."
                },
                if bt {
                    "Use a wired or 2.4 GHz wireless headset, or a separate mic."
                } else {
                    "Check you picked the right mic in OBS, and that it runs at 48 kHz in Windows sound settings."
                },
                FixKind::Guided,
            )
            .with_evidence("upper_bandwidth_hz", m.upper_bandwidth_hz),
        );
    }

    // Room ---------------------------------------------------------------------------------
    let roomy = m.reverb_decay_s.map(|r| r > 0.45).unwrap_or(false);
    let far = roomy && (m.speech_peak_dbfs < -20.0 || m.tone.body_db < -4.0);
    if far {
        out.push(
            f(
                "mic.too_far",
                Severity::Major,
                "You sound far away",
                "The mic hears a lot of room echo compared to your voice, which happens when it's too far from your mouth.",
                &format!("Bring the mic to {} and point it at your mouth. This is the single biggest improvement for most setups.", ctx.distance_text()),
                FixKind::Manual,
            )
            .with_evidence("reverb_decay_s", m.reverb_decay_s.unwrap_or(0.0))
            .suggesting("boom_arm"),
        );
    } else if roomy {
        let r = m.reverb_decay_s.unwrap_or(0.0);
        out.push(
            f(
                "mic.roomy",
                if r > 0.7 { Severity::Major } else { Severity::Minor },
                "You sound like you're in a bathroom",
                "Hard walls and an empty room bounce your voice back into the mic. Settings can't remove echo.",
                "Talk closer to the mic. Add soft things: a rug, curtains, a blanket behind your monitor or on the wall you face.",
                FixKind::Manual,
            )
            .with_evidence("reverb_decay_s", r)
            .suggesting("acoustic_panels"),
        );
    }
    if let Some(r) = m.reflection {
        out.push(
            f(
                "mic.reflection",
                if r.ripple_db > 8.0 { Severity::Major } else { Severity::Minor },
                "You sound hollow or phasey",
                &format!("Your voice bounces off a hard surface about {:.0} cm away (often the desk or monitor) and mixes back in.", r.path_cm / 2.0),
                "Lift the mic off the desk on a boom arm or stand and angle it so the desk is behind it. Put a towel or mouse pad under it to test. EQ can't fix this one.",
                FixKind::Manual,
            )
            .with_evidence("delay_ms", r.delay_ms)
            .with_evidence("ripple_db", r.ripple_db)
            .suggesting("boom_arm"),
        );
    }

    // Plosives & sibilance --------------------------------------------------------------
    if m.plosive_events > 0 {
        let worst = m.plosive_worst_db.unwrap_or(0.0);
        out.push(
            f(
                "mic.plosives",
                if worst > 3.0 || m.plosive_events > 4 { Severity::Major } else { Severity::Minor },
                "Your P's and B's pop",
                "Puffs of air from 'p' and 'b' hit the mic directly.",
                "Angle the mic slightly to the side of your mouth (20–45°) or add a pop filter. We'll add a low-cut filter too.",
                FixKind::Manual,
            )
            .with_evidence("events", m.plosive_events as f64)
            .with_evidence("worst_db", worst)
            .suggesting("pop_filter"),
        );
    }
    if let Some(s) = m.sibilance_db {
        if s > -3.0 {
            out.push(
                f(
                    "mic.sibilance",
                    if s > 3.0 { Severity::Major } else { Severity::Minor },
                    "Your S sounds are sharp",
                    "The 's' and 'sh' sounds are much louder than the rest of your voice.",
                    "Turn the mic slightly off-axis so you talk just past it. We'll add a gentle de-esser.",
                    FixKind::Manual,
                )
                .with_evidence("sibilance_db", s),
            );
        }
    }

    // Tone (physical causes first) ---------------------------------------------------------
    let t = m.tone;
    let wrong_side = ctx.address == Some(Address::Side) && t.presence_db < -5.0 && (m.speech_peak_dbfs < -20.0 || t.air_db < -6.0);
    if wrong_side {
        out.push(
            f(
                "mic.wrong_side",
                Severity::Major,
                "You might be talking into the top of the mic",
                &format!("{} picks up from the side (the side with the logo), not the top. From the top you sound muffled and far away.", ctx.mic()),
                "Turn the mic so the logo side faces your mouth.",
                FixKind::Manual,
            )
            .with_evidence("presence_db", t.presence_db),
        );
    }
    if t.boom_db > 5.0 {
        out.push(
            f("mic.boomy", if t.boom_db > 9.0 { Severity::Major } else { Severity::Minor }, "Your voice is boomy", "Too much deep bass, usually from being very close to a directional mic (proximity effect) or the room.", "Back off a finger-width or two. We'll add a low-cut filter.", FixKind::Auto)
                .with_evidence("boom_db", t.boom_db),
        );
    }
    if t.mud_db > 4.0 {
        out.push(
            f("mic.muddy", if t.mud_db > 7.0 { Severity::Major } else { Severity::Minor }, "Your voice sounds muddy", "There's a build-up in the low-mids that blurs your words.", "Back off slightly from the mic if you're very close. We'll gently cut the muddy range.", FixKind::Auto)
                .with_evidence("mud_db", t.mud_db),
        );
    }
    if t.box_db > 4.0 && t.mud_db <= 4.0 {
        out.push(
            f("mic.boxy", Severity::Minor, "You sound boxy", "Small, bare rooms and nearby walls add a 'cardboard box' tone.", "Soft furnishings help most. We'll trim the boxy range.", FixKind::Auto)
                .with_evidence("box_db", t.box_db),
        );
    }
    if t.nasal_db > 4.0 {
        out.push(
            f("mic.nasal", Severity::Minor, "You sound nasal / honky", "A peak in the upper mids makes the voice sound pinched.", "Aim the mic at your mouth, not your nose. We'll trim the peak.", FixKind::Auto)
                .with_evidence("nasal_db", t.nasal_db),
        );
    }
    let phone = m.upper_bandwidth_hz < 8000.0;
    if t.presence_db > 4.0 {
        out.push(
            f("mic.harsh", if t.presence_db > 7.0 { Severity::Major } else { Severity::Minor }, "Your voice is harsh", "Too much upper-mid energy makes you tiring to listen to.", "Angle the mic slightly off-axis. We'll smooth the harsh range.", FixKind::Auto)
                .with_evidence("presence_db", t.presence_db),
        );
    } else if t.presence_db < -6.0 && !wrong_side && !phone {
        out.push(
            f("mic.dull", if t.presence_db < -9.0 { Severity::Major } else { Severity::Minor }, "You sound muffled", "The clarity range of your voice is low — often the mic is pointed away from your mouth or covered.", "Point the mic straight at your mouth and remove thick foam covers. We'll add a little clarity.", FixKind::Auto)
                .with_evidence("presence_db", t.presence_db),
        );
    }
    if t.body_db < -8.0 && !far {
        out.push(
            f("mic.thin", Severity::Minor, "Your voice sounds thin", "Your voice is missing its warmth, usually from sitting too far or a low-cut switch on the mic.", &format!("Move a bit closer ({}) and check the mic's bass-cut switch is off. We'll add a touch of warmth.", ctx.distance_text()), FixKind::Manual)
                .with_evidence("body_db", t.body_db),
        );
    }

    // Other steps ---------------------------------------------------------------------------
    if let Some(k) = m.typing_rel_db {
        if k > -20.0 {
            out.push(
                f("mic.keyboard", if k > -12.0 { Severity::Major } else { Severity::Minor }, "Your keyboard is too loud", "The mic hears your typing almost as loud as your voice.", "Move the mic closer to your mouth and point its back at the keyboard. A boom arm helps. We'll quiet the gaps.", FixKind::Manual)
                    .with_evidence("typing_rel_db", k)
                    .suggesting("boom_arm"),
            );
        }
    }
    if let Some(v) = super::vibration::assess(m) {
        let vi = &m.vibration;
        let mut fd = f(
            "mic.desk_thumps",
            if v.major() { Severity::Major } else { Severity::Minor },
            "Desk bumps and thumps shake your mic",
            "Your mic is picking up shaking from the desk through its arm or stand. If the boom arm, the cable or your forearm touches the desk, every bump, key press and hit goes straight into the mic as a thud. That is why desk hits sound so loud and your mic sounds louder than it should. Turning the gain down only hides it for a moment and makes your voice quieter.",
            "Fix the touch first. 1) Look for anything touching the desk besides the clamp: the arm, the cable, your forearm. Lift it free. 2) Put a piece of foam or rubber under the clamp or base. 3) Hang the mic in a shock mount. 4) Leave a loose loop of cable. Then run the test again. Please do not turn the gain down for this. If hits are still loud after that, we add a gentle low-cut filter as a backup.",
            FixKind::Manual,
        )
        .suggesting("shock_mount");
        if let Some(tp) = m.tap_rel_db {
            fd = fd.with_evidence("tap_rel_db", tp);
        }
        for (k, val) in [
            ("tap_ring_ms", vi.tap_ring_ms),
            ("tap_lf_gap_db", vi.tap_lf_gap_db),
            ("typing_lf_rel_db", vi.typing_lf_rel_db),
            ("thump_worst_db", vi.thump_worst_db),
        ] {
            if let Some(x) = val {
                fd = fd.with_evidence(k, x);
            }
        }
        fd = fd.with_evidence("thump_events", vi.thump_events as f64).with_evidence("strong_clue", v.strong as u8 as f64).with_evidence("weak_clues", v.weak as f64);
        out.push(fd);
        // The loud keys are the same problem; do not send the user to the keyboard advice.
        if let Some(k) = out.iter_mut().find(|x| x.id == "mic.keyboard") {
            k.why = "Most of what the mic hears is the desk shaking, not the keys in the air (see the desk bumps card).".into();
            k.fix = "Fix the desk touch first. If typing is still loud after that, move the mic closer to your mouth and point its back at the keyboard.".into();
        }
    }
    if m.dropouts > 0 || m.crackles > 0 {
        out.push(
            f("mic.cable_crackle", Severity::Major, "Your audio crackles or cuts out when the cable moves", "A loose connector or a damaged cable.", "Re-seat both ends of the cable; if it still happens, replace the cable.", FixKind::Manual)
                .with_evidence("dropouts", m.dropouts as f64)
                .with_evidence("crackles", m.crackles as f64)
                .suggesting("mic_cable"),
        );
    }
    let swing = m.moving_drop_db.unwrap_or(0.0).max(m.moving_spread_db.unwrap_or(0.0));
    if swing > 6.0 {
        out.push(
            f("mic.level_swings", if swing > 10.0 { Severity::Major } else { Severity::Minor }, "Your volume jumps when you lean back or turn", "You move off the mic's sweet spot.", "Put the mic on a boom arm right in front of your face so it stays with you. We'll even out the level.", FixKind::Manual)
                .with_evidence("moving_drop_db", m.moving_drop_db.unwrap_or(0.0))
                .suggesting("boom_arm"),
        );
    }
    if m.loud_vs_read_db.map(|d| d < 2.0).unwrap_or(false) && ctx.onboard_auto_level != Some(false) {
        out.push(
            f("mic.auto_level_active", Severity::Info, "Your mic seems to be leveling itself", "Your loud laugh came out about as loud as normal talking — usually the mic's own auto-level/auto-gain.", "We'll suggest turning auto level off in the mic's app so our settings stay predictable (keep clip protection on).", FixKind::Guided)
                .with_evidence("loud_vs_read_db", m.loud_vs_read_db.unwrap_or(0.0)),
        );
    }
    if m.dc_offset.abs() > 0.01 {
        out.push(f("mic.dc_offset", Severity::Info, "Your mic has a small DC offset", "A hardware quirk; harmless once filtered.", "Our low-cut filter removes it.", FixKind::Auto).with_evidence("dc_offset", m.dc_offset));
    }

    out
}

/// Workaround text per declined constraint, and which findings it covers.
fn constraint_map(c: Constraint) -> (&'static [&'static str], &'static str) {
    match c {
        Constraint::CantMoveMicCloser => (&["mic.too_far", "mic.thin"], "Got it — we've tuned for the distance: gentler compression (so the room isn't pulled up), a tighter expander and a little clarity boost."),
        Constraint::CantAddCleanGain => (&["mic.not_enough_gain"], "Got it — we're using your interface's max clean gain plus digital gain, with an expander to hide hiss between sentences."),
        Constraint::CantRemoveNoiseSource => (&["mic.background_noise"], "Got it — we've set a stronger expander and low-cut so the noise drops out between sentences. Point the back of the mic at the noise if you can."),
        Constraint::CantFixReflection => (&["mic.reflection"], "Got it — we're not trying to EQ this (it can't be). Small angle changes still help."),
        Constraint::CantTreatRoom => (&["mic.roomy"], "Got it — we've reduced compression so the room isn't pulled up, and tightened the expander to shorten echo tails."),
        Constraint::MovesALot => (&["mic.level_swings"], "Got it — we've added more level smoothing so moving around is less noticeable."),
        Constraint::CantFixPlosives => (&["mic.plosives"], "Got it — we've raised the low-cut filter a little to soften pops."),
        Constraint::CantFixDeskVibration => (&["mic.desk_thumps"], "Got it — we've set a steeper low-cut filter to take the edge off desk thumps. It can't remove them completely."),
        _ => (&[], ""),
    }
}

/// Mark findings the user said they can't fix: downgrade to Info and explain
/// the workaround applied by [`super::compute_chain`].
pub fn apply_constraints(findings: &mut [Finding], constraints: &[Constraint]) {
    for c in constraints {
        let (ids, text) = constraint_map(*c);
        for f in findings.iter_mut().filter(|f| ids.contains(&f.id.as_str())) {
            f.evidence.insert("declined_severity".into(), f.severity as u8 as f64);
            f.severity = Severity::Info;
            f.fix = text.to_string();
            f.fix_kind = FixKind::Auto;
            f.can_decline = false;
        }
    }
}
