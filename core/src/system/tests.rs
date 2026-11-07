use super::*;
use crate::constraints::Constraint;
use crate::finding::{Area, FixKind, Severity};
use serde_json::json;

const MIC_ID: &str = "{mic-endpoint}";
const PHONES_ID: &str = "{phones-endpoint}";

fn filter(name: &str, kind: &str, settings: serde_json::Value) -> ObsFilter {
    ObsFilter { name: name.into(), kind: kind.into(), enabled: true, settings }
}

/// A clean, well set-up system: the audit should find nothing.
fn good() -> SystemSnapshot {
    let mic = AudioEndpoint {
        id: MIC_ID.into(),
        name: "Analogue 1 + 2 (Focusrite USB Audio)".into(),
        is_capture: true,
        is_default: true,
        is_default_comms: true,
        sample_rate: 48_000,
        channels: 2,
        mic_boost_db: Some(0.0),
        endpoint_volume: 1.0,
        ..Default::default()
    };
    let phones = AudioEndpoint {
        id: PHONES_ID.into(),
        name: "Speakers (Focusrite USB Audio)".into(),
        is_capture: false,
        is_default: true,
        channels: 2,
        ..Default::default()
    };
    let mic_input = ObsAudioInput {
        name: "Mic".into(),
        kind: OBS_KIND_MIC.into(),
        device_id: MIC_ID.into(),
        device_name: "Analogue 1 + 2 (Focusrite USB Audio)".into(),
        mono_downmix: true,
        filters: vec![
            filter("Coach EQ", "vst_filter", json!({})),
            filter("Expander", "expander_filter", json!({"threshold": -52.0, "ratio": 3.0})),
            filter("Compressor", "compressor_filter", json!({"ratio": 3.5, "threshold": -20.0, "attack_time": 6, "release_time": 100})),
            filter("Limiter", "limiter_filter", json!({"threshold": -1.5, "release_time": 60})),
        ],
        ..Default::default()
    };
    let desktop = ObsAudioInput {
        name: "Desktop Audio".into(),
        kind: OBS_KIND_DESKTOP.into(),
        device_id: "default".into(),
        ..Default::default()
    };
    SystemSnapshot {
        windows: Some(WindowsAudioState {
            endpoints: vec![mic, phones],
            ducking: DuckingPreference::DoNothing,
            mic_privacy_desktop_allowed: true,
            usb_selective_suspend_enabled: false,
            level_changed_by_other_app: false,
        }),
        obs: Some(ObsState {
            version: "33.0.1".into(),
            websocket_enabled: true,
            sample_rate: 48_000,
            audio_inputs: vec![mic_input, desktop],
            video_captures: vec![ObsVideoCapture {
                name: "Webcam".into(),
                device_id: "cam".into(),
                device_name: "HD Pro Webcam C920".into(),
                res_type_custom: true,
                width: 1920,
                height: 1080,
                fps: Some(30.0),
                video_format: "MJPEG".into(),
                color_range: ColorRange::Default,
                ..Default::default()
            }],
            output_color_range: ColorRange::Partial,
            encoder: Some(ObsEncoder {
                encoder_id: "obs_nvenc_h264_tex".into(),
                rate_control: "CBR".into(),
                bitrate_kbps: 6000,
                keyframe_sec: 2,
                preset: "p5".into(),
                output_width: 1920,
                output_height: 1080,
                fps: 60.0,
                audio_bitrate_kbps: 160,
                enhanced_broadcasting: false,
            }),
            recording_format: "hybrid_mp4".into(),
            platform: Platform::Twitch,
            mic_input: Some("Mic".into()),
        }),
        camera: Some(CameraSystemState::default()),
        processing: KnownProcessingApps::default(),
        mic: MicContext {
            single_live_channel: true,
            direct_monitoring_on: Some(true),
            measured_noise_floor_dbfs: Some(-62.0),
            measured_quiet_speech_dbfs: Some(-38.0),
            ..Default::default()
        },
        hardware: Some(HardwareInfo { gpu_vendor: GpuVendor::Nvidia, gpu_model: "RTX 4070".into(), has_nvenc: true, cpu_cores: 8, ..Default::default() }),
        network: Some(NetworkInfo { connection: Connection::Wired, measured_upload_kbps: Some(20_000) }),
        connection: None,
    }
}

fn ids_of(s: &SystemSnapshot) -> Vec<String> {
    audit(s, &[]).into_iter().map(|f| f.id).collect()
}

fn has(s: &SystemSnapshot, id: &str) -> bool {
    ids_of(s).iter().any(|x| x == id)
}

fn win(s: &mut SystemSnapshot) -> &mut WindowsAudioState {
    s.windows.as_mut().unwrap()
}
fn mic_ep(s: &mut SystemSnapshot) -> &mut AudioEndpoint {
    win(s).endpoints.iter_mut().find(|e| e.id == MIC_ID).unwrap()
}
fn obs(s: &mut SystemSnapshot) -> &mut ObsState {
    s.obs.as_mut().unwrap()
}
fn mic_in(s: &mut SystemSnapshot) -> &mut ObsAudioInput {
    obs(s).audio_inputs.iter_mut().find(|i| i.name == "Mic").unwrap()
}
fn enc(s: &mut SystemSnapshot) -> &mut ObsEncoder {
    obs(s).encoder.as_mut().unwrap()
}

#[test]
fn good_setup_has_no_findings() {
    let f = audit(&good(), &[]);
    assert!(f.is_empty(), "unexpected findings: {:#?}", f.iter().map(|x| &x.id).collect::<Vec<_>>());
}

#[test]
fn empty_snapshot_is_quiet() {
    assert!(audit(&SystemSnapshot::default(), &[]).is_empty());
}

#[test]
fn snapshot_round_trips_through_json() {
    let s = good();
    let text = serde_json::to_string(&s).unwrap();
    let back: SystemSnapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(s, back);
    // Partial JSON from the app still parses with defaults.
    let partial: SystemSnapshot = serde_json::from_str(r#"{"windows":{"endpoints":[{"id":"x","is_default":true}]}}"#).unwrap();
    assert_eq!(partial.windows.unwrap().endpoints[0].endpoint_volume, 1.0);
}

// ---------------------------------------------------------------- Windows

#[test]
fn mic_privacy() {
    let mut s = good();
    win(&mut s).mic_privacy_desktop_allowed = false;
    let f = audit(&s, &[]);
    assert_eq!(f[0].id, ids::MIC_PRIVACY);
    assert_eq!(f[0].severity, Severity::Critical);
}

#[test]
fn enhancements_and_voice_clarity() {
    let mut s = good();
    mic_ep(&mut s).enhancements_enabled = true;
    assert!(has(&s, ids::ENHANCEMENTS));
    assert!(!has(&s, ids::VOICE_CLARITY));
    mic_ep(&mut s).voice_clarity_enabled = true;
    assert!(has(&s, ids::VOICE_CLARITY));
}

#[test]
fn enhancements_on_other_endpoint_ignored() {
    let mut s = good();
    win(&mut s).endpoints[1].enhancements_enabled = true; // headphones
    assert!(!has(&s, ids::ENHANCEMENTS));
}

#[test]
fn sample_rate_mismatch() {
    let mut s = good();
    mic_ep(&mut s).sample_rate = 44_100;
    let f = audit(&s, &[]);
    let m = f.iter().find(|x| x.id == ids::SAMPLE_RATE_MISMATCH).unwrap();
    assert_eq!(m.fix_kind, FixKind::Guided);
    assert_eq!(m.evidence["mic_sample_rate_hz"], 44_100.0);

    let mut s = good();
    obs(&mut s).sample_rate = 44_100;
    mic_ep(&mut s).sample_rate = 44_100;
    let f = audit(&s, &[]);
    let m = f.iter().find(|x| x.id == ids::SAMPLE_RATE_MISMATCH).unwrap();
    assert_eq!(m.fix_kind, FixKind::Auto, "OBS not at 48k is auto-fixable");
}

#[test]
fn mic_boost_rules() {
    let mut s = good();
    mic_ep(&mut s).mic_boost_db = Some(10.0);
    assert!(has(&s, ids::MIC_BOOST));
    mic_ep(&mut s).mic_boost_db = Some(30.0);
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::MIC_BOOST).unwrap().severity, Severity::Major);
    // Onboard Realtek: +20 dB is fine, +30 is not.
    mic_ep(&mut s).is_onboard = true;
    mic_ep(&mut s).mic_boost_db = Some(20.0);
    assert!(!has(&s, ids::MIC_BOOST));
    mic_ep(&mut s).mic_boost_db = Some(30.0);
    assert!(has(&s, ids::MIC_BOOST));
}

#[test]
fn endpoint_volume() {
    let mut s = good();
    mic_ep(&mut s).endpoint_volume = 0.6;
    assert!(has(&s, ids::ENDPOINT_VOLUME));
    s.mic.windows_slider_is_hardware_gain = true;
    assert!(!has(&s, ids::ENDPOINT_VOLUME));
}

#[test]
fn exclusive_listen_usb_ducking_hijack() {
    let mut s = good();
    mic_ep(&mut s).exclusive_mode_allowed = true;
    mic_ep(&mut s).listen_to_device = true;
    win(&mut s).usb_selective_suspend_enabled = true;
    win(&mut s).ducking = DuckingPreference::Reduce80;
    win(&mut s).level_changed_by_other_app = true;
    let ids = ids_of(&s);
    for id in [ids::EXCLUSIVE_MODE, ids::LISTEN_TO_DEVICE, ids::USB_SUSPEND, ids::DUCKING, ids::LEVEL_HIJACKED] {
        assert!(ids.contains(&id.to_string()), "missing {id}");
    }
    let mut s = good();
    win(&mut s).ducking = DuckingPreference::Reduce50;
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::DUCKING).unwrap().severity, Severity::Minor);
    assert_eq!(DuckingPreference::from_registry(3), DuckingPreference::DoNothing);
    assert_eq!(DuckingPreference::from_registry(1), DuckingPreference::Reduce80);
}

#[test]
fn bluetooth_handsfree_mic_and_output() {
    let mut s = good();
    mic_ep(&mut s).is_bluetooth_handsfree = true;
    let f = audit(&s, &[]);
    let b = f.iter().find(|x| x.id == ids::BLUETOOTH_HANDSFREE).unwrap();
    assert_eq!(b.severity, Severity::Critical);
    assert_eq!(b.suggest_product.as_deref(), Some("usb_mic"));

    let mut s = good();
    win(&mut s).endpoints[1].is_bluetooth_handsfree = true;
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::BLUETOOTH_HANDSFREE).unwrap().severity, Severity::Major);
}

#[test]
fn default_device_resolution() {
    let mut s = good();
    mic_in(&mut s).device_id = "default".into();
    // Default resolves to the default capture endpoint, so endpoint rules still apply.
    mic_ep(&mut s).enhancements_enabled = true;
    let f = audit(&s, &[]);
    assert!(f.iter().any(|x| x.id == ids::ENHANCEMENTS));
    let d = f.iter().find(|x| x.id == ids::OBS_DEFAULT_DEVICE).unwrap();
    assert_eq!(d.fix_kind, FixKind::Auto);
    // Without Windows info we can't pin automatically.
    s.windows = None;
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::OBS_DEFAULT_DEVICE).unwrap().fix_kind, FixKind::Guided);
}

// ---------------------------------------------------------------- OBS mic chain

#[test]
fn left_channel_only() {
    let mut s = good();
    mic_in(&mut s).mono_downmix = false;
    let f = audit(&s, &[]);
    assert_eq!(f[0].id, ids::NOT_MONO, "critical, ranked first");
    // Mono mic endpoint: no issue.
    mic_ep(&mut s).channels = 1;
    assert!(!has(&s, ids::NOT_MONO));
    // Profile says both channels live (e.g. stereo USB mic): no issue.
    mic_ep(&mut s).channels = 2;
    s.mic.single_live_channel = false;
    assert!(!has(&s, ids::NOT_MONO));
}

#[test]
fn monitoring_rules() {
    let mut s = good();
    mic_in(&mut s).monitoring = MonitoringType::MonitorAndOutput;
    assert!(has(&s, ids::MONITORING_DOUBLED));
    s.mic.direct_monitoring_on = None;
    assert!(has(&s, ids::MONITORING_DELAYED));
    assert!(!has(&s, ids::MONITORING_DOUBLED));
}

#[test]
fn muted_mic() {
    let mut s = good();
    mic_in(&mut s).muted = true;
    assert!(has(&s, ids::MIC_MUTED));
}

#[test]
fn ai_suppression_detection() {
    let cases = [
        ("noise_suppress_filter_v2", json!({"method": "rnnoise"}), true),
        ("noise_suppress_filter_v2", json!({"method": "nvafx_denoiser"}), true),
        ("noise_suppress_filter_v2", json!({}), true),
        ("noise_suppress_filter_v2", json!({"method": "speex"}), false),
        ("noise_suppress_filter", json!({}), false),
        ("nvidia_audiofx_filter", json!({}), true),
        ("compressor_filter", json!({}), false),
    ];
    for (kind, settings, ai) in cases {
        assert_eq!(obs_audio::is_ai_suppressor(&filter("x", kind, settings.clone())), ai, "{kind} {settings}");
    }
    let mut s = good();
    mic_in(&mut s).filters.insert(0, filter("Noise Suppression", "noise_suppress_filter_v2", json!({"method": "rnnoise"})));
    assert!(has(&s, ids::AI_SUPPRESSION));
    assert!(!has(&s, ids::SPEEX_SUPPRESSION));
    let mut s = good();
    mic_in(&mut s).filters.insert(0, filter("NS", "noise_suppress_filter_v2", json!({"method": "speex"})));
    assert!(has(&s, ids::SPEEX_SUPPRESSION));
    assert!(!has(&s, ids::AI_SUPPRESSION));
    // Disabled filters are ignored.
    let mut s = good();
    let mut f = filter("NS", "noise_suppress_filter_v2", json!({}));
    f.enabled = false;
    mic_in(&mut s).filters.insert(0, f);
    assert!(!has(&s, ids::AI_SUPPRESSION));
}

#[test]
fn filter_order_rules() {
    let mut s = good();
    // Limiter in the middle → major.
    let lim = mic_in(&mut s).filters.remove(3);
    mic_in(&mut s).filters.insert(1, lim);
    let f = audit(&s, &[]);
    let o = f.iter().find(|x| x.id == ids::FILTER_ORDER).unwrap();
    assert_eq!(o.severity, Severity::Major);
    // Compressor before expander → minor.
    let mut s = good();
    mic_in(&mut s).filters.swap(1, 2);
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::FILTER_ORDER).unwrap().severity, Severity::Minor);
}

#[test]
fn recommended_order_is_stable_and_keeps_unknowns_with_neighbour() {
    let input = ObsAudioInput {
        filters: vec![
            filter("Lim", "limiter_filter", json!({})),
            filter("Comp", "compressor_filter", json!({})),
            filter("Mystery", "some_third_party_filter", json!({})),
            filter("Gain", "gain_filter", json!({})),
            filter("EQ", "basic_eq_filter", json!({})),
        ],
        ..Default::default()
    };
    assert_eq!(obs_audio::recommended_order(&input), vec!["Gain", "EQ", "Comp", "Mystery", "Lim"]);
}

#[test]
fn gate_rules() {
    let mut s = good();
    mic_in(&mut s).filters.insert(1, filter("Gate", "noise_gate_filter", json!({"open_threshold": -30.0, "close_threshold": -36.0, "attack_time": 5, "hold_time": 200})));
    // Quiet speech -38: open at -30 chops words.
    assert!(has(&s, ids::GATE_CHOPS));
    // Well-set gate: open -45, close -54.
    let g = &mut mic_in(&mut s).filters[1];
    g.settings = json!({"open_threshold": -45.0, "close_threshold": -54.0, "attack_time": 5, "hold_time": 200});
    assert!(!has(&s, ids::GATE_CHOPS));
    assert!(!has(&s, ids::GATE_SLOW));
    // OBS default attack 25 ms → slow.
    mic_in(&mut s).filters[1].settings = json!({"open_threshold": -45.0, "close_threshold": -54.0});
    assert!(has(&s, ids::GATE_SLOW));
    // Without measurements, a default gate is only "slow", never "chops".
    s.mic.measured_quiet_speech_dbfs = None;
    s.mic.measured_noise_floor_dbfs = None;
    assert!(!has(&s, ids::GATE_CHOPS));
}

#[test]
fn low_snr_replaces_gate_advice() {
    let mut s = good();
    s.mic.measured_noise_floor_dbfs = Some(-44.0);
    s.mic.measured_quiet_speech_dbfs = Some(-38.0);
    mic_in(&mut s).filters.insert(1, filter("Gate", "noise_gate_filter", json!({"open_threshold": -30.0, "attack_time": 5})));
    let ids = ids_of(&s);
    assert!(ids.contains(&ids::LOW_SNR_FOR_GATE.to_string()));
    assert!(!ids.contains(&ids::GATE_CHOPS.to_string()));
    let f = audit(&s, &[]);
    assert!(f.iter().find(|x| x.id == ids::LOW_SNR_FOR_GATE).unwrap().can_decline);
}

#[test]
fn recommended_gate_sits_between_noise_and_speech() {
    let g = obs_audio::recommended_gate(Some(-62.0), Some(-38.0));
    let open = g["open_threshold"].as_f64().unwrap();
    let close = g["close_threshold"].as_f64().unwrap();
    assert!(close >= -56.5 && close < open, "close {close} open {open}");
    assert!(open <= -41.0, "open {open} must be below quiet speech");
    assert_eq!(g["attack_time"], 5);
}

#[test]
fn expander_rules() {
    let mut s = good();
    mic_in(&mut s).filters[1].settings = json!({"threshold": -36.0});
    assert!(has(&s, ids::EXPANDER_CHOPS));
    let rec = obs_audio::recommended_expander(Some(-62.0), Some(-38.0), &json!({"presets": "expander"}));
    assert_eq!(rec["threshold"], -50.0);
    assert_eq!(rec["presets"], "expander", "existing keys kept");
}

#[test]
fn compressor_rules() {
    let mut s = good();
    mic_in(&mut s).filters[2].settings = json!({}); // OBS defaults: ratio 10
    assert!(has(&s, ids::COMPRESSOR_EXTREME));
    mic_in(&mut s).filters[2].settings = json!({"ratio": 4.0, "threshold": -40.0});
    assert!(has(&s, ids::COMPRESSOR_EXTREME));
    mic_in(&mut s).filters[2].settings = json!({"ratio": 4.0, "threshold": -22.0, "release_time": 80});
    assert!(!has(&s, ids::COMPRESSOR_EXTREME));
    let rec = obs_audio::recommended_compressor(&json!({"threshold": -50.0, "sidechain_source": "none"}));
    assert_eq!(rec["ratio"], 3.5);
    assert_eq!(rec["threshold"], -20.0);
    assert_eq!(rec["sidechain_source"], "none");
}

#[test]
fn limiter_rules() {
    let mut s = good();
    mic_in(&mut s).filters.pop();
    assert!(has(&s, ids::LIMITER_MISSING));
    let mut s = good();
    mic_in(&mut s).filters[3].settings = json!({"threshold": -10.0});
    assert!(has(&s, ids::LIMITER_LEVEL));
    mic_in(&mut s).filters[3].settings = json!({"threshold": 0.0});
    assert!(has(&s, ids::LIMITER_LEVEL));
    mic_in(&mut s).filters[3].settings = json!({});
    assert!(!has(&s, ids::LIMITER_LEVEL), "OBS default -6 is acceptable");
}

#[test]
fn gain_stacking_and_constraint() {
    let mut s = good();
    mic_ep(&mut s).mic_boost_db = Some(10.0);
    mic_in(&mut s).filters.insert(0, filter("Gain", "gain_filter", json!({"db": 15.0})));
    let f = audit(&s, &[]);
    let g = f.iter().find(|x| x.id == ids::GAIN_STACKED).unwrap();
    assert_eq!(g.severity, Severity::Major);
    assert_eq!(g.suggest_product.as_deref(), Some("inline_booster"));
    let f = audit(&s, &[Constraint::CantAddCleanGain]);
    assert_eq!(f.iter().find(|x| x.id == ids::GAIN_STACKED).unwrap().severity, Severity::Info);
    // A single gain stage is not "stacking".
    let mut s = good();
    mic_in(&mut s).filters.insert(0, filter("Gain", "gain_filter", json!({"db": 15.0})));
    assert!(!has(&s, ids::GAIN_STACKED));
}

#[test]
fn gain_stacked_and_mic_boost_tips_follow_gain_access() {
    use crate::audio::findings::GainAccess;
    let fix_for = |gain: GainAccess, wh: Option<&str>, id: &str| {
        let mut s = good();
        mic_ep(&mut s).mic_boost_db = Some(10.0);
        mic_in(&mut s).filters.insert(0, filter("Gain", "gain_filter", json!({"db": 15.0})));
        s.mic.gain = gain;
        s.mic.gain_where = wh.map(String::from);
        audit(&s, &[]).into_iter().find(|f| f.id == id).unwrap().fix
    };
    for id in [ids::GAIN_STACKED, ids::MIC_BOOST] {
        // A mic with no gain control is never told to turn its gain up.
        let none = fix_for(GainAccess::None, None, id);
        assert!(!none.to_lowercase().contains("turn up the gain"), "{none}");
        assert!(!none.contains("interface gain"), "{none}");
        assert!(none.contains("no gain control") && none.contains("close"), "{none}");
        // GoXLR: the gain lives in its app.
        let sw = fix_for(GainAccess::Software, Some("the GoXLR app"), id);
        assert!(sw.contains("in the GoXLR app"), "{sw}");
        assert!(!sw.contains("interface"), "{sw}");
        // A knob is still a knob.
        assert!(fix_for(GainAccess::Knob, None, id).contains("gain"));
        // Unknown gear: "if it has one".
        assert!(fix_for(GainAccess::Unknown, None, id).contains("if"));
    }
}

#[test]
fn double_capture_app_audio() {
    let mut s = good();
    obs(&mut s).audio_inputs.push(ObsAudioInput {
        name: "Discord".into(),
        kind: OBS_KIND_APP_AUDIO.into(),
        app_target: Some("Discord.exe".into()),
        ..Default::default()
    });
    let f = audit(&s, &[]);
    let d = f.iter().find(|x| x.id == ids::DOUBLE_CAPTURE_APP).unwrap();
    assert!(d.title.contains("Discord"), "{}", d.title);
    // Desktop audio muted → fine.
    obs(&mut s).audio_inputs[1].muted = true;
    assert!(!has(&s, ids::DOUBLE_CAPTURE_APP));
}

#[test]
fn double_capture_stream_mix() {
    let mut s = good();
    obs(&mut s).audio_inputs.push(ObsAudioInput {
        name: "Wave Link".into(),
        kind: OBS_KIND_MIC.into(),
        device_id: "wl".into(),
        device_name: "Wave Link Stream (Elgato Virtual Audio)".into(),
        ..Default::default()
    });
    assert!(has(&s, ids::DOUBLE_CAPTURE_MIX));
    mic_in(&mut s).muted = true;
    assert!(!has(&s, ids::DOUBLE_CAPTURE_MIX));
    assert!(obs_audio::is_stream_mix_device("Broadcast Stream Mix (TC-Helicon GoXLR)"));
    assert!(!obs_audio::is_stream_mix_device("Chat Mic (TC-Helicon GoXLR)"));
}

#[test]
fn processing_apps() {
    let mut s = good();
    s.processing.krisp_active = true;
    let f = audit(&s, &[]);
    assert!(f.iter().find(|x| x.id == ids::AI_APP_PROCESSING).unwrap().title.contains("Krisp"));
    let mut s = good();
    s.processing.wave_link_fx_active = true;
    assert!(has(&s, ids::VENDOR_DSP_STACKED));
    // Only a limiter in OBS → not stacked.
    mic_in(&mut s).filters.retain(|f| f.kind == "limiter_filter");
    assert!(!has(&s, ids::VENDOR_DSP_STACKED));
}

#[test]
fn onboard_auto_level() {
    let mut s = good();
    s.mic.onboard_dsp_active = vec!["auto_level".into()];
    assert!(has(&s, ids::ONBOARD_DSP_FIGHTS));
    mic_in(&mut s).filters.retain(|f| f.kind != "compressor_filter");
    assert!(!has(&s, ids::ONBOARD_DSP_FIGHTS));
}

#[test]
fn obs_app_rules() {
    let mut s = good();
    obs(&mut s).websocket_enabled = false;
    obs(&mut s).version = "29.1.3".into();
    let ids = ids_of(&s);
    assert!(ids.contains(&ids::WEBSOCKET_OFF.to_string()));
    assert!(ids.contains(&ids::OBS_OLD.to_string()));
}

// ---------------------------------------------------------------- camera

#[test]
fn camera_system_rules() {
    let mut s = good();
    s.camera = Some(CameraSystemState {
        privacy_allowed: false,
        frame_server_running: false,
        studio_effects_on: true,
        multi_app_camera_on: true,
        vendor_apps_running: vec!["G HUB".into()],
    });
    let ids = ids_of(&s);
    for id in [ids::CAM_PRIVACY, ids::CAM_FRAME_SERVER, ids::CAM_STUDIO_EFFECTS, ids::CAM_MULTI_APP, ids::CAM_VENDOR_APPS] {
        assert!(ids.contains(&id.to_string()), "missing {id}");
    }
    let f = audit(&s, &[]);
    assert!(f.iter().find(|x| x.id == ids::CAM_VENDOR_APPS).unwrap().why.contains("G HUB"));
    assert!(f.iter().all(|x| x.area == Area::Camera));
}

#[test]
fn camera_obs_source_rules() {
    let mut s = good();
    obs(&mut s).video_captures[0].res_type_custom = false;
    assert!(has(&s, ids::CAM_DEVICE_DEFAULT));
    let mut s = good();
    obs(&mut s).video_captures[0].video_format = "YUY2".into();
    assert!(has(&s, ids::CAM_YUY2_HIGH_RES));
    // 720p30 YUY2 is fine; 720p60 is not.
    obs(&mut s).video_captures[0].height = 720;
    obs(&mut s).video_captures[0].width = 1280;
    assert!(!has(&s, ids::CAM_YUY2_HIGH_RES));
    obs(&mut s).video_captures[0].fps = Some(60.0);
    assert!(has(&s, ids::CAM_YUY2_HIGH_RES));
    let mut s = good();
    obs(&mut s).video_captures[0].color_range = ColorRange::Full;
    assert!(has(&s, ids::CAM_COLOR_RANGE));
    obs(&mut s).video_captures[0].color_range = ColorRange::Partial;
    assert!(!has(&s, ids::CAM_COLOR_RANGE));
}

// ---------------------------------------------------------------- encoder

#[test]
fn cpu_encoder_with_gpu() {
    let mut s = good();
    enc(&mut s).encoder_id = "obs_x264".into();
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::ENC_CPU_WITH_GPU).unwrap().severity, Severity::Major);
    s.hardware.as_mut().unwrap().cpu_cores = 16;
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::ENC_CPU_WITH_GPU).unwrap().severity, Severity::Minor);
    s.hardware.as_mut().unwrap().has_nvenc = false;
    assert!(!has(&s, ids::ENC_CPU_WITH_GPU), "no hardware encoder available");
}

#[test]
fn rate_control_keyframe_audio_recording() {
    let mut s = good();
    enc(&mut s).rate_control = "VBR".into();
    enc(&mut s).keyframe_sec = 0;
    enc(&mut s).audio_bitrate_kbps = 96;
    obs(&mut s).recording_format = "mp4".into();
    let ids = ids_of(&s);
    for id in [ids::ENC_NOT_CBR, ids::ENC_KEYFRAME, ids::ENC_AUDIO_LOW, ids::REC_MP4] {
        assert!(ids.contains(&id.to_string()), "missing {id}");
    }
}

#[test]
fn enhanced_broadcasting_exempts_encoder_rules() {
    let mut s = good();
    enc(&mut s).enhanced_broadcasting = true;
    enc(&mut s).rate_control = "VBR".into();
    enc(&mut s).bitrate_kbps = 9000;
    enc(&mut s).keyframe_sec = 0;
    let ids = ids_of(&s);
    assert!(!ids.contains(&ids::ENC_NOT_CBR.to_string()));
    assert!(!ids.contains(&ids::ENC_OVER_PLATFORM.to_string()));
    assert!(!ids.contains(&ids::ENC_KEYFRAME.to_string()));
}

#[test]
fn platform_limit_only_twitch() {
    let mut s = good();
    enc(&mut s).bitrate_kbps = 8000;
    assert!(has(&s, ids::ENC_OVER_PLATFORM));
    obs(&mut s).platform = Platform::YouTube;
    assert!(!has(&s, ids::ENC_OVER_PLATFORM));
}

#[test]
fn bitrate_for_resolution() {
    let mut s = good();
    enc(&mut s).bitrate_kbps = 4500; // 1080p60 → 0.036 bpp
    let f = audit(&s, &[]);
    let r = f.iter().find(|x| x.id == ids::ENC_LOW_FOR_RES).unwrap();
    assert_eq!(r.severity, Severity::Minor);
    assert!(r.evidence["suggested_height"] < 1080.0);
    enc(&mut s).bitrate_kbps = 3000; // 0.024 bpp
    let f = audit(&s, &[]);
    assert_eq!(f.iter().find(|x| x.id == ids::ENC_LOW_FOR_RES).unwrap().severity, Severity::Major);
    assert_eq!(encoder::suggested_output_height(6000, 30.0), 1080);
    assert_eq!(encoder::suggested_output_height(6000, 60.0), 936);
    assert_eq!(encoder::suggested_output_height(4500, 60.0), 864);
    assert_eq!(encoder::suggested_output_height(3000, 60.0), 720);
}

#[test]
fn upload_and_wifi() {
    let mut s = good();
    s.network = Some(NetworkInfo { connection: Connection::Wired, measured_upload_kbps: Some(7000) });
    let f = audit(&s, &[]);
    let u = f.iter().find(|x| x.id == ids::ENC_OVER_UPLOAD).unwrap();
    assert_eq!(u.evidence["safe_video_kbps"], 5000.0); // 7000*0.75-160 = 5090 → 5000
    s.network = Some(NetworkInfo { connection: Connection::Wifi, measured_upload_kbps: Some(20_000) });
    let f = audit(&s, &[]);
    let w = f.iter().find(|x| x.id == ids::NET_WIFI).unwrap();
    assert_eq!(w.severity, Severity::Major);
    assert!(w.can_decline);
    let f = audit(&s, &[Constraint::MustUseWifi]);
    assert_eq!(f.iter().find(|x| x.id == ids::NET_WIFI).unwrap().severity, Severity::Info);
    assert_eq!(encoder::safe_bitrate_for_upload(10_000, 160, true), 5750);
}

// ---------------------------------------------------------------- ranking

#[test]
fn findings_are_ranked_and_deduplicated() {
    let mut s = good();
    mic_in(&mut s).mono_downmix = false; // critical auto
    win(&mut s).ducking = DuckingPreference::Reduce50; // minor
    mic_ep(&mut s).listen_to_device = true; // major
    let f = audit(&s, &[]);
    let sev: Vec<Severity> = f.iter().map(|x| x.severity).collect();
    let mut sorted = sev.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sev, sorted);
    let mut ids: Vec<_> = f.iter().map(|x| (&x.id, &x.title)).collect();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n);
}

// ---------------------------------------------------------------- quick fixes

fn plan(s: &SystemSnapshot) -> Vec<PlannedAction> {
    quick_fixes(s, &audit(s, &[]))
}

fn find_action<'a>(p: &'a [PlannedAction], id: &str) -> Vec<&'a Action> {
    p.iter().filter(|a| a.resolves.iter().any(|r| r == id)).map(|a| &a.action).collect()
}

#[test]
fn good_setup_needs_no_actions() {
    assert!(plan(&good()).is_empty());
}

#[test]
fn windows_quick_fixes() {
    let mut s = good();
    mic_ep(&mut s).enhancements_enabled = true;
    mic_ep(&mut s).voice_clarity_enabled = true;
    mic_ep(&mut s).mic_boost_db = Some(10.0);
    mic_ep(&mut s).endpoint_volume = 0.5;
    mic_ep(&mut s).exclusive_mode_allowed = true;
    mic_ep(&mut s).listen_to_device = true;
    win(&mut s).ducking = DuckingPreference::MuteOthers;
    win(&mut s).usb_selective_suspend_enabled = true;
    let p = plan(&s);
    let ep = MIC_ID.to_string();
    assert_eq!(find_action(&p, ids::ENHANCEMENTS), vec![&Action::DisableEnhancements { endpoint: ep.clone() }]);
    assert_eq!(find_action(&p, ids::VOICE_CLARITY), vec![&Action::DisableVoiceClarity { endpoint: ep.clone() }]);
    assert_eq!(find_action(&p, ids::MIC_BOOST), vec![&Action::SetMicBoost { endpoint: ep.clone(), db: 0.0 }]);
    assert_eq!(find_action(&p, ids::ENDPOINT_VOLUME), vec![&Action::SetEndpointVolume { endpoint: ep.clone(), volume: 1.0 }]);
    assert_eq!(find_action(&p, ids::EXCLUSIVE_MODE), vec![&Action::DisableExclusiveMode { endpoint: ep.clone() }]);
    assert_eq!(find_action(&p, ids::LISTEN_TO_DEVICE), vec![&Action::DisableListen { endpoint: ep.clone() }]);
    assert_eq!(find_action(&p, ids::DUCKING), vec![&Action::SetWindowsDucking { preference: DuckingPreference::DoNothing }]);
    assert_eq!(find_action(&p, ids::USB_SUSPEND), vec![&Action::DisableUsbSelectiveSuspend]);
    // Guided findings produce no actions.
    let mut s = good();
    win(&mut s).mic_privacy_desktop_allowed = false;
    assert!(plan(&s).is_empty());
}

#[test]
fn sample_rate_action_only_when_obs_wrong() {
    let mut s = good();
    mic_ep(&mut s).sample_rate = 44_100;
    assert!(find_action(&plan(&s), ids::SAMPLE_RATE_MISMATCH).is_empty());
    obs(&mut s).sample_rate = 44_100;
    assert_eq!(find_action(&plan(&s), ids::SAMPLE_RATE_MISMATCH), vec![&Action::SetObsSampleRate { hz: 48_000 }]);
}

#[test]
fn obs_chain_quick_fixes_are_ordered() {
    let mut s = good();
    {
        let m = mic_in(&mut s);
        m.mono_downmix = false;
        m.device_id = "default".into();
        m.monitoring = MonitoringType::MonitorAndOutput;
        m.filters = vec![
            filter("Compressor", "compressor_filter", json!({})),
            filter("Noise Suppression", "noise_suppress_filter_v2", json!({"method": "rnnoise"})),
            filter("Gate", "noise_gate_filter", json!({"open_threshold": -30.0})),
        ];
    }
    let p = plan(&s);
    let kinds: Vec<&str> = p
        .iter()
        .map(|a| match &a.action {
            Action::RemoveObsFilter { .. } => "remove",
            Action::ReorderObsFilters { .. } => "reorder",
            Action::AddObsFilter { .. } => "add",
            _ => "set",
        })
        .collect();
    let first_set = kinds.iter().position(|k| *k == "set").unwrap();
    assert_eq!(kinds[0], "remove");
    assert_eq!(kinds[1], "reorder");
    assert!(first_set > 1);
    assert_eq!(*kinds.last().unwrap(), "add");

    match find_action(&p, ids::FILTER_ORDER)[0] {
        Action::ReorderObsFilters { order, .. } => assert_eq!(order, &vec!["Gate".to_string(), "Compressor".to_string()]),
        a => panic!("{a:?}"),
    }
    assert_eq!(find_action(&p, ids::AI_SUPPRESSION), vec![&Action::RemoveObsFilter { input: "Mic".into(), filter: "Noise Suppression".into() }]);
    assert_eq!(find_action(&p, ids::NOT_MONO), vec![&Action::SetObsInputMono { input: "Mic".into() }]);
    assert_eq!(find_action(&p, ids::OBS_DEFAULT_DEVICE), vec![&Action::PinObsMicDevice { input: "Mic".into(), device_id: MIC_ID.into() }]);
    assert_eq!(find_action(&p, ids::MONITORING_DOUBLED), vec![&Action::SetObsMonitoring { input: "Mic".into(), monitoring: MonitoringType::Off }]);
    match find_action(&p, ids::GATE_CHOPS)[0] {
        Action::SetObsFilterSettings { filter, settings, .. } => {
            assert_eq!(filter, "Gate");
            assert!(settings["open_threshold"].as_f64().unwrap() < -41.0);
        }
        a => panic!("{a:?}"),
    }
    match find_action(&p, ids::COMPRESSOR_EXTREME)[0] {
        Action::SetObsFilterSettings { settings, .. } => assert_eq!(settings["ratio"], 3.5),
        a => panic!("{a:?}"),
    }
    match find_action(&p, ids::LIMITER_MISSING)[0] {
        Action::AddObsFilter { kind, settings, .. } => {
            assert_eq!(kind, "limiter_filter");
            assert_eq!(settings["threshold"], -1.5);
        }
        a => panic!("{a:?}"),
    }
}

#[test]
fn camera_and_encoder_quick_fixes() {
    let mut s = good();
    obs(&mut s).video_captures[0].res_type_custom = false;
    enc(&mut s).encoder_id = "obs_x264".into();
    enc(&mut s).rate_control = "VBR".into();
    enc(&mut s).keyframe_sec = 0;
    enc(&mut s).bitrate_kbps = 9000;
    enc(&mut s).audio_bitrate_kbps = 96;
    obs(&mut s).recording_format = "mp4".into();
    s.network = Some(NetworkInfo { connection: Connection::Wired, measured_upload_kbps: Some(7000) });
    let p = plan(&s);
    assert_eq!(
        find_action(&p, ids::CAM_DEVICE_DEFAULT),
        vec![&Action::SetObsVideoCaptureMode { input: "Webcam".into(), width: 1920, height: 1080, fps: 30.0, format: "MJPEG".into() }]
    );
    assert_eq!(find_action(&p, ids::ENC_CPU_WITH_GPU), vec![&Action::SetObsStreamEncoder { encoder_id: "obs_nvenc_h264_tex".into() }]);
    assert_eq!(find_action(&p, ids::ENC_NOT_CBR), vec![&Action::SetObsRateControl { mode: "CBR".into() }]);
    assert_eq!(find_action(&p, ids::ENC_KEYFRAME), vec![&Action::SetObsKeyframeInterval { seconds: 2 }]);
    assert_eq!(find_action(&p, ids::ENC_AUDIO_LOW), vec![&Action::SetObsAudioBitrate { kbps: 160 }]);
    assert_eq!(find_action(&p, ids::REC_MP4), vec![&Action::SetObsRecordingFormat { format: "hybrid_mp4".into() }]);
    // One bitrate action resolving both platform and upload findings, taking the lower value.
    let br: Vec<&PlannedAction> = p.iter().filter(|a| matches!(a.action, Action::SetObsStreamBitrate { .. })).collect();
    assert_eq!(br.len(), 1);
    assert_eq!(br[0].action, Action::SetObsStreamBitrate { kbps: 5000 }); // 7000*0.75-96 = 5154 → 5000
    assert!(br[0].resolves.contains(&ids::ENC_OVER_PLATFORM.to_string()));
    assert!(br[0].resolves.contains(&ids::ENC_OVER_UPLOAD.to_string()));
    // 5000 kbps at 1080p60 is below target bpp → lower output resolution.
    match find_action(&p, ids::ENC_LOW_FOR_RES)[0] {
        Action::SetObsOutputResolution { width, height } => {
            assert!(*height < 1080);
            assert_eq!(*width % 2, 0);
        }
        a => panic!("{a:?}"),
    }
}

#[test]
fn actions_serialize_with_type_tag() {
    let a = Action::SetObsInputMono { input: "Mic".into() };
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["type"], "set_obs_input_mono");
    let back: Action = serde_json::from_value(v).unwrap();
    assert_eq!(back, a);
}

// ---------------------------------------------------------------- snapshot diff

#[test]
fn diff_unchanged_is_empty() {
    assert!(snapshot_diff(&good(), &good()).is_empty());
}

#[test]
fn diff_default_mic_changed() {
    let prev = good();
    let mut cur = good();
    {
        let w = win(&mut cur);
        w.endpoints[0].is_default = false;
        w.endpoints.push(AudioEndpoint { id: "{webcam-mic}".into(), name: "Microphone (C920)".into(), is_default: true, ..Default::default() });
    }
    let f = snapshot_diff(&prev, &cur);
    let d = f.iter().find(|x| x.id == ids::CHANGED_DEFAULT_MIC).unwrap();
    assert!(d.title.contains("C920"));
}

#[test]
fn diff_obs_mic_missing() {
    let prev = good();
    let mut cur = good();
    win(&mut cur).endpoints.retain(|e| e.id != MIC_ID);
    let f = snapshot_diff(&prev, &cur);
    assert_eq!(f.iter().find(|x| x.id == ids::CHANGED_OBS_MIC_MISSING).unwrap().severity, Severity::Critical);
}

#[test]
fn diff_settings_changed() {
    let prev = good();
    let mut cur = good();
    mic_ep(&mut cur).sample_rate = 44_100;
    mic_ep(&mut cur).enhancements_enabled = true;
    mic_ep(&mut cur).endpoint_volume = 0.7;
    win(&mut cur).ducking = DuckingPreference::Reduce80;
    mic_in(&mut cur).filters.pop();
    cur.camera.as_mut().unwrap().studio_effects_on = true;
    let ids: Vec<String> = snapshot_diff(&prev, &cur).into_iter().map(|f| f.id).collect();
    for id in [
        ids::CHANGED_SAMPLE_RATE,
        ids::CHANGED_ENHANCEMENTS,
        ids::CHANGED_LEVEL,
        ids::CHANGED_DUCKING,
        ids::CHANGED_OBS_FILTERS,
        ids::CHANGED_STUDIO_EFFECTS,
    ] {
        assert!(ids.contains(&id.to_string()), "missing {id}");
    }
}

#[test]
fn diff_enhancement_change_maps_to_action() {
    let prev = good();
    let mut cur = good();
    mic_ep(&mut cur).enhancements_enabled = true;
    let mut findings = snapshot_diff(&prev, &cur);
    findings.retain(|f| f.id == ids::CHANGED_ENHANCEMENTS);
    let p = quick_fixes(&cur, &findings);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].action, Action::DisableEnhancements { endpoint: MIC_ID.into() });
}

#[test]
fn goxlr_official_app_recommends_utility() {
    let mut s = good();
    s.processing.goxlr_app_running = true;
    assert!(has(&s, ids::GOXLR_OFFICIAL_APP));
    let f = crate::system::audit(&s, &[]).into_iter().find(|f| f.id == ids::GOXLR_OFFICIAL_APP).unwrap();
    assert!(f.fix.contains("GoXLR Utility"));
    s.processing.goxlr_utility_running = true;
    assert!(!has(&s, ids::GOXLR_OFFICIAL_APP));
}
