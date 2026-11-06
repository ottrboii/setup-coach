//! Capture-card setup read from OBS: does the camera's frame rate family
//! match the stream's, and is the card's own (HDMI) sound running at the same
//! time as a separate mic.
//!
//! Only what OBS reports is used (source settings, inputs), no frames. The
//! card's real HDMI rate is only known when the source is set to a fixed
//! mode, so "Device Default" sources are never accused.

use super::connection::capture_card_name;
use super::ids;
use super::state::{ObsState, MonitoringType, SystemSnapshot, OBS_KIND_MIC};
use crate::finding::{Area, Finding, FixKind, Severity};

/// Frame-rate family of a rate: 50 Hz (25/50) or 60 Hz (24/30/60 and the
/// 1.001 variants). `None` for anything else (e.g. 144).
fn family(fps: f32) -> Option<Hz> {
    let near = |t: f32| (fps - t).abs() < 0.6;
    if near(25.0) || near(50.0) {
        Some(Hz::Fifty)
    } else if near(24.0) || near(30.0) || near(60.0) {
        Some(Hz::Sixty)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hz {
    Fifty,
    Sixty,
}

pub(crate) fn check(s: &SystemSnapshot, out: &mut Vec<Finding>) {
    let Some(obs) = &s.obs else { return };
    fps_family(obs, out);
    audio_twice(obs, out);
}

/// A 25/50 fps camera in a 30/60 fps stream (or the other way round) makes
/// OBS repeat or drop a frame every fraction of a second: steady judder.
fn fps_family(obs: &ObsState, out: &mut Vec<Finding>) {
    let Some(enc) = &obs.encoder else { return };
    let Some(out_fam) = family(enc.fps) else { return };
    for c in obs.video_captures.iter().filter(|c| c.res_type_custom && (capture_card_name(&c.device_name) || capture_card_name(&c.name))) {
        let Some(src) = c.fps else { continue };
        let Some(src_fam) = family(src) else { continue };
        if src_fam == out_fam {
            continue;
        }
        let (cam_side, tip) = match src_fam {
            Hz::Fifty => ("50", "In the camera's menu look for the video system or NTSC/PAL setting (and the HDMI output rate) and switch it to NTSC / 60 Hz."),
            Hz::Sixty => ("60", "In the camera's menu look for the video system or NTSC/PAL setting (and the HDMI output rate) and switch it to PAL / 50 Hz."),
        };
        let mut f = Finding::new(
            ids::CAM_FPS_FAMILY,
            Area::Camera,
            Severity::Major,
            &format!("\"{}\" and your stream run at different frame rates", c.name),
            &format!("The capture card is set to a {cam_side} Hz style frame rate but your stream uses the other family, so OBS has to repeat or skip frames and motion looks jerky even though nothing is wrong with the PC."),
            &format!("{tip} Then make the OBS source and the stream use the same rate (the OBS stream rate is under Settings, Video). We'll check again once you changed it."),
            FixKind::Guided,
        );
        f.can_decline = true;
        f.evidence.insert("source_fps".into(), f64::from(src));
        f.evidence.insert("stream_fps".into(), f64::from(enc.fps));
        out.push(f);
    }
}

/// The capture card's own sound is in the mix while a separate mic is live:
/// two clocks, so the voice can echo or slowly slide out of sync.
fn audio_twice(obs: &ObsState, out: &mut Vec<Finding>) {
    let mut card: Option<(String, &'static str)> = None;
    for c in obs.video_captures.iter().filter(|c| capture_card_name(&c.device_name) || capture_card_name(&c.name)) {
        if c.own_audio || capture_card_name(&c.audio_device) {
            card = Some((c.name.clone(), "source"));
            break;
        }
    }
    if card.is_none() {
        card = obs
            .audio_inputs
            .iter()
            .find(|i| i.kind == OBS_KIND_MIC && !i.muted && capture_card_name(&i.device_name))
            .map(|i| (i.name.clone(), "input"));
    }
    let Some((card_name, kind)) = card else { return };
    let mic = obs.audio_inputs.iter().find(|i| i.kind == OBS_KIND_MIC && !i.muted && !capture_card_name(&i.device_name));
    let Some(mic) = mic else { return };
    let how = if kind == "source" {
        "In OBS, right-click the capture card source, choose Properties, and set \"Audio Output Mode\" to \"Capture audio only\" (and untick the custom audio device).".to_string()
    } else {
        format!("In OBS, mute \"{card_name}\" in the mixer (or remove it).")
    };
    let mut f = Finding::new(
        ids::MIC_CAPTURE_AUDIO_TWICE,
        Area::Mic,
        Severity::Major,
        "Your capture card's sound is on at the same time as your mic",
        &format!("OBS is taking sound from \"{card_name}\" and from \"{}\". If the card is fed by a camera, that is the camera's own tiny mic, so viewers hear you twice with a slight echo, and the two sources slowly drift apart because they run on different clocks.", mic.name),
        &format!("If the card carries your camera, turn the card's sound off. {how} If it carries the game sound from another PC, keep it and tell us (\"I can't do this\"); then make sure the camera isn't also feeding sound into it. We'll check again once you changed it."),
        FixKind::Guided,
    );
    f.can_decline = true;
    f.evidence.insert("mic_monitored".into(), if mic.monitoring == MonitoringType::Off { 0.0 } else { 1.0 });
    out.push(f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::state::*;
    use crate::system::audit;

    fn snap(cap: ObsVideoCapture, out_fps: f32, inputs: Vec<ObsAudioInput>) -> SystemSnapshot {
        SystemSnapshot {
            obs: Some(ObsState {
                video_captures: vec![cap],
                audio_inputs: inputs,
                encoder: Some(ObsEncoder { fps: out_fps, ..Default::default() }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    fn link(fps: Option<f32>) -> ObsVideoCapture {
        ObsVideoCapture { name: "Cam Link".into(), device_name: "Cam Link 4K".into(), res_type_custom: true, fps, width: 1920, height: 1080, video_format: "NV12".into(), ..Default::default() }
    }
    fn mic(name: &str, device: &str, muted: bool) -> ObsAudioInput {
        ObsAudioInput { name: name.into(), kind: OBS_KIND_MIC.into(), device_name: device.into(), muted, ..Default::default() }
    }
    fn ids_of(s: &SystemSnapshot) -> Vec<String> {
        audit(s, &[]).into_iter().map(|f| f.id).filter(|i| i == ids::CAM_FPS_FAMILY || i == ids::MIC_CAPTURE_AUDIO_TWICE).collect()
    }

    #[test]
    fn pal_camera_in_a_60_fps_stream_is_flagged() {
        let f = audit(&snap(link(Some(50.0)), 60.0, vec![]), &[]);
        let f = f.iter().find(|f| f.id == ids::CAM_FPS_FAMILY).expect("mismatch found");
        assert_eq!((f.evidence["source_fps"], f.evidence["stream_fps"]), (50.0, 60.0));
        assert!(!f.title.chars().any(|c| c.is_ascii_digit()) && f.fix.contains("NTSC"));
        // The other way round names PAL as the camera fix.
        let f = audit(&snap(link(Some(59.94)), 25.0, vec![]), &[]);
        assert!(f.iter().find(|f| f.id == ids::CAM_FPS_FAMILY).unwrap().fix.contains("PAL"));
    }

    #[test]
    fn matching_unknown_or_non_card_sources_are_quiet() {
        assert!(ids_of(&snap(link(Some(60.0)), 60.0, vec![])).is_empty());
        assert!(ids_of(&snap(link(Some(29.97)), 60.0, vec![])).is_empty(), "30 and 60 are the same family");
        assert!(ids_of(&snap(link(None), 60.0, vec![])).is_empty(), "device default says nothing");
        let mut auto = link(Some(50.0));
        auto.res_type_custom = false;
        assert!(ids_of(&snap(auto, 60.0, vec![])).is_empty());
        let mut webcam = link(Some(50.0));
        webcam.name = "C922".into();
        webcam.device_name = "C922 Pro Stream Webcam".into();
        assert!(ids_of(&snap(webcam, 60.0, vec![])).is_empty());
    }

    #[test]
    fn card_sound_plus_separate_mic_is_flagged_and_clears() {
        let mut c = link(Some(60.0));
        c.own_audio = true;
        let s = snap(c.clone(), 60.0, vec![mic("Mic/Aux", "GoXLR Chat Mic", false)]);
        let f = audit(&s, &[]);
        let f = f.iter().find(|f| f.id == ids::MIC_CAPTURE_AUDIO_TWICE).unwrap();
        assert_eq!(f.fix_kind, FixKind::Guided);
        assert!(f.can_decline && f.fix.contains("Capture audio only"));
        // Fixed (audio capture only): next scan is quiet.
        c.own_audio = false;
        assert!(ids_of(&snap(c.clone(), 60.0, vec![mic("Mic/Aux", "GoXLR Chat Mic", false)])).is_empty());
        // No separate mic, or mic muted: nothing to double.
        c.own_audio = true;
        assert!(ids_of(&snap(c.clone(), 60.0, vec![])).is_empty());
        assert!(ids_of(&snap(c, 60.0, vec![mic("Mic/Aux", "GoXLR Chat Mic", true)])).is_empty());
    }

    #[test]
    fn card_endpoint_captured_as_an_input_counts() {
        let inputs = vec![mic("HDMI audio", "Digital Audio Interface (Cam Link 4K)", false), mic("Mic/Aux", "Microphone (RE20)", false)];
        let f = audit(&snap(link(Some(60.0)), 60.0, inputs.clone()), &[]);
        assert!(f.iter().find(|f| f.id == ids::MIC_CAPTURE_AUDIO_TWICE).unwrap().fix.contains("mute \"HDMI audio\""));
        // The card as the only mic is just a card, not a double.
        assert!(ids_of(&snap(link(Some(60.0)), 60.0, inputs[..1].to_vec())).is_empty());
    }
}
