use super::*;
use std::collections::HashSet;

fn db() -> ProfileDb {
    ProfileDb::from_embedded().expect("embedded profiles load")
}

fn mic_for(name: &str) -> (String, Confidence, Vec<QuestionId>) {
    let db = db();
    let m = db.match_mic(&DeviceIdentity::named(name)).expect("match");
    (m.profile.id.clone(), m.confidence, m.needs_questions.iter().map(|q| q.id).collect())
}

// ------------------------------------------------------------ invariants --

#[test]
fn embedded_and_dir_loads_agree() {
    let a = db();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../profiles");
    let b = ProfileDb::load_dir(dir).expect("load_dir");
    assert_eq!(a.mics.len(), b.mics.len());
    assert_eq!(a.interfaces.len(), b.interfaces.len());
    assert_eq!(a.cameras.len(), b.cameras.len());
    assert_eq!(a.camera_bodies.len(), b.camera_bodies.len());
    assert_eq!(a.boosters.len(), b.boosters.len());
}

#[test]
fn duplicate_ids_rejected() {
    let f = r#"{"schema_version":1,"boosters":[{"id":"x","brand":"b","model":"m","gain_db":25,"needs_phantom":true}]}"#;
    let err = ProfileDb::from_sources([("a.json".into(), f.into()), ("b.json".into(), f.into())]).unwrap_err();
    assert_eq!(err, ProfileError::DuplicateId("x".into()));
}

#[test]
fn wrong_schema_version_rejected() {
    let err = ProfileDb::from_sources([("a.json".into(), r#"{"schema_version":99}"#.into())]).unwrap_err();
    assert!(matches!(err, ProfileError::SchemaVersion { found: 99, .. }));
}

#[test]
fn usb_ids_roundtrip_as_hex() {
    let id: UsbId = serde_json::from_str(r#"{"vid":"046d","pid":"082D"}"#).unwrap();
    assert_eq!(id, UsbId { vid: 0x046d, pid: 0x082d });
    assert_eq!(serde_json::to_string(&id).unwrap(), r#"{"vid":"046d","pid":"082d"}"#);
}

#[test]
fn mic_invariants() {
    let db = db();
    for m in &db.mics {
        let d = m.distance_cm;
        assert!(d.min > 0.0 && d.min <= d.ideal && d.ideal <= d.max, "{}: distance {d:?}", m.id);
        for e in &m.eq_prior {
            assert!(e.gain_db.abs() <= 6.0, "{}: eq gain {} beyond +/-6 dB", m.id, e.gain_db);
            assert!(e.q > 0.0 && (20.0..=20000.0).contains(&e.freq_hz), "{}: bad eq {e:?}", m.id);
            if e.kind == EqKind::HighPass {
                assert_eq!(e.gain_db, 0.0, "{}: high-pass with gain", m.id);
            }
        }
        if let Some(g) = m.gain_needed_db {
            assert!((20.0..=80.0).contains(&g), "{}: gain {g}", m.id);
        }
        if let Address::MultiPattern { default, .. } = m.address {
            assert!(m.patterns.contains(&default), "{}: default pattern not in list", m.id);
        }
        if m.generic_class.is_some() {
            assert!(m.endpoint_name_patterns.is_empty(), "{}: generics must not match by name", m.id);
        }
        for p in &m.endpoint_name_patterns {
            assert_eq!(p, &p.to_lowercase(), "{}: patterns must be lower-case", m.id);
            assert!(p.chars().count() >= 3, "{}: pattern '{p}' too short", m.id);
        }
        // XLR condensers need phantom power.
        if m.connection == MicConnection::Xlr && m.capsule == Capsule::Condenser {
            assert!(m.needs_phantom, "{}: XLR condenser without phantom", m.id);
        }
    }
}

#[test]
fn camera_and_interface_invariants() {
    let db = db();
    for c in &db.cameras {
        for p in &c.name_patterns {
            assert_eq!(p, &p.to_lowercase(), "{}", c.id);
            assert!(p.chars().count() >= 3, "{}: pattern '{p}' too short", c.id);
        }
        for ctl in &c.controls {
            if let (Some(lo), Some(hi)) = (ctl.min, ctl.max) {
                assert!(lo <= hi, "{}: {ctl:?}", c.id);
            }
        }
        if c.generic {
            assert!(c.name_patterns.is_empty() && c.usb_ids.is_empty(), "{}: generic must not match", c.id);
        }
    }
    for i in &db.interfaces {
        assert!(!i.name_patterns.is_empty(), "{}: needs a name pattern", i.id);
        for p in &i.name_patterns {
            assert_eq!(p, &p.to_lowercase(), "{}", i.id);
        }
        if let Some(g) = i.max_gain_db {
            assert!((30.0..=90.0).contains(&g), "{}: gain {g}", i.id);
        }
    }
    for b in &db.boosters {
        assert!((10.0..=35.0).contains(&b.gain_db), "{}", b.id);
    }
    for b in &db.camera_bodies {
        assert!(!b.guidance.is_empty() && !b.aliases.is_empty(), "{}", b.id);
        let topics: HashSet<_> = b.guidance.iter().map(|g| g.topic).collect();
        assert!(topics.contains(&GuidanceTopic::Shutter) && topics.contains(&GuidanceTopic::WhiteBalance), "{}", b.id);
    }
}

#[test]
fn usb_ids_unique_per_category() {
    let db = db();
    let mut seen = HashSet::new();
    for id in db.mics.iter().flat_map(|m| &m.usb_ids) {
        assert!(seen.insert(*id), "mic usb id {id:?} used twice");
    }
    let mut seen = HashSet::new();
    for id in db.cameras.iter().flat_map(|c| &c.usb_ids) {
        assert!(seen.insert(*id), "camera usb id {id:?} used twice");
    }
}

#[test]
fn launch_list_families_present() {
    let db = db();
    let mic_f: HashSet<&str> = db.mics.iter().map(|m| m.family.as_str()).collect();
    for f in [
        "yeti", "quadcast", "solocast", "fifine", "elgato_wave", "seiren", "rode_nt_usb", "podmic", "shure_mv7",
        "samson_q2u", "atr2100", "maono_pd", "at2020usb", "tonor", // USB
        "sm7", "procaster", "re20", "sm58", "at2020", "rode_nt1", "wave_dx", // XLR
        "dji_mic", "rode_wireless", "hollyland_lark", // wireless
        "hyperx_cloud", "logitech_g_headset", "razer_blackshark", "arctis", "corsair_hs", "turtle_beach", "astro", // headsets
    ] {
        assert!(mic_f.contains(f), "mic family {f} missing");
    }
    let itf_f: HashSet<&str> = db.interfaces.iter().map(|i| i.family.as_str()).collect();
    for f in ["scarlett_solo", "scarlett_2i2", "wave_xlr", "goxlr", "behringer_umc", "rodecaster", "rode_streamer_x", "yamaha_ag", "motu_m", "ssl2", "ua_volt"] {
        assert!(itf_f.contains(f), "interface family {f} missing");
    }
    let cam_f: HashSet<&str> = db.cameras.iter().map(|c| c.family.as_str()).collect();
    for f in [
        "logitech_c920", "logitech_c922", "logitech_c270", "logitech_brio", "logitech_brio_500", "logitech_brio_100",
        "logitech_streamcam", "elgato_facecam", "razer_kiyo", "obsbot", "insta360_link", "elgato_capture", "phone_app",
    ] {
        assert!(cam_f.contains(f), "camera family {f} missing");
    }
    for b in ["sony_a6000", "sony_zv_e10", "sony_a6400", "sony_zv_1", "canon_m50", "canon_r50", "canon_r10", "panasonic_g7", "fujifilm_x_s10"] {
        assert!(db.camera_body(b).is_some(), "camera body {b} missing");
    }
    assert!(db.mics.iter().filter(|m| m.generic_class.is_none()).count() >= 60);
    assert!(db.interfaces.len() >= 20 && db.cameras.iter().filter(|c| !c.generic).count() >= 25);
}

#[test]
fn generics_present() {
    let db = db();
    for class in MicClass::ALL {
        assert!(db.generic_mic(class).is_some(), "generic mic {class:?} missing");
    }
    for class in [CameraClass::FullUvc, CameraClass::CaptureCard, CameraClass::Phone, CameraClass::Virtual, CameraClass::Laptop] {
        assert!(db.generic_camera(class).is_some(), "generic camera {class:?} missing");
    }
}

#[test]
fn owners_gear_present_and_flagged() {
    let db = db();
    for id in ["ev_re20", "rode_nt1a", "dji_mic_3"] {
        assert!(db.mic(id).is_some_and(|m| m.verify_first), "{id}");
    }
    assert!(db.interface("tc_helicon_goxlr").is_some_and(|i| i.verify_first));
    assert!(db.camera("logitech_c920").is_some_and(|c| c.verify_first));
    assert!(db.camera("elgato_cam_link_4k").is_some_and(|c| c.verify_first && c.class == CameraClass::CaptureCard));
    assert!(db.camera_body("sony_a6000").is_some_and(|b| b.verify_first));
    assert!(db.generic_mic(MicClass::HeadsetAnalog).is_some() && db.generic_camera(CameraClass::Phone).is_some());
    assert_eq!(db.find_camera_body("a6000").map(|b| b.id.as_str()), Some("sony_a6000"));
    assert_eq!(db.find_camera_body("Sony ZV-E10").map(|b| b.id.as_str()), Some("sony_zv_e10"));
    let nt1a = db.mic("rode_nt1a").unwrap();
    assert!(nt1a.needs_phantom && nt1a.address.facing() == Some(Facing::Side));
}

// --------------------------------------------------------------- matcher --

#[test]
fn matches_yeti_endpoint_name() {
    let (id, conf, q) = mic_for("Microphone (Yeti Stereo Microphone)");
    assert_eq!(id, "blue_yeti");
    assert_eq!(conf, Confidence::NameMatch);
    assert!(q.contains(&QuestionId::WhichPatternSelected) && q.contains(&QuestionId::WhichSideDoYouSpeakInto));
}

#[test]
fn yeti_variants_pick_the_most_specific() {
    assert_eq!(mic_for("Microphone (Yeti X)").0, "blue_yeti_x");
    assert_eq!(mic_for("Microphone (Yeti Nano)").0, "blue_yeti_nano");
    assert_eq!(mic_for("Microphone (Yeti GX)").0, "logitech_yeti_gx");
    assert_eq!(mic_for("Microphone (Blue Yeti X)").0, "blue_yeti_x");
}

#[test]
fn matches_quadcast_variants() {
    assert_eq!(mic_for("Microphone (HyperX QuadCast)").0, "hyperx_quadcast");
    assert_eq!(mic_for("Microphone (HyperX QuadCast S)").0, "hyperx_quadcast_s");
    assert_eq!(mic_for("Microphone (HyperX QuadCast 2 S)").0, "hyperx_quadcast_2_s");
}

#[test]
fn matches_by_usb_id_first() {
    let db = db();
    let m = db.match_mic(&DeviceIdentity::named("USB Audio Device").with_usb(0xb58e, 0x9e84)).unwrap();
    assert_eq!((m.profile.id.as_str(), m.confidence), ("blue_yeti", Confidence::Exact));
    let c = db.match_camera(&DeviceIdentity::named("USB Camera").with_usb(0x046d, 0x082d)).unwrap();
    assert_eq!((c.profile.id.as_str(), c.confidence), ("logitech_c920", Confidence::Exact));
}

#[test]
fn shure_mv7_plus_beats_mv7() {
    assert_eq!(mic_for("Microphone (Shure MV7+)").0, "shure_mv7_plus");
    assert_eq!(mic_for("Microphone (Shure MV7)").0, "shure_mv7");
}

#[test]
fn rode_names_with_slashed_o() {
    assert_eq!(mic_for("Microphone (RØDE NT-USB+)").0, "rode_nt_usb_plus");
    let db = db();
    let i = db.match_interface(&DeviceIdentity::named("RØDECaster Duo Main")).unwrap();
    assert_eq!(i.profile.id, "rode_rodecaster_duo");
}

#[test]
fn focusrite_4th_gen_name_is_ambiguous_and_asks() {
    let db = db();
    let dev = DeviceIdentity::named("Analogue 1 + 2 (Focusrite USB Audio)");
    let i = db.match_interface(&dev).unwrap();
    assert!(i.profile.family.starts_with("scarlett"));
    assert!(i.profile.input1_left_only);
    assert_eq!(i.needs_questions.first().map(|q| q.id), Some(QuestionId::WhichModel));
    assert!(i.needs_questions[0].options.iter().any(|o| o.contains("Solo (4th")));
    assert!(i.needs_questions[0].options.iter().any(|o| o.contains("2i2 (4th")));
    // The mic behind it is unknown: generic + ask which mic.
    let (id, conf, q) = mic_for("Analogue 1 + 2 (Focusrite USB Audio)");
    assert_eq!(id, "generic.xlr_dynamic_low_output");
    assert_eq!(conf, Confidence::Generic);
    assert!(q.contains(&QuestionId::WhichMicOnInterface) && q.contains(&QuestionId::BoosterInChain));
}

#[test]
fn goxlr_chat_mic_matches_goxlr_and_flags_wrong_endpoint() {
    let db = db();
    let dev = DeviceIdentity::named("Chat Mic (TC-Helicon GoXLR)");
    let input = db.match_input(&dev).unwrap();
    let itf = input.interface.expect("goxlr");
    assert_eq!(itf.profile.id, "tc_helicon_goxlr");
    assert_eq!(input.mic.profile.id, "generic.xlr_dynamic_low_output");
    let role = itf.profile.endpoint_role(&dev.name).unwrap();
    assert_eq!(role.role, EndpointKind::ChatMic);
    assert!(!role.use_in_obs);
    let stream = itf.profile.endpoint_role("Broadcast Stream Mix (TC-Helicon GoXLR)").unwrap();
    assert!(stream.use_in_obs && stream.role == EndpointKind::StreamMix);
    let mini = db.match_interface(&DeviceIdentity::named("Chat Mic (TC-Helicon GoXLR Mini)")).unwrap();
    assert_eq!(mini.profile.id, "tc_helicon_goxlr_mini");
}

#[test]
fn bluetooth_hands_free_goes_to_bt_generic() {
    let (id, conf, _) = mic_for("Headset (Some BT Hands-Free AG Audio)");
    assert_eq!((id.as_str(), conf), ("generic.headset_bluetooth", Confidence::Generic));
    // Even when the model name is visible, Hands-Free wins.
    assert_eq!(mic_for("Headset (G435 Hands-Free AG Audio)").0, "generic.headset_bluetooth");
    let db = db();
    let m = db.match_mic(&DeviceIdentity::named("Headset Microphone").with_hint(ConnectionHint::Bluetooth)).unwrap();
    assert_eq!(m.profile.generic_class, Some(MicClass::HeadsetBluetooth));
}

#[test]
fn unknown_usb_mic_falls_back_to_generic_with_questions() {
    let (id, conf, q) = mic_for("Microphone (USB PnP Audio Device)");
    assert_eq!((id.as_str(), conf), ("generic.usb_condenser_side", Confidence::Generic));
    assert!(q.contains(&QuestionId::WhichModel) && q.contains(&QuestionId::WhichSideDoYouSpeakInto));
}

#[test]
fn builtin_and_analog_heuristics() {
    assert_eq!(mic_for("Microphone Array (Realtek(R) Audio)").0, "generic.builtin_mic");
    let (id, _, q) = mic_for("Microphone (Realtek(R) Audio)");
    assert_eq!(id, "generic.headset_analog");
    assert!(q.contains(&QuestionId::WhichHeadsetModel));
    assert_eq!(mic_for("Headset Microphone (USB Audio Device)").0, "generic.headset_usb_wireless");
}

#[test]
fn fifine_brand_only_asks_which_model() {
    let (id, _, q) = mic_for("Microphone (FIFINE Microphone)");
    assert!(id.starts_with("fifine_"));
    assert_eq!(q.first(), Some(&QuestionId::WhichModel));
}

#[test]
fn sm7b_has_switch_question_when_picked() {
    let db = db();
    let sm7b = db.mic("shure_sm7b").unwrap();
    assert_eq!(sm7b.gain_needed_db, Some(60.0));
    assert!(!sm7b.needs_phantom && sm7b.address.facing() == Some(Facing::End));
}

#[test]
fn matches_wireless_lav() {
    assert_eq!(mic_for("Microphone (DJI MIC 3)").0, "dji_mic_3");
    let db = db();
    let m = db.mic("dji_mic_3").unwrap();
    assert!(m.onboard_dsp.iter().any(|d| d.kind == DspKind::NoiseCancel));
}

#[test]
fn matches_webcams() {
    let db = db();
    let cam = |n: &str| db.match_camera(&DeviceIdentity::named(n)).unwrap();
    let c920 = cam("HD Pro Webcam C920");
    assert_eq!((c920.profile.id.as_str(), c920.confidence), ("logitech_c920", Confidence::NameMatch));
    assert!(c920.profile.quirks.contains(&CameraQuirk::AePriorityFpsDrop));
    assert_eq!(c920.profile.persistence, Persistence::Volatile);
    assert!(c920.profile.control(ControlName::WhiteBalance).is_some_and(|c| c.min == Some(2000.0)));
    assert_eq!(cam("C922 Pro Stream Webcam").profile.id, "logitech_c922");
    assert_eq!(cam("Logitech BRIO").profile.id, "logitech_brio_4k");
    assert_eq!(cam("MX Brio").profile.id, "logitech_mx_brio");
    assert_eq!(cam("Logitech StreamCam").profile.id, "logitech_streamcam");
    assert_eq!(cam("Elgato Facecam Pro").profile.id, "elgato_facecam_pro");
    assert_eq!(cam("Razer Kiyo Pro").profile.id, "razer_kiyo_pro");
    assert_eq!(cam("Razer Kiyo X").profile.id, "razer_kiyo");
    assert_eq!(cam("Logitech Brio 500").profile.id, "logitech_brio_500");
    assert_eq!(cam("Logitech Brio 101").profile.id, "logitech_brio_100");
    assert_eq!(cam("Insta360 Link").profile.id, "insta360_link");
}

#[test]
fn capture_cards_are_coach_only_and_ask_for_body() {
    let db = db();
    let m = db.match_camera(&DeviceIdentity::named("Cam Link 4K")).unwrap();
    assert_eq!(m.profile.id, "elgato_cam_link_4k");
    assert!(m.profile.class.coach_only());
    assert_eq!(m.needs_questions.first().map(|q| q.id), Some(QuestionId::WhichCameraBody));
    let dongle = db.match_camera(&DeviceIdentity::named("USB Video")).unwrap();
    assert_eq!((dongle.profile.id.as_str(), dongle.confidence), ("generic.capture_card", Confidence::Generic));
    assert_eq!(dongle.needs_questions.first().map(|q| q.id), Some(QuestionId::WhichCameraBody));
}

#[test]
fn phones_virtual_laptop_and_unknown_cameras() {
    let db = db();
    let cam = |n: &str| db.match_camera(&DeviceIdentity::named(n)).unwrap();
    assert_eq!(cam("Camo").profile.class, CameraClass::Phone);
    assert_eq!(cam("Windows Virtual Camera").profile.id, "windows_phone_link");
    assert_eq!(cam("OBS Virtual Camera").profile.class, CameraClass::Virtual);
    assert_eq!(cam("Integrated Camera").profile.id, "generic.laptop");
    let unknown = cam("Some Random Webcam 2000");
    assert_eq!((unknown.profile.id.as_str(), unknown.confidence), ("generic.uvc_webcam", Confidence::Generic));
    let hinted = db.match_camera(&DeviceIdentity::named("Video Device").with_hint(ConnectionHint::CaptureCard)).unwrap();
    assert_eq!(hinted.profile.class, CameraClass::CaptureCard);
}

#[test]
fn every_profile_matches_its_own_patterns() {
    // Each pattern, used as the device name, must resolve to a profile that
    // owns that pattern (no variant shadowed by a longer base pattern).
    let db = db();
    for m in db.mics.iter().filter(|m| m.generic_class.is_none()) {
        for p in &m.endpoint_name_patterns {
            let hit = db.match_mic(&DeviceIdentity::named(&format!("Microphone ({p})"))).unwrap();
            assert!(hit.profile.endpoint_name_patterns.contains(p), "mic pattern '{p}' of {} resolved to {}", m.id, hit.profile.id);
        }
    }
    for c in db.cameras.iter().filter(|c| !c.generic) {
        for p in &c.name_patterns {
            let hit = db.match_camera(&DeviceIdentity::named(p)).unwrap();
            assert!(hit.profile.name_patterns.contains(p), "camera pattern '{p}' of {} resolved to {}", c.id, hit.profile.id);
        }
    }
    for i in &db.interfaces {
        for p in &i.name_patterns {
            let hit = db.match_interface(&DeviceIdentity::named(p)).unwrap();
            assert!(hit.profile.name_patterns.contains(p), "interface pattern '{p}' of {} resolved to {}", i.id, hit.profile.id);
        }
    }
}

#[test]
fn every_mic_question_is_exclusive_with_not_sure_and_switches_have_help() {
    let db = db();
    let re20 = db.questions_for_mic("ev_re20");
    let q = re20.iter().find(|q| q.id == QuestionId::SwitchPositions).expect("switch question");
    assert_eq!(q.key.as_deref(), Some("bass_tilt"));
    assert!(q.options.len() >= 3, "off / on / not sure: {:?}", q.options);
    assert!(q.help.as_ref().is_some_and(|h| !h.what.is_empty() && h.location.is_some()));
    for id in ["ev_re20", "ev_re320", "shure_sm7b", "shure_sm7db"] {
        let m = db.mic(id).unwrap();
        assert!(!m.controls.is_empty(), "{id}");
        for q in db.questions_for_mic(id) {
            assert!(q.options.is_empty() || q.options.iter().any(|o| o == NOT_SURE), "{id} {:?}", q.id);
        }
        for c in &m.controls {
            assert!(c.options.len() >= 2 && c.help.is_some() && c.source.is_some(), "{id} {}", c.key);
        }
    }
}
