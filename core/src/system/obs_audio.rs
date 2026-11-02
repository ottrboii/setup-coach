//! OBS mic-source and OBS-app rules, plus filter helpers shared with actions.

use super::ids;
use super::state::{
    MonitoringType, ObsAudioInput, ObsFilter, SystemSnapshot, OBS_KIND_APP_AUDIO,
    OBS_KIND_DESKTOP, OBS_KIND_MIC,
};
use crate::finding::{Area, Finding, FixKind, Severity};
use serde_json::{json, Value};

// ---------------------------------------------------------------- filter helpers

/// Where a filter belongs in a clean voice chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Gain = 0,
    /// High-pass/EQ/cleanup (incl. our VST3, 3-band EQ, noise suppression).
    Tone = 1,
    /// Gate / expander.
    Gate = 2,
    Compressor = 3,
    DeEsser = 4,
    Limiter = 5,
}

/// Classify an OBS filter kind. `None` = not an audio-chain filter we order.
pub fn stage_of(kind: &str) -> Option<Stage> {
    let k = kind.to_ascii_lowercase();
    if k.contains("deess") || k.contains("de_ess") {
        return Some(Stage::DeEsser);
    }
    match k.as_str() {
        "gain_filter" | "invert_polarity_filter" => Some(Stage::Gain),
        "basic_eq_filter" | "nvidia_audiofx_filter" => Some(Stage::Tone),
        "noise_gate_filter" | "expander_filter" => Some(Stage::Gate),
        "compressor_filter" | "upward_compressor_filter" => Some(Stage::Compressor),
        "limiter_filter" => Some(Stage::Limiter),
        _ if k.starts_with("noise_suppress_filter") || k.contains("vst") => Some(Stage::Tone),
        _ => None,
    }
}

/// Number from filter settings, with OBS's default when unset.
pub fn num(settings: &Value, key: &str, default: f64) -> f64 {
    settings.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn enabled<'a>(input: &'a ObsAudioInput, kind: &'a str) -> impl Iterator<Item = &'a ObsFilter> + 'a {
    input.filters.iter().filter(move |f| f.enabled && f.kind == kind)
}

/// Whether a noise-suppression filter uses an AI (neural) method.
pub fn is_ai_suppressor(f: &ObsFilter) -> bool {
    if f.kind == "nvidia_audiofx_filter" {
        return true;
    }
    if !f.kind.starts_with("noise_suppress_filter") {
        return false;
    }
    match f.settings.get("method").and_then(Value::as_str) {
        Some(m) => !m.eq_ignore_ascii_case("speex"),
        // v2 defaults to RNNoise; the legacy filter is Speex-only.
        None => f.kind.ends_with("_v2"),
    }
}

/// Filter names in the recommended order (stable). Filters we don't classify
/// travel with the filter in front of them.
pub fn recommended_order(input: &ObsAudioInput) -> Vec<String> {
    let mut last = 0u8;
    let mut keyed: Vec<(u8, usize, &str)> = input
        .filters
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let s = stage_of(&f.kind).map(|s| s as u8).unwrap_or(last);
            last = s;
            (s, i, f.name.as_str())
        })
        .collect();
    keyed.sort_by_key(|(s, i, _)| (*s, *i));
    keyed.into_iter().map(|(_, _, n)| n.to_string()).collect()
}

fn order_problem(input: &ObsAudioInput) -> Option<bool> {
    // Returns Some(limiter_not_last) if the enabled, classified filters are out of order.
    let stages: Vec<Stage> = input
        .filters
        .iter()
        .filter(|f| f.enabled)
        .filter_map(|f| stage_of(&f.kind))
        .collect();
    let out_of_order = stages.windows(2).any(|w| w[0] > w[1]);
    if !out_of_order {
        return None;
    }
    let limiter_not_last = stages
        .iter()
        .position(|s| *s == Stage::Limiter)
        .is_some_and(|p| p + 1 != stages.len());
    Some(limiter_not_last)
}

/// Gate settings that open below your quietest speech and close above the noise.
pub fn recommended_gate(noise_floor_dbfs: Option<f32>, quiet_speech_dbfs: Option<f32>) -> Value {
    let q = quiet_speech_dbfs.map(f64::from).unwrap_or(-40.0);
    let floor = noise_floor_dbfs.map(f64::from).unwrap_or(q - 20.0);
    let close = floor + 6.0;
    let open = ((close + q) / 2.0).max(close + 3.0).min(q - 3.0);
    json!({
        "open_threshold": round1(open),
        "close_threshold": round1(close.min(open - 2.0)),
        "attack_time": 5,
        "hold_time": 200,
        "release_time": 150
    })
}

/// Expander threshold halfway between noise and quiet speech.
pub fn recommended_expander(noise_floor_dbfs: Option<f32>, quiet_speech_dbfs: Option<f32>, existing: &Value) -> Value {
    let q = quiet_speech_dbfs.map(f64::from).unwrap_or(-40.0);
    let threshold = match noise_floor_dbfs {
        Some(f) => (f as f64 + q) / 2.0,
        None => q - 10.0,
    };
    merge(existing, json!({ "threshold": round1(threshold), "ratio": 3.0, "attack_time": 5, "release_time": 150 }))
}

pub fn recommended_compressor(existing: &Value) -> Value {
    let t = num(existing, "threshold", -18.0);
    let threshold = if (-30.0..=-10.0).contains(&t) { t } else { -20.0 };
    merge(existing, json!({ "ratio": 3.5, "threshold": threshold, "attack_time": 6, "release_time": 100 }))
}

pub fn recommended_limiter() -> Value {
    json!({ "threshold": -1.5, "release_time": 60 })
}

fn merge(existing: &Value, new: Value) -> Value {
    let mut out = existing.as_object().cloned().unwrap_or_default();
    if let Value::Object(n) = new {
        for (k, v) in n {
            out.insert(k, v);
        }
    }
    Value::Object(out)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// Device names that are an already-mixed stream bus (Wave Link, GoXLR, Sonar).
pub fn is_stream_mix_device(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["stream mix", "wave link stream", "broadcast stream", "sonar - stream", "sonar stream"]
        .iter()
        .any(|m| n.contains(m))
}

// ---------------------------------------------------------------- rules

pub(crate) fn check(s: &SystemSnapshot, out: &mut Vec<Finding>) {
    let Some(obs) = &s.obs else { return };

    if !obs.websocket_enabled {
        out.push(Finding::new(
            ids::WEBSOCKET_OFF,
            Area::System,
            Severity::Major,
            "Setup Coach can't talk to OBS yet",
            "OBS's built-in remote control (WebSocket server) is switched off, so we can't read or apply your settings.",
            "Turn on the OBS WebSocket server. We can switch it on for you while OBS is closed.",
            FixKind::Auto,
        ));
    }
    if obs.major_version().is_some_and(|v| v < 30) {
        out.push(Finding::new(
            ids::OBS_OLD,
            Area::System,
            Severity::Minor,
            "Your OBS is out of date",
            "Older OBS versions miss crash-safe recording, newer encoders and audio fixes.",
            "Update OBS from obsproject.com (your scenes and settings are kept).",
            FixKind::Guided,
        ));
    }

    let Some(mic) = obs.main_mic() else { return };
    let ctx = &s.mic;

    if mic.muted {
        out.push(Finding::new(
            ids::MIC_MUTED,
            Area::Mic,
            Severity::Major,
            "Your mic is muted in OBS",
            "OBS has your mic source muted, so viewers can't hear you.",
            "Unmute the mic in OBS's audio mixer. We can unmute it for you.",
            FixKind::Auto,
        ));
    }

    if mic.device_id == "default" || mic.device_id.is_empty() {
        let resolvable = s.windows.as_ref().and_then(|w| w.default_capture()).is_some();
        out.push(Finding::new(
            ids::OBS_DEFAULT_DEVICE,
            Area::Mic,
            Severity::Minor,
            "OBS follows whatever Windows calls the default mic",
            "Your OBS mic is set to \"Default\". When Windows switches default (new headset, webcam, update), OBS quietly switches to the wrong mic.",
            "Pin OBS to your actual mic. We can do it for you.",
            if resolvable { FixKind::Auto } else { FixKind::Guided },
        ));
    }

    // Mono / left channel only
    let endpoint_channels = s.mic_endpoint().map(|e| e.channels).unwrap_or(2);
    if ctx.single_live_channel && endpoint_channels >= 2 && !mic.mono_downmix {
        out.push(Finding::new(
            ids::NOT_MONO,
            Area::Mic,
            Severity::Critical,
            "Viewers only hear you in their left ear",
            "Your interface sends your mic on input 1 as the left half of a stereo signal, so OBS puts your voice in the left ear only.",
            "Turn on \"Mono\" for your mic in OBS (Advanced Audio Properties). We can do it for you.",
            FixKind::Auto,
        ));
    }

    // Monitoring
    if mic.monitoring != MonitoringType::Off {
        if ctx.direct_monitoring_on == Some(true) {
            out.push(Finding::new(
                ids::MONITORING_DOUBLED,
                Area::Mic,
                Severity::Major,
                "You hear yourself twice, slightly out of sync",
                "Your interface already plays your mic back with zero delay, and OBS plays it again a moment later. It sounds phasey and distracting.",
                "Keep the interface's zero-latency monitoring and turn OBS monitoring off for the mic. We can do it for you.",
                FixKind::Auto,
            ));
        } else {
            out.push(Finding::new(
                ids::MONITORING_DELAYED,
                Area::Mic,
                Severity::Minor,
                "You hear yourself slightly late",
                "OBS monitoring plays your voice back after processing, with a small delay that makes it hard to talk naturally.",
                "Turn OBS monitoring off for the mic, or use your interface or headset's zero-latency monitoring instead.",
                FixKind::Auto,
            ));
        }
    }

    // AI / DSP noise suppression
    for f in mic.filters.iter().filter(|f| f.enabled) {
        if is_ai_suppressor(f) {
            out.push(
                Finding::new(
                    ids::AI_SUPPRESSION,
                    Area::Mic,
                    Severity::Major,
                    "AI noise removal is making you sound robotic",
                    "OBS's RNNoise/NVIDIA noise suppression guesses what your voice is and can leave it warbly, underwater or chopped. It hides problems instead of fixing them.",
                    "Remove the AI noise suppression. We'll fix the noise at the source and use a gentle expander instead.",
                    FixKind::Auto,
                )
                .with_evidence("filter_index", mic.filters.iter().position(|x| x.name == f.name).unwrap_or(0) as f64),
            );
            break;
        }
    }
    if mic.filters.iter().any(|f| f.enabled && f.kind.starts_with("noise_suppress_filter") && !is_ai_suppressor(f)) {
        out.push(Finding::new(
            ids::SPEEX_SUPPRESSION,
            Area::Mic,
            Severity::Minor,
            "Noise suppression is softening your voice",
            "Speex noise suppression is plain DSP, but it still dulls your voice a little. Once the noise is fixed at the source it usually isn't needed.",
            "Remove it after the physical fixes, or keep it very light (around -10 dB).",
            FixKind::Auto,
        ));
    }

    // Filter order
    if let Some(limiter_not_last) = order_problem(mic) {
        out.push(Finding::new(
            ids::FILTER_ORDER,
            Area::Mic,
            if limiter_not_last { Severity::Major } else { Severity::Minor },
            "Your OBS mic filters are in the wrong order",
            "Filters work top to bottom. In the wrong order a gate reacts to boosted noise, or the limiter can't catch the peaks.",
            "Reorder to: gain → EQ → gate/expander → compressor → de-esser → limiter. We can reorder them for you.",
            FixKind::Auto,
        ));
    }

    // Gate
    let floor = ctx.measured_noise_floor_dbfs;
    let quiet = ctx.measured_quiet_speech_dbfs;
    let low_snr = matches!((floor, quiet), (Some(f), Some(q)) if q - f < 10.0);
    if low_snr {
        out.push(
            Finding::new(
                ids::LOW_SNR_FOR_GATE,
                Area::Mic,
                Severity::Major,
                "Background noise is almost as loud as your quiet voice",
                "Your quiet speech is only a few dB above the room noise, so no gate or expander can remove the noise without cutting words.",
                "Move the mic closer to your mouth and away from the noise (PC fan, keyboard), then lower the gain.",
                FixKind::Manual,
            )
            .with_evidence("noise_floor_dbfs", floor.unwrap_or_default() as f64)
            .with_evidence("quiet_speech_dbfs", quiet.unwrap_or_default() as f64),
        );
    }
    // Only the first enabled gate matters.
    if let Some(g) = enabled(mic, "noise_gate_filter").next() {
        let open = num(&g.settings, "open_threshold", -26.0);
        let attack = num(&g.settings, "attack_time", 25.0);
        let hold = num(&g.settings, "hold_time", 200.0);
        let chops = quiet.is_some_and(|q| open > q as f64 - 3.0);
        if chops && !low_snr {
            out.push(
                Finding::new(
                    ids::GATE_CHOPS,
                    Area::Mic,
                    Severity::Major,
                    "Your noise gate cuts off quiet words",
                    "The gate only opens for speech louder than your quietest words, so the start and end of sentences disappear.",
                    "Lower the gate's open level below your quiet speech and close it just above the room noise. We can set it from your test.",
                    FixKind::Auto,
                )
                .with_evidence("gate_open_dbfs", open)
                .with_evidence("quiet_speech_dbfs", quiet.unwrap_or_default() as f64),
            );
        } else if attack > 10.0 || hold < 100.0 {
            out.push(
                Finding::new(
                    ids::GATE_SLOW,
                    Area::Mic,
                    Severity::Minor,
                    "Your noise gate clips the first or last sound of words",
                    "A slow gate attack swallows the start of words, and a short hold chops word endings.",
                    "Use a 5 ms attack and around 200 ms hold. We can set it for you.",
                    FixKind::Auto,
                )
                .with_evidence("attack_ms", attack)
                .with_evidence("hold_ms", hold),
            );
        }
    }
    if let Some(e) = enabled(mic, "expander_filter").next() {
        let t = num(&e.settings, "threshold", -40.0);
        if !low_snr && quiet.is_some_and(|q| t > q as f64 - 3.0) {
            out.push(
                Finding::new(
                    ids::EXPANDER_CHOPS,
                    Area::Mic,
                    Severity::Major,
                    "Your expander turns down your quiet words",
                    "The expander's threshold sits above your quiet speech, so soft words get pushed down with the noise.",
                    "Set the threshold halfway between room noise and your quiet speech. We can set it from your test.",
                    FixKind::Auto,
                )
                .with_evidence("expander_threshold_dbfs", t),
            );
        }
    }

    // Compressor
    if let Some(c) = enabled(mic, "compressor_filter").next() {
        let ratio = num(&c.settings, "ratio", 10.0);
        let threshold = num(&c.settings, "threshold", -18.0);
        let release = num(&c.settings, "release_time", 60.0);
        if ratio >= 8.0 || threshold <= -35.0 || release < 30.0 {
            out.push(
                Finding::new(
                    ids::COMPRESSOR_EXTREME,
                    Area::Mic,
                    Severity::Minor,
                    "Your compressor is squashing your voice",
                    "A very high ratio, very low threshold or very fast release makes your voice flat and pulls background noise up between sentences (\"pumping\").",
                    "Use a gentle setting: ratio 3.5:1, threshold so normal speech is reduced by 3-6 dB, 6 ms attack, 100 ms release. We can set it for you.",
                    FixKind::Auto,
                )
                .with_evidence("ratio", ratio)
                .with_evidence("threshold_db", threshold)
                .with_evidence("release_ms", release),
            );
        }
    }

    // Limiter
    match enabled(mic, "limiter_filter").next() {
        None => out.push(Finding::new(
            ids::LIMITER_MISSING,
            Area::Mic,
            Severity::Major,
            "Nothing stops your laughs and shouts from distorting",
            "Without a limiter at the end of the chain, loud moments clip and crackle for viewers.",
            "Add a limiter as the last mic filter (-1.5 dB). We can add it for you.",
            FixKind::Auto,
        )),
        Some(l) => {
            let t = num(&l.settings, "threshold", -6.0);
            if !(-6.0..=-0.5).contains(&t) {
                out.push(
                    Finding::new(
                        ids::LIMITER_LEVEL,
                        Area::Mic,
                        Severity::Minor,
                        if t < -6.0 { "Your limiter is flattening your voice" } else { "Your limiter leaves no safety margin" },
                        "A limiter set too low squashes every loud word; set right at 0 dB it lets peaks distort after streaming encoding.",
                        "Set the limiter to -1.5 dB. We can set it for you.",
                        FixKind::Auto,
                    )
                    .with_evidence("limiter_threshold_db", t),
                );
            }
        }
    }

    // Gain stacking
    let boost = s
        .mic_endpoint()
        .filter(|e| !e.is_onboard)
        .and_then(|e| e.mic_boost_db)
        .unwrap_or(0.0)
        .max(0.0) as f64;
    let filter_gain: f64 = enabled(mic, "gain_filter").map(|g| num(&g.settings, "db", 0.0).max(0.0)).sum();
    let fader = (mic.volume_db as f64).max(0.0);
    let stages = [boost, filter_gain, fader].iter().filter(|g| **g > 0.5).count();
    let total = boost + filter_gain + fader;
    if stages >= 2 && total > 10.0 {
        out.push(
            Finding::new(
                ids::GAIN_STACKED,
                Area::Mic,
                if total > 20.0 { Severity::Major } else { Severity::Minor },
                "Your mic is turned up in several places, adding hiss",
                "Extra digital boost in Windows and OBS turns up noise as much as your voice. Clean gain only comes from the mic or interface.",
                "Turn up the gain on your mic or interface, then remove the extra Windows/OBS boosts. If the interface is already maxed, your mic needs more clean gain (see suggestion).",
                FixKind::Guided,
            )
            .with_evidence("digital_gain_db", total)
            .suggesting("inline_booster"),
        );
    }

    // Double capture: same app via desktop audio and app capture
    let desktop_live = obs.audio_inputs.iter().any(|i| i.kind == OBS_KIND_DESKTOP && !i.muted);
    if desktop_live {
        for app in obs.audio_inputs.iter().filter(|i| i.kind == OBS_KIND_APP_AUDIO && !i.muted) {
            let target = app.app_target.clone().unwrap_or_else(|| app.name.clone());
            out.push(Finding::new(
                ids::DOUBLE_CAPTURE_APP,
                Area::Mic,
                Severity::Major,
                &format!("Viewers probably hear {} twice", pretty_app(&target)),
                "That app is captured on its own and again inside \"Desktop Audio\", which causes echoes and makes it too loud.",
                "Capture each app in one place only: either exclude it from Desktop Audio (send it to another output) or remove its separate capture.",
                FixKind::Guided,
            ));
        }
    }

    // Double capture: raw mic + an already-mixed stream bus
    let raw_mics: Vec<&ObsAudioInput> = obs
        .audio_inputs
        .iter()
        .filter(|i| i.kind == OBS_KIND_MIC && !i.muted && !is_stream_mix_device(&i.device_name))
        .collect();
    let mixes: Vec<&ObsAudioInput> = obs
        .audio_inputs
        .iter()
        .filter(|i| !i.muted && is_stream_mix_device(&i.device_name))
        .collect();
    if !raw_mics.is_empty() && !mixes.is_empty() {
        out.push(Finding::new(
            ids::DOUBLE_CAPTURE_MIX,
            Area::Mic,
            Severity::Major,
            "Your voice reaches the stream twice",
            "OBS captures your mic directly and also through your mixer's \"Stream Mix\", which already contains your mic. It sounds phasey and too loud.",
            "Keep the Stream Mix in OBS and remove (or mute) the direct mic capture.",
            FixKind::Guided,
        ));
    }

    // Processing apps
    let ai = s.processing.ai_active();
    if !ai.is_empty() {
        out.push(Finding::new(
            ids::AI_APP_PROCESSING,
            Area::Mic,
            Severity::Major,
            &format!("{} is reprocessing your voice with AI", ai.join(" and ")),
            "AI voice cleanup hides noise and echo instead of fixing them, and can leave your voice robotic or warbly.",
            "Turn its mic effects off. We'll fix the cause and use plain, transparent settings instead.",
            FixKind::Guided,
        ));
    }
    let dsp = s.processing.dsp_active();
    let obs_processing = mic
        .filters
        .iter()
        .any(|f| f.enabled && matches!(stage_of(&f.kind), Some(Stage::Tone | Stage::Gate | Stage::Compressor)));
    if !dsp.is_empty() && obs_processing {
        out.push(Finding::new(
            ids::VENDOR_DSP_STACKED,
            Area::Mic,
            Severity::Minor,
            "Your voice is processed in two places",
            &format!(
                "{} already processes your mic, and OBS filters process it again. Settings fight each other and are hard to predict.",
                dsp.join(" and ")
            ),
            "Process in one place: turn off the mixer app's mic effects and let Setup Coach run the OBS chain (or the other way round).",
            FixKind::Guided,
        ));
    }
    let auto_level = ctx
        .onboard_dsp_active
        .iter()
        .any(|d| d.contains("auto_level") || d.contains("auto_gain"));
    if auto_level && enabled(mic, "compressor_filter").next().is_some() {
        out.push(Finding::new(
            ids::ONBOARD_DSP_FIGHTS,
            Area::Mic,
            Severity::Minor,
            "Your mic's auto-level fights your compressor",
            "Your mic levels itself automatically, and the OBS compressor works on top of it, so loudness pumps.",
            "Turn off the mic's Auto Level / Auto Gain in its app (e.g. Shure MOTIV, G HUB) once we've set a fixed gain.",
            FixKind::Guided,
        ));
    }
}

fn pretty_app(exe: &str) -> String {
    let base = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
    if base.is_empty() { "that app".into() } else { base.to_string() }
}

/// Is this an OBS mic-type input (used by actions).
pub fn is_mic_kind(i: &ObsAudioInput) -> bool {
    i.kind == OBS_KIND_MIC
}
