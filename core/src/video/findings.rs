//! Camera findings in plain language. Physical fixes come first; settings
//! second. Thresholds follow the research brief (section 5) and are starting
//! values to tune against real recordings.

use super::metrics::{ColourCast, RangeHint, VideoMetrics};
use super::types::{CameraClass, CameraState, MainsHz, VideoFormat};
use crate::finding::{Area, Finding, FixKind, Severity};

/// Paper (white sheet) target band, 8-bit luma.
pub const PAPER_TARGET_LO: f64 = 215.0;
pub const PAPER_TARGET_HI: f64 = 235.0;

fn f(id: &str, sev: Severity, title: &str, why: &str, fix: &str, kind: FixKind) -> Finding {
    Finding::new(id, Area::Camera, sev, title, why, fix, kind)
}

/// Pick fix text and kind by camera class. For webcams the app applies the
/// setting; for capture-card cameras and phones the user changes it in the
/// camera's or phone app's own menus.
fn by_class(class: CameraClass, webcam_fix: &str, camera_menu_fix: &str, phone_fix: &str) -> (String, FixKind) {
    match class {
        CameraClass::FullUvc | CameraClass::LimitedUvc => (webcam_fix.to_string(), FixKind::Auto),
        CameraClass::CaptureCard => (camera_menu_fix.to_string(), FixKind::Manual),
        CameraClass::PhoneOrVirtual => (phone_fix.to_string(), FixKind::Guided),
    }
}

fn frame_time_log2(fps: f64) -> f64 {
    (1.0 / fps.max(1.0)).log2()
}

/// Is the camera likely starved of light (rather than simply set too dark)?
fn light_limited(m: &VideoMetrics, s: &CameraState) -> bool {
    let gain_high = match (s.gain, s.gain_range) {
        (Some(g), Some(r)) => r.fraction(g) > 0.6,
        _ => false,
    };
    let shutter_maxed = s.exposure_log2.map(|e| e >= frame_time_log2(s.requested_fps) - 0.1).unwrap_or(false);
    let fps_sag = m.delivered_fps.map(|d| d < s.requested_fps * 0.9).unwrap_or(false);
    let noisy = m.noise_sigma.map(|n| n > 5.0).unwrap_or(false);
    gain_high || shutter_maxed || noisy || (fps_sag && (s.ae_priority || s.auto_exposure))
}

/// Build all camera findings for one test.
pub fn findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, mains: MainsHz) -> Vec<Finding> {
    let mut out = Vec::new();
    if m.frames == 0 {
        out.push(f(
            "camera.no_video",
            Severity::Critical,
            "We couldn't get a picture from your camera",
            "No frames arrived during the test. Another app may be using the camera, or Windows camera access is off.",
            "Close other apps that use the camera (Discord, browser, Teams, the camera's own app), check Windows camera privacy settings, then try again.",
            FixKind::Guided,
        ));
        return out;
    }
    exposure_findings(m, s, class, &mut out);
    colour_findings(m, s, class, &mut out);
    flicker_findings(m, s, class, mains, &mut out);
    focus_findings(m, s, class, &mut out);
    timing_findings(m, s, class, &mut out);
    lighting_shape_findings(m, &mut out);
    framing_findings(m, &mut out);
    range_findings(m, &mut out);
    if class.controllable() {
        state_findings(m, s, &mut out);
    }
    if !m.paper_found {
        out.push(f(
            "camera.no_paper",
            Severity::Info,
            "Hold up a white sheet of paper for the best result",
            "A white sheet next to your face tells us how much light is really on you and what true white looks like, for any skin tone.",
            "Run the paper step again holding a plain white sheet next to your cheek.",
            FixKind::Guided,
        ));
    }
    out
}

fn exposure_findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, out: &mut Vec<Finding>) {
    // Backlit: background lit more strongly than you (measured against the paper).
    let backlit = match (m.background_vs_paper_stops, m.face_vs_background_stops) {
        (Some(bp), _) => (bp > 0.0).then_some((Severity::Major, bp)),
        // No paper: we can't separate skin tone from light, so only flag the
        // extreme case, softly.
        (None, Some(fb)) => (fb < -2.5).then_some((Severity::Minor, -fb)),
        _ => None,
    };
    if let Some((sev, v)) = backlit {
        out.push(
            f(
                "camera.backlit",
                sev,
                "You're darker than the wall behind you",
                "Something behind you (usually a window or lamp) is brighter than the light on your face, so the camera exposes for the background.",
                "Turn so the window is in front of you or beside you, close the blinds, or add a light in front of you. Then we'll lock the exposure on you.",
                FixKind::Manual,
            )
            .with_evidence("background_vs_light_on_you_stops", v)
            .suggesting("key_light"),
        );
    } else if let Some(bp) = m.background_vs_paper_stops {
        if bp > -0.3 {
            out.push(
                f(
                    "camera.background_competes",
                    Severity::Minor,
                    "Your background is as bright as you are",
                    "Viewers' eyes go to the brightest part of the picture. It should be you.",
                    "Dim or move the light behind you, or move a little further from the wall.",
                    FixKind::Manual,
                )
                .with_evidence("background_vs_light_on_you_stops", bp),
            );
        }
    }

    // Paper exposure: too dark / too bright overall.
    if let Some(p) = m.paper_luma {
        if p < PAPER_TARGET_LO - 15.0 {
            if light_limited(m, s) {
                out.push(
                    f(
                        "camera.too_dark_grainy",
                        if p < 150.0 { Severity::Major } else { Severity::Minor },
                        "Your room is too dark for your camera",
                        "To make up for missing light the camera boosts gain (grain) or slows down (choppy, blurry motion).",
                        "Add light in front of you: face a window, or bounce a lamp off the wall behind your monitor. Then we'll lower the gain.",
                        FixKind::Manual,
                    )
                    .with_evidence("paper_luma", p)
                    .suggesting("key_light"),
                );
            } else {
                let (fix, kind) = by_class(
                    class,
                    "We'll lock a brighter exposure measured on your white paper.",
                    "In the camera menu, slow the shutter to the flicker-safe value or raise ISO a little until the paper looks white, not grey.",
                    "In the phone camera app, raise exposure until the paper looks white, then lock it.",
                );
                out.push(
                    f("camera.underexposed", Severity::Minor, "Your picture is a bit dark", "The exposure is set lower than your light allows.", &fix, kind)
                        .with_evidence("paper_luma", p),
                );
            }
        } else if p > PAPER_TARGET_HI + 8.0 || m.paper_clipped_pct.unwrap_or(0.0) > 5.0 {
            let (fix, kind) = by_class(
                class,
                "We'll lock a darker exposure so bright areas keep their detail.",
                "In the camera menu, use a faster flicker-safe shutter or lower ISO until the paper isn't pure white.",
                "In the phone camera app, lower exposure until the paper isn't pure white, then lock it.",
            );
            out.push(
                f(
                    "camera.overexposed",
                    Severity::Major,
                    "Your picture is too bright",
                    "Bright areas are turning pure white and losing detail.",
                    &fix,
                    kind,
                )
                .with_evidence("paper_luma", p),
            );
        }
    } else if m.noise_sigma.map(|n| n > 5.0).unwrap_or(false) && light_limited(m, s) {
        out.push(
            f(
                "camera.too_dark_grainy",
                Severity::Major,
                "Your room is too dark for your camera",
                "The picture is grainy because the camera is boosting gain to make up for missing light.",
                "Add light in front of you: face a window, or bounce a lamp off the wall behind your monitor.",
                FixKind::Manual,
            )
            .with_evidence("noise_sigma", m.noise_sigma.unwrap_or(0.0))
            .suggesting("key_light"),
        );
    }

    // Face blown out (shine) — independent of skin tone.
    if let Some(c) = m.face_clipped_pct {
        if c > 2.0 {
            out.push(
                f(
                    "camera.face_clipping",
                    if c > 5.0 { Severity::Major } else { Severity::Minor },
                    "Parts of your face are blown out to white",
                    "The light on you is too hard or too strong, or the exposure is too high, so shine on forehead or cheeks loses all detail.",
                    "Soften the light (diffuser, bounce it off a wall) or move it back a little; blotting paper helps with shine. Then we'll lower exposure until no part of your face clips.",
                    FixKind::Manual,
                )
                .with_evidence("face_clipped_pct", c)
                .suggesting("diffuser"),
            );
        }
    }
    // Face in shadow: lots of face pixels with no detail. Measured as lost
    // detail, never as "darker than average skin", so deep skin tones that are
    // correctly lit are not flagged.
    if let Some(c) = m.face_crushed_pct {
        if c > 15.0 {
            out.push(
                f(
                    "camera.face_needs_light",
                    Severity::Major,
                    "Your face is in shadow",
                    "Parts of your face are so dark the camera can't show any detail there.",
                    "Add a light in front of you, slightly above eye level. Bring it closer rather than turning the camera brighter.",
                    FixKind::Manual,
                )
                .with_evidence("face_crushed_pct", c)
                .suggesting("key_light"),
            );
        }
    }

    // Exposure pumping.
    if m.exposure_swing_stops > 0.35 {
        let auto = s.auto_exposure && class.controllable();
        out.push(
            f(
                "camera.exposure_pumping",
                Severity::Major,
                "Your brightness keeps changing",
                if auto {
                    "Auto exposure re-adjusts every time you move or the screen flashes, so the picture breathes."
                } else {
                    "The light in your room is changing during the stream (daylight, screen glow or a flickering lamp)."
                },
                if auto {
                    "Make your main light stronger than your monitor's glow, then we'll lock the exposure."
                } else {
                    "Use a steady light in front of you that's stronger than your monitor, and close blinds if sunlight comes and goes."
                },
                if auto { FixKind::Auto } else { FixKind::Manual },
            )
            .with_evidence("swing_stops", m.exposure_swing_stops),
        );
    }
}

fn colour_findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, out: &mut Vec<Finding>) {
    if let (Some(cast), Some(pct)) = (m.paper_cast, m.paper_cast_pct) {
        if cast != ColourCast::Neutral && pct > 5.0 {
            let name = match cast {
                ColourCast::Warm => "orange",
                ColourCast::Cool => "blue",
                ColourCast::Green => "green",
                ColourCast::Magenta => "pink",
                ColourCast::Neutral => "",
            };
            let title = format!("Your picture looks too {name}");
            let (fix, kind) = by_class(
                class,
                "We'll lock white balance (in Kelvin) using your white paper. If you have mixed lights (window + warm lamp), switch one off or swap bulbs so they match.",
                "In the camera menu, set white balance to a Kelvin value (or custom white balance on the paper) instead of auto. Match your lights' colour if they differ.",
                "In the phone camera app, lock white balance while pointing at the white paper.",
            );
            let mut fd = f("camera.colour_cast", if pct > 12.0 { Severity::Major } else { Severity::Minor }, &title,
                "White isn't showing as white, so skin looks off. Usually auto white balance guessing wrong, or lights of different colours mixing.",
                &fix, kind).with_evidence("paper_cast_pct", pct);
            if matches!(cast, ColourCast::Green | ColourCast::Magenta) {
                fd.why = "White has a green or pink tint. Cheap LED bulbs and some fluorescent lights do this; white balance in Kelvin can't fully remove it.".into();
                fd.fix = "Swap to good-quality white bulbs (high CRI), or turn the tinted light away from you.".into();
                fd.fix_kind = FixKind::Manual;
                fd.can_decline = true;
                fd.suggest_product = Some("key_light".into());
            }
            out.push(fd);
        }
    }
    if m.colour_drift_pct > 8.0 && s.auto_wb {
        let (fix, kind) = by_class(
            class,
            "We'll lock white balance so the colour stays put.",
            "In the camera menu, set a fixed Kelvin white balance instead of auto.",
            "In the phone camera app, lock white balance.",
        );
        out.push(
            f("camera.colour_drift", Severity::Major, "Your colours keep shifting",
                "Auto white balance keeps re-guessing as your clothes, the game on screen or daylight changes.", &fix, kind)
                .with_evidence("drift_pct", m.colour_drift_pct),
        );
    }
    if m.face_tinted {
        let name = m.face_tint_name.clone().unwrap_or_else(|| "coloured".into());
        out.push(
            f(
                "camera.face_tint",
                Severity::Major,
                &format!("Your face has a {name} tint"),
                "Coloured light (RGB strips, a bright monitor) is landing on your skin. White balance can't fix light that's only on your face.",
                "Point RGB lights at the wall behind you, never at your face; make a white light in front of you stronger than your monitor; dim the screen or use a dark theme.",
                FixKind::Manual,
            )
            .with_evidence("face_hue_deg", m.face_hue_deg.unwrap_or(0.0)),
        );
    }
}

fn flicker_findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, mains: MainsHz, out: &mut Vec<Finding>) {
    let wrong_setting = class.controllable() && s.powerline_hz.map(|p| p != mains.as_u8()).unwrap_or(false);
    if wrong_setting {
        out.push(
            f(
                "camera.anti_flicker_wrong",
                if m.flicker.detected { Severity::Major } else { Severity::Minor },
                "Your camera's anti-flicker is set for the wrong country",
                &format!("Your lights flicker at your mains frequency ({} Hz). With the wrong setting you get rolling dark bands.", mains.as_u8()),
                &format!("We'll set anti-flicker to {} Hz.", mains.as_u8()),
                FixKind::Auto,
            )
            .with_evidence("setting_hz", s.powerline_hz.unwrap_or(0) as f64),
        );
    }
    if m.flicker.detected {
        let shutters = match mains {
            MainsHz::Hz50 => "1/50 or 1/100",
            MainsHz::Hz60 => "1/60 or 1/120",
        };
        let (fix, kind) = by_class(
            class,
            &format!("We'll set anti-flicker to {} Hz and a flicker-safe shutter ({shutters}). If bands remain, a dimmer or cheap LED bulb is the cause: run it at full brightness or swap it.", mains.as_u8()),
            &format!("In the camera menu set the shutter to {shutters} s. If bands remain, run dimmable lights at 100% or swap the bulb."),
            "Lock exposure in the phone camera app and set anti-banding to your country's frequency. If bands remain, swap the bulb.",
        );
        let mut fd = f(
            "camera.flicker",
            Severity::Major,
            "Dark bands roll through your picture",
            "Your lights flicker faster than you can see; the camera catches it as bands when its shutter doesn't match the mains frequency.",
            &fix,
            kind,
        )
        .with_evidence("amplitude_pct", m.flicker.amplitude_pct);
        if let Some(b) = m.flicker.bands_per_frame {
            fd = fd.with_evidence("bands_per_frame", b);
        }
        out.push(fd);
    }
}

fn focus_findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, out: &mut Vec<Finding>) {
    if let Some(cvv) = m.sharpness_cv {
        if cvv > 0.35 && (s.auto_focus || !class.controllable()) {
            let (fix, kind) = by_class(
                class,
                "More light helps autofocus, but we'll simply lock focus at your sitting distance.",
                "In the camera menu use continuous AF with face/eye detection (AF-C), or manual focus at your sitting distance.",
                "In the phone camera app, tap your face and lock focus.",
            );
            out.push(
                f("camera.focus_hunting", Severity::Major, "Your picture keeps going soft and sharp",
                    "Autofocus is hunting, especially in dim light or when you lean.", &fix, kind)
                    .with_evidence("sharpness_cv", cvv),
            );
        }
    }
    if let (Some(fs), Some(bs)) = (m.face_sharpness, m.background_sharpness) {
        if bs > 0.0 && fs / bs < 0.35 {
            let (fix, kind) = by_class(
                class,
                "We'll set focus on your face. If it stays soft, sit 60-80 cm from the camera and wipe the lens.",
                "Use face/eye-detect autofocus or focus manually on your eyes; wipe the lens.",
                "Tap your face to focus and lock it; wipe the phone lens.",
            );
            out.push(
                f("camera.soft_focus", Severity::Minor, "You're out of focus (the background is sharper)",
                    "The camera is focusing on the wall behind you, or you're closer than its fixed focus range.", &fix, kind)
                    .with_evidence("face_vs_background_sharpness", fs / bs),
            );
        }
    }
}

fn timing_findings(m: &VideoMetrics, s: &CameraState, class: CameraClass, out: &mut Vec<Finding>) {
    if let Some(d) = m.delivered_fps {
        if s.requested_fps > 0.0 && d < s.requested_fps * 0.97 {
            let cause = if s.format == VideoFormat::Yuy2 && s.requested_height >= 1080 {
                "Uncompressed video (YUY2) at 1080p is too much data for this camera's USB link."
            } else if s.ae_priority {
                "\"Low light compensation\" lets the camera slow down to gather more light."
            } else {
                "The camera is slowing down to gather more light, or the USB connection is struggling."
            };
            let (fix, kind) = by_class(
                class,
                "Add light first. We'll turn off low light compensation and use MJPEG; plug the camera straight into a USB 3 port on the PC, not a hub.",
                "Check the camera's HDMI output is set to the frame rate you stream (NTSC for 60p) and the capture card's format in OBS matches.",
                "In the phone app, pick 30 fps and add light in front of you.",
            );
            out.push(
                f("camera.low_fps", Severity::Major, "Your video is choppy", cause, &fix, kind)
                    .with_evidence("delivered_fps", d)
                    .with_evidence("requested_fps", s.requested_fps),
            );
        }
    }
    if m.dropouts >= 2 {
        out.push(
            f("camera.dropouts", Severity::Major, "Your camera freezes for a moment now and then",
                "Frames are going missing, usually because USB bandwidth is shared (hub, second camera, capture card on the same controller).",
                "Plug the camera straight into the PC (rear port), one camera per USB controller, no unpowered hubs. Use MJPEG.",
                FixKind::Manual)
                .with_evidence("dropouts", m.dropouts as f64),
        );
    }
}

fn lighting_shape_findings(m: &VideoMetrics, out: &mut Vec<Finding>) {
    let ratio = m.key_fill_ratio.unwrap_or(1.0);
    let raccoon = m.eye_to_cheek_stops.map(|e| e < -1.2).unwrap_or(false);
    if ratio > 3.0 || raccoon {
        let mut fd = f(
            "camera.harsh_shadows",
            if ratio > 4.0 || raccoon { Severity::Major } else { Severity::Minor },
            if raccoon { "Your eyes are in dark shadows" } else { "One side of your face is much darker" },
            if raccoon {
                "The main light is coming from straight above (a ceiling light), so your eye sockets fall into shadow."
            } else {
                "There's a single strong light on one side and nothing filling in the other side."
            },
            "Put a large, soft light in front of you, slightly above eye level and angled down; diffuse it (or bounce it off a wall). A white wall or board on the dark side fills the shadow.",
            FixKind::Manual,
        )
        .with_evidence("key_fill_ratio", ratio)
        .suggesting("key_light");
        if let Some(e) = m.eye_to_cheek_stops {
            fd = fd.with_evidence("eye_to_cheek_stops", e);
        }
        out.push(fd);
    }
    if let Some(g) = m.eye_glare_pct {
        if g > 0.5 {
            out.push(
                f(
                    "camera.glare",
                    Severity::Minor,
                    "Lights reflect in your eyes or glasses",
                    "A light right next to the camera (often a ring light) bounces straight back into the lens.",
                    "Raise the light and tilt it down, or move it 30-45 degrees to the side. Tilting glasses down slightly also helps.",
                    FixKind::Manual,
                )
                .with_evidence("eye_glare_pct", g),
            );
        }
    }
}

fn framing_findings(m: &VideoMetrics, out: &mut Vec<Finding>) {
    if let Some(p) = m.face_pitch_deg {
        if p > 12.0 {
            out.push(
                f("camera.camera_low", Severity::Minor, "Viewers look up at you",
                    "The camera is below your eye level, which shows the underside of your chin and nose.",
                    "Raise the camera to eye level (a 10-15 cm riser or stack of books under a laptop).", FixKind::Manual)
                    .with_evidence("pitch_deg", p)
                    .suggesting("camera_mount"),
            );
        }
    }
    if let Some(h) = m.face_height_frac {
        if h > 0.5 {
            out.push(
                f("camera.too_close", Severity::Minor, "You're too close to the camera",
                    "Wide webcam lenses stretch faces up close and crop your head.",
                    "Sit about an arm's length (60-80 cm) from the camera, head and shoulders in frame.", FixKind::Manual)
                    .with_evidence("face_height_frac", h),
            );
        } else if h < 0.12 {
            out.push(
                f("camera.too_far", Severity::Minor, "You're small in the picture",
                    "Your face fills only a small part of the frame, so viewers see more room than you.",
                    "Move the camera closer or zoom in a little so your head and shoulders fill the frame.", FixKind::Manual)
                    .with_evidence("face_height_frac", h),
            );
        }
    }
    let off_centre = m.face_center_x.map(|c| (c - 0.5).abs() > 0.2).unwrap_or(false);
    let eyes_off = m.eye_line_y.map(|e| !(0.2..=0.5).contains(&e)).unwrap_or(false);
    if off_centre || eyes_off {
        let mut fd = f(
            "camera.framing",
            Severity::Minor,
            "You're not well framed",
            "Your eyes look best about a third down from the top, with a little space above your head.",
            "Aim the camera so you're centred and your eyes sit about a third down from the top. Put the camera above the monitor you look at.",
            FixKind::Manual,
        );
        if let Some(c) = m.face_center_x {
            fd = fd.with_evidence("face_center_x", c);
        }
        if let Some(e) = m.eye_line_y {
            fd = fd.with_evidence("eye_line_y", e);
        }
        out.push(fd);
    }
}

fn range_findings(m: &VideoMetrics, out: &mut Vec<Finding>) {
    match m.range_hint {
        Some(RangeHint::WashedOut) => out.push(
            f("camera.range_washed_out", Severity::Minor, "Your blacks look grey and washed out",
                "The camera sends 'limited range' video but it's shown as 'full range' (or the other way round).",
                "We'll match the colour range in OBS's camera settings (usually Partial).", FixKind::Auto)
                .with_evidence("black_floor", m.black_floor_luma)
                .with_evidence("white_ceiling", m.white_ceiling_luma),
        ),
        Some(RangeHint::Crushed) => out.push(
            f("camera.range_crushed", Severity::Minor, "Dark areas turn solid black",
                "Contrast is set too high, or the colour range doesn't match between camera and OBS.",
                "We'll reset contrast to default and match the colour range; adding a little light on the dark side helps too.", FixKind::Auto)
                .with_evidence("shadow_crushed_pct", m.shadow_crushed_pct),
        ),
        None => {}
    }
}

/// Findings from the camera's current settings (webcams only).
fn state_findings(m: &VideoMetrics, s: &CameraState, out: &mut Vec<Finding>) {
    if s.auto_exposure && m.exposure_swing_stops <= 0.35 {
        out.push(f("camera.auto_exposure_on", Severity::Minor, "Your camera is on auto brightness",
            "Auto exposure will change your brightness whenever you move or the screen flashes.",
            "We'll lock exposure at the right value for your room.", FixKind::Auto));
    }
    if s.ae_priority && !out.iter().any(|x| x.id == "camera.low_fps") {
        out.push(f("camera.low_light_comp_on", Severity::Minor, "\"Low light compensation\" is on",
            "It lets the camera drop to 15 fps in dim light, which makes motion choppy.",
            "We'll turn it off and lock a frame rate.", FixKind::Auto));
    }
    if s.auto_wb && m.colour_drift_pct <= 8.0 {
        out.push(f("camera.auto_wb_on", Severity::Info, "Your camera is on auto white balance",
            "It can shift your skin colour when the screen or daylight changes.",
            "We'll lock white balance from your white paper.", FixKind::Auto));
    }
    if s.format == VideoFormat::Yuy2 && s.requested_height >= 1080 && s.requested_fps >= 30.0
        && !out.iter().any(|x| x.id == "camera.low_fps")
    {
        out.push(f("camera.yuy2_bandwidth", Severity::Major, "Your camera format can't do smooth 1080p",
            "Uncompressed YUY2 at 1080p needs more USB bandwidth than most webcams have.",
            "We'll switch the camera to MJPEG in OBS.", FixKind::Auto));
    }
    if s.hdr == Some(true) {
        out.push(f("camera.hdr_on", Severity::Minor, "HDR mode is on",
            "Webcam HDR can cause halos, waxy skin, flicker and a lower frame rate.",
            "We'll turn HDR off; even out the room light instead.", FixKind::Auto));
    }
    if let (Some(v), Some(r)) = (s.sharpness, s.sharpness_range) {
        if v > r.default + 0.1 * (r.max - r.min) {
            out.push(f("camera.oversharpened", Severity::Minor, "Your image is over-sharpened",
                "Too much sharpening draws white halos around edges and makes skin look crunchy.",
                "We'll bring sharpness back to the camera's default or a little below.", FixKind::Auto)
                .with_evidence("sharpness", v));
        }
    }
    if let (Some(v), Some(r)) = (s.saturation, s.saturation_range) {
        if v > r.default + 0.15 * (r.max - r.min) {
            out.push(f("camera.oversaturated", Severity::Minor, "Colours are too strong",
                "High saturation makes skin look orange or blotchy.",
                "We'll set saturation back to default.", FixKind::Auto)
                .with_evidence("saturation", v));
        }
    }
}
