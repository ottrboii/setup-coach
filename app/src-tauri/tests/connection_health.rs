//! Connection health end to end on the mock platform: findings appear for a
//! slow port / dropping cable / app holding the mic, and go away by themselves
//! once the cause is gone (no "did you do it?").

use coach_core::system::{ConnectionState, DeviceHolder, HeldDevice, UsbLink, UsbRole, UsbSpeed};
use setup_coach_app::app::{AppCore, CollectingEmitter};
use setup_coach_app::platform::mock::MockPlatform;
use setup_coach_app::platform::parse_usb_probe;
use std::sync::Arc;

fn core() -> (Arc<AppCore>, MockPlatform, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockPlatform::demo();
    *mock.system.obs_running.lock().unwrap() = false;
    let core = Arc::new(AppCore::new(mock.platform(), dir.path().join("data"), Arc::new(CollectingEmitter::default())));
    (core, mock, dir)
}

fn conn_ids(q: &setup_coach_app::types::QuickChecks) -> Vec<String> {
    q.findings.iter().filter(|f| f.id.starts_with("connection.")).map(|f| f.id.clone()).collect()
}

#[tokio::test]
async fn problems_show_up_and_clear_on_their_own() {
    let (core, mock, _d) = core();
    assert!(conn_ids(&core.run_quick_checks().await).is_empty(), "a quiet PC says nothing");

    *mock.system.connection.lock().unwrap() = ConnectionState {
        links: vec![
            UsbLink { name: "Brio".into(), role: UsbRole::Camera, speed: UsbSpeed::High, superspeed_capable: true, controller: "A".into(), ..Default::default() },
            UsbLink { name: "Yeti".into(), role: UsbRole::Audio, drops_7d: 4, controller: "A".into(), ..Default::default() },
        ],
        holders: vec![DeviceHolder { app: "Zoom".into(), device: HeldDevice::Mic, confirmed: true }],
        ..Default::default()
    };
    let q = core.run_quick_checks().await;
    let ids = conn_ids(&q);
    for want in ["connection.usb3_on_usb2_port", "connection.device_dropouts", "connection.mic_held_by_app"] {
        assert!(ids.contains(&want.to_string()), "missing {want}: {ids:?}");
    }
    let held = q.findings.iter().find(|f| f.id == "connection.mic_held_by_app").unwrap();
    assert!(held.title.contains("Zoom"));

    // Zoom closed, camera moved to a fast port: the next scan is clean.
    *mock.system.connection.lock().unwrap() = ConnectionState {
        links: vec![UsbLink { name: "Brio".into(), role: UsbRole::Camera, speed: UsbSpeed::Super, superspeed_capable: true, ..Default::default() }],
        holders: vec![],
        ..Default::default()
    };
    let ids = conn_ids(&core.run_quick_checks().await);
    assert!(!ids.contains(&"connection.usb3_on_usb2_port".to_string()));
    assert!(!ids.contains(&"connection.mic_held_by_app".to_string()));
}

#[tokio::test]
async fn camera_body_that_is_not_a_webcam_is_named_and_clears_when_it_becomes_one() {
    let (core, mock, _d) = core();
    *mock.system.connection.lock().unwrap() = ConnectionState {
        links: vec![UsbLink { name: "Canon EOS M50".into(), role: UsbRole::Camera, vid: 0x04A9, image_only: true, ..Default::default() }],
        vendor_tools_running: vec!["EOS Utility".into()],
        ..Default::default()
    };
    let q = core.run_quick_checks().await;
    let f = q.findings.iter().find(|f| f.id == "connection.camera_body_not_webcam").expect("body finding");
    assert!(f.fix.contains("EOS Utility is running"));
    // Webcam mode now on: Windows lists it as a camera, the finding is gone.
    *mock.system.connection.lock().unwrap() = ConnectionState {
        links: vec![UsbLink { name: "EOS Webcam Utility".into(), role: UsbRole::Camera, vid: 0x04A9, image_only: false, ..Default::default() }],
        ..Default::default()
    };
    assert!(!conn_ids(&core.run_quick_checks().await).contains(&"connection.camera_body_not_webcam".to_string()));
}

#[test]
fn probe_reads_body_and_run_times() {
    let j = r#"[{"name":"EOS M50","role":"camera","vid":1193,"pid":12345,"image_only":true,"run_minutes":[27,30]},{"name":"Mic","role":"audio","run_minutes":[]}]"#;
    let d = parse_usb_probe(j);
    assert_eq!(d.len(), 2);
    assert!(d[0].image_only && d[0].run_minutes == vec![27, 30]);
    assert!(!d[1].image_only && d[1].run_minutes.is_empty());
}

#[test]
fn probe_output_parses_and_garbage_is_ignored() {
    let real = r#"[{"name":"C922 Pro Stream Webcam","role":"camera","also_audio":true,"vid":1133,"pid":2140,"speed":"high","superspeed_capable":null,"controller":"PCIROOT(0)#PCI(0803)#PCI(0000)","behind_hub":true,"hub_bus_powered":false,"hub_name":"Generic USB Hub","drops_7d":2}]"#;
    let d = parse_usb_probe(real);
    assert_eq!(d.len(), 1);
    assert_eq!((d[0].vid, d[0].pid, d[0].speed.as_str(), d[0].drops_7d), (0x046D, 0x085C, "high", 2));
    assert!(d[0].superspeed_capable.is_none());
    // A lone object, a BOM, an empty list and noise.
    assert_eq!(parse_usb_probe(&format!("\u{feff}{}", &real[1..real.len() - 1])).len(), 1);
    assert!(parse_usb_probe("[]").is_empty());
    assert!(parse_usb_probe("Access is denied").is_empty());
}
