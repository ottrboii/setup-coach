//! Setup profiles: what the user is setting up for in this session
//! (streaming, recording a voiceover, or a call / meeting).
//!
//! A profile moves the **pass bars**, the **physical guidance** and, only in
//! the documented fields below (loudness, limiter, compressor, expander cap,
//! no EQ boost above 7 kHz on calls), the **processing**. Tone is never moved
//! by the profile yet. "I can't do this" (`Constraint`) works the same everywhere.
//!
//! Where each number comes from (re-run the evidence with
//! `cargo run --release --example profile_sweep`):
//!
//! * **Streaming** is the behaviour before profiles existed, unchanged:
//!   research brief §6 (speech peaks −18…−12 dBFS, loudest ≤ −6, noise floor
//!   ≤ −60 / ≤ −50 OK / > −45 problem, speech-to-noise ≥ 50 / ≥ 40) and the
//!   existing room bars (≤ 0.35 s met, roomy finding above 0.45 s).
//! * **Voiceover** is never looser than streaming. Noise and level close bands
//!   come from the ACX audiobook submission rules (noise floor ≤ −60 dBFS,
//!   peaks ≤ −3 dBFS, RMS −23…−18 dB): normalising a take to ACX RMS puts the
//!   floor about 42 dB under active speech at the limit, so the close band for
//!   speech-to-noise ends at 45 dB (3 dB margin). Room bars come from the
//!   synthetic room sweep: a dry booth (RT60 ≈ 0.2 s) measures 0.20–0.26 s at
//!   every wet level, RT60 0.3 s measures 0.23–0.37 s, so met ≤ 0.27 s and
//!   close ≤ 0.35 s (streaming's met bar).
//! * **Call / meeting** is tolerant where intelligibility allows. Noise bars
//!   are the brief's "OK" (≤ −50) and "problem" (> −45) lines. At a normal
//!   talking level the sweep puts those floors at 25 and 20 dB speech-to-noise,
//!   and the floor of 15 dB for a major finding is the classroom
//!   intelligibility minimum (ANSI/ASA S12.60). Room: S12.60 asks RT60 ≤ 0.6 s
//!   for intelligible speech; the sweep measures RT60 0.6 s as 0.45–0.64 s, so
//!   close ≤ 0.65 s and met ≤ 0.5 s (RT60 ≤ 0.4 s at every wet level).
//!
//! Talking level and loudest-moment bars are the same in every profile: the
//! raw capture needs the same headroom whatever it is used for, and we have no
//! evidence to move them.
//!
//! # Which findings a profile hides
//!
//! A profile also decides which findings are worth showing at all. Hiding is
//! conservative: only things that cannot affect that use are dropped, and
//! nothing about the voice itself (every `mic.*` level, noise, room, tone,
//! hum, clipping, plosive finding, Windows enhancements, other apps holding
//! the mic) is ever hidden except the two listed below. It is applied in core
//! (`audio::scan` for the mic report) and by the app to the quick scan and
//! the camera report, so hidden items cost no score. The same table is sent to
//! the UI as `ProfileInfo::irrelevant`.
//!
//! | Finding id / group                                 | Streaming | Voiceover | Call   | Why |
//! |----------------------------------------------------|-----------|-----------|--------|-----|
//! | everything in area `camera` (`camera.*`, camera-side `connection.*`, capture-card links, HDMI) | shown | hidden | shown* | nobody sees a voiceover; calls do use the camera |
//! | everything in area `stream` (`stream.*`: encoder, bitrate, keyframes, upload, health) | shown | hidden | hidden | no live encode in a recording or a call |
//! | `stream.recording_plain_mp4` (OBS recording format) | shown    | shown     | hidden | a voiceover may be recorded in OBS |
//! | `stream.on_wifi`                                    | shown     | hidden    | shown  | Wi-Fi drops are heard on a call, not in a local recording |
//! | `mic.keyboard` (typing next to the mic)             | shown     | hidden    | shown  | you are not typing during a take; you do type on calls |
//! | `mic.capture_card_audio_and_mic`                    | shown     | hidden    | hidden | capture-card audio is stream gear |
//! | `system.obs_websocket_off`, `system.obs_outdated`   | shown     | shown     | hidden | OBS is not part of a call |
//! | `camera.hdmi_*`, `connection.capture_card_*`        | shown     | hidden    | hidden | HDMI capture is stream gear (*the rest of camera stays on calls) |
//!
//! Anything not in the table is shown in every profile. A mic-side USB hub
//! finding (`connection.behind_hub` on a mic) stays everywhere: it is in area
//! `mic`, not `camera`.
//!
//! # How a profile changes the processing (voice tuning, design section 3.3)
//!
//! Deliberately, and only in these fields of the chain ([`ProfileTuning`]):
//! loudness target, limiter ceiling, compressor ratio / gain reduction,
//! expander ratio cap (voiceover), and whether EQ may boost above 7 kHz (not on
//! a call: wideband call codecs stop at 7-8 kHz, RFC 6716 / G.722). Streaming is
//! the chain as it was before profiles. The profile tone deltas (+1 dB
//! presence for streaming and calls) have no published number behind them and
//! ship at 0 until a listening test (decision 2026-11-07).
//!
//! | | Streaming | Voiceover (YouTube / Podcast / Audiobook) | Call |
//! |---|---|---|---|
//! | Loudness | -16 LUFS | -14 LUFS / -16 LUFS / -20 dB RMS | -16 LUFS |
//! | Limiter ceiling | -1.5 dBFS | -1.5 / -1.5 / -3.5 dBFS | -1.5 dBFS |
//! | Compressor | 3:1, 4 dB | 2:1, 2.5 dB | 4:1, 5 dB |
//! | Expander ratio | as measured | at most 2:1 | as measured |
//! | EQ boosts above 7 kHz | yes | yes | no |
//!
//! Sources: Apple Podcasts -16 LKFS / true peak -1; YouTube -14 LUFS / -1 dBTP;
//! ACX RMS -23...-18 dB, peaks under -3 dB (0.5 dB margin because the OBS
//! limiter has no look-ahead; the same margin turns -1 dBTP into -1.5 dBFS).
//! Voiceover and call compressor settings are starting values.

use crate::audio::findings::{Capsule, Connection, MicContext};
use crate::finding::{Area, Finding};
use serde::{Deserialize, Serialize};

/// What this session is set up for. Chosen at the start; defaults to streaming.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupProfile {
    /// Live streaming: mic out of the camera's view, forgiving, steady.
    #[default]
    Streaming,
    /// Recording / voiceover: cleanest signal, closest good position, strict on room and noise.
    Voiceover,
    /// Call / meeting: intelligible and steady, tolerant, little tweaking.
    Call,
}

/// A "lower is better" bar: at or under `met` passes, at or under `close` is nearly there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MaxBar {
    pub met: f64,
    pub close: f64,
    /// Above this a finding is raised. Equal to `close` except streaming's
    /// room bar, which has always flagged at 0.45 s inside its 0.5 s close band.
    pub flag: f64,
    /// Above this the finding is Major (between `close` and here: Minor).
    pub major: f64,
}

/// A "higher is better" bar.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MinBar {
    pub met: f64,
    pub close: f64,
    /// Below this the finding is Major.
    pub major: f64,
}

/// Every number a profile changes. Findings fire past `flag` (max bars) or under `close` (min bars).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfileTargets {
    /// Normal talking peaks (dBFS): met inside `speech_peak`, close inside `speech_peak_close`.
    pub speech_peak: (f64, f64),
    pub speech_peak_close: (f64, f64),
    /// Loudest moments (dBFS, lower is better).
    pub loud_peak: MaxBar,
    /// Noise floor while silent (dBFS).
    pub noise_floor: MaxBar,
    /// Speech RMS above the noise floor (dB).
    pub snr: MinBar,
    /// Room decay as the analyser measures it (s), see `AudioMetrics::reverb_decay_s`.
    pub room_decay: MaxBar,
}

/// What the app shows for one profile. `targets` and `distance_cm` are for
/// the Advanced drawer only; everything else is plain language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub id: SetupProfile,
    pub label: String,
    pub summary: String,
    pub placement: String,
    pub room_advice: String,
    pub distance_cm: Option<(f64, f64)>,
    pub targets: ProfileTargets,
    /// Findings this profile never shows (additive field).
    #[serde(default)]
    pub irrelevant: IrrelevantFindings,
}

/// Which findings do not apply to a profile: whole areas (minus `keep`) and
/// single ids. The app and UI hide these; core applies it in `audio::scan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrrelevantFindings {
    /// Every finding in these areas is hidden, except the ids in `keep`.
    pub areas: Vec<Area>,
    /// Exceptions to `areas`.
    pub keep: Vec<String>,
    /// Single finding ids that are hidden, whatever their area.
    pub ids: Vec<String>,
}

/// Loudness goal for a voiceover: the user's choice (decision 2026-11-07).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceoverTarget {
    /// YouTube video: -14 LUFS integrated, true peak <= -1 dBTP.
    #[default]
    Youtube,
    /// Podcast: -16 LUFS (Apple Podcasts), true peak <= -1 dBTP.
    Podcast,
    /// Audiobook (ACX): -20 dB RMS (inside -23...-18), peaks <= -3 dBFS.
    Audiobook,
}

/// How a loudness target is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessMeasure {
    /// ITU-R BS.1770 integrated loudness of the read.
    Lufs,
    /// Plain RMS of the read (what ACX checks).
    Rms,
}

/// A loudness goal and the window that counts as "on target".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoudnessTarget {
    pub value: f64,
    pub measure: LoudnessMeasure,
    /// Accepted range (inclusive). ACX publishes one; elsewhere value +-1.
    pub accept: (f64, f64),
}

/// A profile tone delta. Off (0 dB) until a listening test shows it helps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneDelta {
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Always 0 for now; the design's proposal is +1 dB (unverified).
    pub db: f64,
}

/// Every way a profile changes the processing (module docs, design section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProfileTuning {
    pub loudness: LoudnessTarget,
    pub limiter_ceiling_db: f64,
    pub compressor_ratio: f64,
    /// Gain reduction on normal speech (dB).
    pub compressor_gr_db: f64,
    /// Upper bound on the expander ratio, if the profile sets one.
    pub expander_ratio_max: Option<f64>,
    /// EQ may boost above 7 kHz.
    pub boost_above_7k: bool,
    /// Frequency range the tone is judged on (Hz).
    pub judge_hz: (f64, f64),
    pub tone_delta: Option<ToneDelta>,
}

impl IrrelevantFindings {
    /// True when the finding does not apply.
    pub fn hides(&self, f: &Finding) -> bool {
        self.ids.iter().any(|i| i == &f.id) || (self.areas.contains(&f.area) && !self.keep.iter().any(|k| k == &f.id))
    }
}

impl SetupProfile {
    /// The findings this profile never shows. Table in the module docs.
    pub fn irrelevant(self) -> IrrelevantFindings {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            SetupProfile::Streaming => IrrelevantFindings::default(),
            SetupProfile::Voiceover => IrrelevantFindings {
                areas: vec![Area::Camera, Area::Stream],
                keep: s(&["stream.recording_plain_mp4"]),
                ids: s(&["mic.keyboard", "mic.capture_card_audio_and_mic"]),
            },
            SetupProfile::Call => IrrelevantFindings {
                areas: vec![Area::Stream],
                keep: s(&["stream.on_wifi"]),
                ids: s(&[
                    "system.obs_websocket_off",
                    "system.obs_outdated",
                    "mic.capture_card_audio_and_mic",
                    "camera.hdmi_interlaced",
                    "camera.hdmi_no_signal",
                    "camera.hdmi_overlay",
                    "connection.capture_card_on_slow_port",
                    "connection.capture_card_behind_hub",
                    "connection.capture_card_shares_controller",
                ]),
            },
        }
    }

    /// The processing this profile asks for, voiceover at its default goal (YouTube).
    pub fn tuning(self) -> ProfileTuning {
        self.tuning_for(VoiceoverTarget::default())
    }

    /// The processing this profile asks for; `vo` only matters for voiceover.
    pub fn tuning_for(self, vo: VoiceoverTarget) -> ProfileTuning {
        let lufs = |v: f64| LoudnessTarget { value: v, measure: LoudnessMeasure::Lufs, accept: (v - 1.0, v + 1.0) };
        let base = ProfileTuning {
            loudness: lufs(-16.0),
            limiter_ceiling_db: -1.5,
            compressor_ratio: 3.0,
            compressor_gr_db: 4.0,
            expander_ratio_max: None,
            boost_above_7k: true,
            judge_hz: (80.0, 10000.0),
            tone_delta: None,
        };
        match self {
            SetupProfile::Streaming => ProfileTuning { tone_delta: Some(ToneDelta { lo_hz: 2000.0, hi_hz: 4000.0, db: 0.0 }), ..base },
            SetupProfile::Voiceover => {
                let (loudness, limiter_ceiling_db) = match vo {
                    VoiceoverTarget::Youtube => (lufs(-14.0), -1.5),
                    VoiceoverTarget::Podcast => (lufs(-16.0), -1.5),
                    VoiceoverTarget::Audiobook => (LoudnessTarget { value: -20.0, measure: LoudnessMeasure::Rms, accept: (-23.0, -18.0) }, -3.5),
                };
                ProfileTuning { loudness, limiter_ceiling_db, compressor_ratio: 2.0, compressor_gr_db: 2.5, expander_ratio_max: Some(2.0), ..base }
            }
            SetupProfile::Call => ProfileTuning {
                compressor_ratio: 4.0,
                compressor_gr_db: 5.0,
                boost_above_7k: false,
                judge_hz: (100.0, 7000.0),
                tone_delta: Some(ToneDelta { lo_hz: 1000.0, hi_hz: 4000.0, db: 0.0 }),
                ..base
            },
        }
    }

    /// Does this finding matter for what the user is setting up?
    pub fn is_relevant(self, f: &Finding) -> bool {
        !self.irrelevant().hides(f)
    }

    /// Drop the findings that do not apply. Call before ranking and scoring so
    /// hidden items cost nothing.
    pub fn filter_findings(self, findings: &mut Vec<Finding>) {
        let irrelevant = self.irrelevant();
        findings.retain(|f| !irrelevant.hides(f));
    }

    pub const ALL: [SetupProfile; 3] = [SetupProfile::Streaming, SetupProfile::Voiceover, SetupProfile::Call];

    /// Stable id used in `setup.json` and the API (same as the serde name).
    pub fn id(self) -> &'static str {
        match self {
            SetupProfile::Streaming => "streaming",
            SetupProfile::Voiceover => "voiceover",
            SetupProfile::Call => "call",
        }
    }

    /// Plain name for the picker.
    pub fn label(self) -> &'static str {
        match self {
            SetupProfile::Streaming => "Streaming",
            SetupProfile::Voiceover => "Recording / voiceover",
            SetupProfile::Call => "Call / meeting",
        }
    }

    /// One plain sentence under the name in the picker.
    pub fn summary(self) -> &'static str {
        match self {
            SetupProfile::Streaming => "Sound clear and steady on stream, with the mic kept out of the camera's view.",
            SetupProfile::Voiceover => "The cleanest recording you can get: mic close, room and noise kept very quiet.",
            SetupProfile::Call => "Be easy to understand on calls and meetings, with as little fiddling as possible.",
        }
    }

    pub fn targets(self) -> ProfileTargets {
        let speech_peak = (-18.0, -12.0);
        let speech_peak_close = (-24.0, -10.0);
        let loud_peak = MaxBar { met: -6.0, close: -3.0, flag: -3.0, major: -0.3 };
        match self {
            SetupProfile::Streaming => ProfileTargets {
                speech_peak,
                speech_peak_close,
                loud_peak,
                noise_floor: MaxBar { met: -60.0, close: -50.0, flag: -50.0, major: -45.0 },
                snr: MinBar { met: 50.0, close: 40.0, major: 30.0 },
                room_decay: MaxBar { met: 0.35, close: 0.5, flag: 0.45, major: 0.7 },
            },
            SetupProfile::Voiceover => ProfileTargets {
                speech_peak,
                speech_peak_close,
                loud_peak,
                noise_floor: MaxBar { met: -60.0, close: -55.0, flag: -55.0, major: -50.0 },
                snr: MinBar { met: 50.0, close: 45.0, major: 40.0 },
                room_decay: MaxBar { met: 0.27, close: 0.35, flag: 0.35, major: 0.5 },
            },
            SetupProfile::Call => ProfileTargets {
                speech_peak,
                speech_peak_close,
                loud_peak,
                noise_floor: MaxBar { met: -50.0, close: -45.0, flag: -45.0, major: -40.0 },
                snr: MinBar { met: 25.0, close: 20.0, major: 15.0 },
                room_decay: MaxBar { met: 0.5, close: 0.65, flag: 0.65, major: 0.75 },
            },
        }
    }

    /// Ideal mouth-to-mic distance for this profile (cm), from the mic's
    /// profile range or the research-brief priors. Voiceover takes the closer
    /// half of the range ("closest good position"); a headset boom is always
    /// at the corner of the mouth.
    pub fn distance_cm(self, ctx: &MicContext) -> Option<(f64, f64)> {
        if ctx.connection == Some(Connection::Headset) && ctx.distance_cm.is_none() {
            return Some((1.0, 3.0));
        }
        let range = ctx.distance_cm.or(match ctx.capsule {
            Some(Capsule::Dynamic) => Some((3.0, 10.0)),
            Some(_) => Some((10.0, 20.0)),
            None => None,
        })?;
        Some(match self {
            SetupProfile::Voiceover => (range.0, (range.0 + (range.1 - range.0) / 2.0).round()),
            _ => range,
        })
    }

    /// Short distance words used inside findings and coaching ("3–10 cm").
    pub fn distance_text(self, ctx: &MicContext) -> String {
        match self.distance_cm(ctx) {
            Some((a, b)) => format!("{a:.0}–{b:.0} cm"),
            None => "about a fist away".into(),
        }
    }

    /// Where to put the mic, in plain words, for the physical-setup step.
    pub fn placement(self, ctx: &MicContext) -> String {
        let d = self.distance_text(ctx);
        if ctx.connection == Some(Connection::Headset) {
            return format!("Bend the headset mic to the corner of your mouth, {d} away, so your breath doesn't hit it.");
        }
        match self {
            SetupProfile::Streaming => format!("Put the mic {d} from your mouth, just below your chin or off to the side so it stays out of the camera's view, and point it at your mouth."),
            SetupProfile::Voiceover => format!("Bring the mic in close, {d} from your mouth: as close as it still sounds natural. Nobody sees it, so it can sit right in front of you. Use a pop filter and point it at your mouth."),
            SetupProfile::Call => format!("Put the mic {d} from your mouth, somewhere it can stay put while you talk, pointed at your mouth."),
        }
    }

    /// Everything the picker and the physical-setup step show for this profile
    /// and mic, in one serializable value (`SetupProfileInfo` in app/API.md).
    pub fn info(self, ctx: &MicContext) -> ProfileInfo {
        ProfileInfo {
            id: self,
            label: self.label().into(),
            summary: self.summary().into(),
            placement: self.placement(ctx),
            room_advice: self.room_advice().into(),
            distance_cm: self.distance_cm(ctx),
            targets: self.targets(),
            irrelevant: self.irrelevant(),
        }
    }

    /// Plain advice about the room and noise for this profile.
    pub fn room_advice(self) -> &'static str {
        match self {
            SetupProfile::Streaming => "Turn off or move noisy things you can, and add something soft to bare walls if your voice echoes.",
            SetupProfile::Voiceover => "Record in the quietest, softest spot you have: fans and the PC off or far away, curtains, a rug, a closet full of clothes works great.",
            SetupProfile::Call => "A normal quiet room is fine. Close the door and keep loud things away from the mic.",
        }
    }
}
