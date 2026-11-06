//! Connection health: USB link speed and topology, devices that keep
//! dropping out, and apps that are holding the camera or mic.
//!
//! Input is [`ConnectionState`], collected by the Windows app. Everything is
//! a physical fix first (other port, other cable, no hub); numbers only land
//! in `evidence` (the Advanced drawer).

use super::ids;
use super::state::SystemSnapshot;
use crate::finding::{Area, Finding, FixKind, Severity};
use serde::{Deserialize, Serialize};

/// What a USB device is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsbRole {
    Camera,
    #[default]
    Audio,
}

/// Link speed the device is running at right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsbSpeed {
    #[default]
    Unknown,
    Low,
    Full,
    /// USB 2.0 (480 Mbit/s).
    High,
    /// USB 3.x (5 Gbit/s and up).
    Super,
}

/// One USB camera / capture card / mic as Windows sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UsbLink {
    pub name: String,
    pub role: UsbRole,
    pub vid: u16,
    pub pid: u16,
    pub speed: UsbSpeed,
    /// The device can run at USB 3 speed (reported by the hub or known from its profile).
    pub superspeed_capable: bool,
    /// Which USB controller (host) it hangs off; devices with the same value share bandwidth.
    pub controller: String,
    /// A hub sits between the device and the PC.
    pub behind_hub: bool,
    pub hub_name: Option<String>,
    /// The hub has no power supply of its own (bus-powered).
    pub hub_bus_powered: bool,
    /// Times Windows removed and re-added the device in the last 7 days.
    pub drops_7d: u32,
    /// Windows only sees it as a stills device (photo transfer), not as a
    /// webcam: a camera body whose webcam mode is not running.
    pub image_only: bool,
    /// Minutes the device stayed connected before each removal in the last
    /// 7 days (oldest first). Used to spot a camera that shuts off after the
    /// same time every time.
    pub run_minutes: Vec<u32>,
}

/// Which device an app is holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldDevice {
    Camera,
    Mic,
}

/// An app that has the camera or mic open right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHolder {
    /// Plain name, e.g. "Microsoft Teams".
    pub app: String,
    pub device: HeldDevice,
    /// Windows says it is using the device right now (false = running and
    /// known to grab it, can't tell).
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConnectionState {
    pub links: Vec<UsbLink>,
    pub holders: Vec<DeviceHolder>,
    /// Camera-maker tools that are running and grab camera bodies
    /// (e.g. `EOS Utility`, `Imaging Edge`), by plain name.
    pub vendor_tools_running: Vec<String>,
}

/// Apps that holding a device is expected and fine: our own tools.
fn is_expected_holder(app: &str) -> bool {
    matches!(app, "OBS Studio" | "Streamlabs" | "Setup Coach")
}

pub(crate) fn check(s: &SystemSnapshot, out: &mut Vec<Finding>) {
    let Some(c) = &s.connection else { return };

    for l in &c.links {
        let what = if l.role == UsbRole::Camera { "camera" } else { "mic" };

        // USB 3 device running at USB 2 speed.
        if l.role == UsbRole::Camera && l.superspeed_capable && l.speed != UsbSpeed::Unknown && l.speed < UsbSpeed::Super {
            let mut f = Finding::new(
                ids::CONN_SLOW_PORT,
                Area::Camera,
                Severity::Major,
                &format!("Your {} is plugged into a slow USB port", l.name),
                "This camera can use the fast (blue) USB 3 connection but is running on the slow one, so it falls back to a lower quality or choppier picture.",
                "Move the camera's plug to a blue USB 3 port straight on the back of your PC, and use the cable that came with it. Avoid hubs and extension cables.",
                FixKind::Manual,
            );
            f.evidence.insert("speed_mbps".into(), speed_mbps(l.speed));
            out.push(f);
        }

        // Behind a hub.
        if l.behind_hub {
            let hub = l.hub_name.as_deref().unwrap_or("a USB hub");
            if l.hub_bus_powered {
                out.push(Finding::new(
                    ids::CONN_HUB,
                    if l.role == UsbRole::Camera { Area::Camera } else { Area::Mic },
                    Severity::Major,
                    &format!("Your {what} shares power through an unpowered hub"),
                    &format!("{} is plugged into {hub}, which has no power supply of its own. Cameras and mics cut out or glitch when the hub can't give them enough power.", l.name),
                    "Plug it straight into a USB port on the back of your PC. If you need a hub, use one with its own power adapter.",
                    FixKind::Manual,
                ));
            } else if l.role == UsbRole::Camera {
                out.push(Finding::new(
                    ids::CONN_HUB,
                    Area::Camera,
                    Severity::Minor,
                    "Your camera is plugged in through a hub",
                    &format!("{} goes through {hub}. Hubs share speed between everything plugged in, which can make video stutter or drop out.", l.name),
                    "If you see stutter or dropouts, plug the camera straight into a port on the back of your PC.",
                    FixKind::Manual,
                ));
            }
        }

        // Dropping out.
        if l.drops_7d >= 1 {
            let (sev, how) = if l.drops_7d >= 3 {
                (Severity::Major, "keeps")
            } else {
                (Severity::Minor, "has")
            };
            let mut f = Finding::new(
                ids::CONN_DROPOUTS,
                if l.role == UsbRole::Camera { Area::Camera } else { Area::Mic },
                sev,
                &format!("Your {what} {how} disconnecting"),
                &format!("Windows lost and re-found {} more than once recently. That is almost always a worn cable, a loose plug or a weak USB port, not a software problem.", l.name),
                "Swap the cable first (a short, good one, not a charge-only cable), then try another port directly on the PC. Keep it off hubs and loose extension cables.",
                FixKind::Manual,
            );
            f.evidence.insert("drops_7d".into(), f64::from(l.drops_7d));
            out.push(f);
        }
    }

    body_not_webcam(c, out);
    runtime_limit(c, out);

    // Two cameras / capture cards on one controller.
    let video: Vec<&UsbLink> = c.links.iter().filter(|l| l.role == UsbRole::Camera && !l.controller.is_empty()).collect();
    for (i, a) in video.iter().enumerate() {
        if let Some(b) = video.iter().skip(i + 1).find(|b| b.controller == a.controller && b.name != a.name) {
            let mut f = Finding::new(
                ids::CONN_SHARED_CONTROLLER,
                Area::Camera,
                Severity::Major,
                "Two cameras are sharing one USB connection",
                &format!("{} and {} hang off the same USB controller, so they split its speed. One of them can freeze or go black when both run.", a.name, b.name),
                "Plug one of them into a port on a different part of the PC (try the other side of the back panel, or the front ports).",
                FixKind::Manual,
            );
            f.evidence.insert("devices_on_controller".into(), 2.0);
            out.push(f);
            break;
        }
    }

    // Apps holding the camera or mic.
    for h in c.holders.iter().filter(|h| h.confirmed && !is_expected_holder(&h.app)) {
        let (area, what, id) = match h.device {
            HeldDevice::Camera => (Area::Camera, "camera", ids::CONN_CAMERA_HELD),
            HeldDevice::Mic => (Area::Mic, "mic", ids::CONN_MIC_HELD),
        };
        out.push(Finding::new(
            id,
            area,
            Severity::Major,
            &format!("{} is using your {what}", h.app),
            &format!("While {} has your {what} open, other apps can get a black picture, no sound, or a \"device in use\" message.", h.app),
            &format!("Close {} (check the system tray near the clock too), then we'll check again.", h.app),
            FixKind::Guided,
        ));
    }
}

/// Camera makers by USB vendor id: (id, plain maker name, its webcam tool).
const CAMERA_MAKERS: [(u16, &str, &str); 8] = [
    (0x04A9, "Canon", "EOS Webcam Utility"),
    (0x054C, "Sony", "Imaging Edge Webcam"),
    (0x04B0, "Nikon", "Nikon Webcam Utility"),
    (0x04CB, "Fujifilm", "Fujifilm X Webcam"),
    (0x04DA, "Panasonic", "LUMIX Webcam Software"),
    (0x07B4, "OM System", "OM Workspace"),
    (0x2672, "GoPro", "GoPro Webcam mode"),
    (0x2CA3, "DJI", "DJI's webcam mode"),
];

/// A camera body is plugged in, but Windows does not see a webcam.
fn body_not_webcam(c: &ConnectionState, out: &mut Vec<Finding>) {
    for l in c.links.iter().filter(|l| l.role == UsbRole::Camera && l.image_only) {
        let Some((_, maker, tool)) = CAMERA_MAKERS.iter().find(|m| m.0 == l.vid) else { continue };
        let tools = if c.vendor_tools_running.is_empty() {
            String::new()
        } else {
            format!(" {} is running and may be holding the camera: close it (and its icon in the system tray) first.", c.vendor_tools_running.join(" and "))
        };
        out.push(Finding::new(
            ids::CONN_BODY_NOT_WEBCAM,
            Area::Camera,
            Severity::Major,
            &format!("Your {maker} camera is connected but isn't acting as a webcam"),
            &format!("Windows sees {} as a photo camera only, so OBS and other apps can't pick it as a webcam.", l.name),
            &format!("Check the cable is a data cable (not charge-only) in a port on the back of your PC, turn the camera's Wi-Fi and Bluetooth off, and set its USB mode to webcam or PC remote if it has the choice.{tools} Then install or update {tool} (we'll open the download page for you) and replug the camera."),
            FixKind::Guided,
        ));
    }
}

/// Shortest and longest connected run (minutes) that still counts as
/// "shut off by heat or battery". Starting values from single sources
/// (owner reports of about 27 to 30 minutes); no lab data yet.
const RUNTIME_MIN: u32 = 10;
const RUNTIME_MAX: u32 = 60;
/// Runs this close together (longest / shortest) count as "same time every time".
const RUNTIME_SPREAD: f64 = 1.4;

/// Typical run time (minutes) if a camera keeps stopping after about the
/// same time connected: at least two runs, all inside the window, and close
/// together. Anything messier is a flaky cable or port, not heat.
pub fn typical_runtime_minutes(run_minutes: &[u32]) -> Option<u32> {
    if run_minutes.len() < 2 || run_minutes.iter().any(|m| !(RUNTIME_MIN..=RUNTIME_MAX).contains(m)) {
        return None;
    }
    let (lo, hi) = (*run_minutes.iter().min()?, *run_minutes.iter().max()?);
    if f64::from(hi) > f64::from(lo) * RUNTIME_SPREAD {
        return None;
    }
    Some(run_minutes.iter().sum::<u32>() / run_minutes.len() as u32)
}

/// True once a running camera has used up most of its usual time (80 %), so
/// the app can warn before it cuts out mid-stream.
pub fn near_runtime_limit(typical_min: u32, running_min: u32) -> bool {
    typical_min > 0 && f64::from(running_min) >= f64::from(typical_min) * 0.8
}

fn runtime_limit(c: &ConnectionState, out: &mut Vec<Finding>) {
    for l in c.links.iter().filter(|l| l.role == UsbRole::Camera) {
        let Some(typical) = typical_runtime_minutes(&l.run_minutes) else { continue };
        let mut f = Finding::new(
            ids::CONN_RUNTIME_LIMIT,
            Area::Camera,
            Severity::Major,
            &format!("Your {} keeps switching off after about the same time", l.name),
            "It stopped each time after roughly the same length of time connected. That points to the camera overheating or its battery or auto power-off running out, not a bad cable.",
            "Power it from the wall with a dummy battery, turn the camera's auto power-off off, give it airflow, and use 720p if it has the choice. If it still stops, use the HDMI output with a capture card instead of USB streaming.",
            FixKind::Manual,
        );
        f.evidence.insert("typical_run_minutes".into(), f64::from(typical));
        f.evidence.insert("runs".into(), l.run_minutes.len() as f64);
        out.push(f);
    }
}

fn speed_mbps(s: UsbSpeed) -> f64 {
    match s {
        UsbSpeed::Low => 1.5,
        UsbSpeed::Full => 12.0,
        UsbSpeed::High => 480.0,
        UsbSpeed::Super => 5000.0,
        UsbSpeed::Unknown => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::audit;

    fn snap(links: Vec<UsbLink>, holders: Vec<DeviceHolder>) -> SystemSnapshot {
        SystemSnapshot { connection: Some(ConnectionState { links, holders, ..Default::default() }), ..Default::default() }
    }
    fn cam(name: &str, speed: UsbSpeed, capable: bool) -> UsbLink {
        UsbLink { name: name.into(), role: UsbRole::Camera, speed, superspeed_capable: capable, controller: "PCI(1)".into(), ..Default::default() }
    }
    fn ids_of(s: &SystemSnapshot) -> Vec<String> {
        audit(s, &[]).into_iter().map(|f| f.id).collect()
    }

    #[test]
    fn healthy_connection_is_quiet() {
        let s = snap(vec![cam("Brio", UsbSpeed::Super, true)], vec![DeviceHolder { app: "OBS Studio".into(), device: HeldDevice::Camera, confirmed: true }]);
        assert!(ids_of(&s).iter().all(|i| !i.starts_with("connection.")));
    }

    #[test]
    fn usb3_camera_on_usb2_port() {
        let ids = ids_of(&snap(vec![cam("Brio", UsbSpeed::High, true)], vec![]));
        assert!(ids.contains(&ids::CONN_SLOW_PORT.to_string()));
        // Same camera at full speed, or one that was never USB 3: nothing.
        assert!(ids_of(&snap(vec![cam("C922", UsbSpeed::High, false)], vec![])).is_empty());
        // Unknown speed is never accused.
        assert!(ids_of(&snap(vec![cam("Brio", UsbSpeed::Unknown, true)], vec![])).is_empty());
    }

    #[test]
    fn unpowered_hub_is_major_plain_hub_only_nags_cameras() {
        let mut mic = UsbLink { name: "Yeti".into(), role: UsbRole::Audio, behind_hub: true, hub_bus_powered: true, ..Default::default() };
        let f = audit(&snap(vec![mic.clone()], vec![]), &[]);
        assert!(f.iter().any(|f| f.id == ids::CONN_HUB && f.severity == Severity::Major && f.area == Area::Mic));
        mic.hub_bus_powered = false;
        assert!(ids_of(&snap(vec![mic], vec![])).is_empty());
        let mut c = cam("C922", UsbSpeed::High, false);
        c.behind_hub = true;
        let f = audit(&snap(vec![c], vec![]), &[]);
        assert!(f.iter().any(|f| f.id == ids::CONN_HUB && f.severity == Severity::Minor));
    }

    #[test]
    fn dropouts_scale_with_count_and_carry_numbers_only_as_evidence() {
        let mut m = UsbLink { name: "Yeti".into(), role: UsbRole::Audio, drops_7d: 1, ..Default::default() };
        let f = audit(&snap(vec![m.clone()], vec![]), &[]);
        let d = f.iter().find(|f| f.id == ids::CONN_DROPOUTS).unwrap();
        assert_eq!(d.severity, Severity::Minor);
        assert!(!d.title.chars().any(|c| c.is_ascii_digit()) && !d.fix.chars().any(|c| c.is_ascii_digit()));
        m.drops_7d = 5;
        let f = audit(&snap(vec![m], vec![]), &[]);
        let d = f.iter().find(|f| f.id == ids::CONN_DROPOUTS).unwrap();
        assert_eq!(d.severity, Severity::Major);
        assert_eq!(d.evidence["drops_7d"], 5.0);
        assert_eq!(d.fix_kind, FixKind::Manual);
        assert!(d.can_decline);
    }

    #[test]
    fn two_cameras_on_one_controller() {
        let ids = ids_of(&snap(vec![cam("Cam Link 4K", UsbSpeed::Super, true), cam("C922", UsbSpeed::High, false)], vec![]));
        assert!(ids.contains(&ids::CONN_SHARED_CONTROLLER.to_string()));
        let mut other = cam("C922", UsbSpeed::High, false);
        other.controller = "PCI(2)".into();
        assert!(!ids_of(&snap(vec![cam("Cam Link 4K", UsbSpeed::Super, true), other], vec![])).contains(&ids::CONN_SHARED_CONTROLLER.to_string()));
    }

    #[test]
    fn names_the_app_holding_the_camera_or_mic() {
        let s = snap(
            vec![],
            vec![
                DeviceHolder { app: "Microsoft Teams".into(), device: HeldDevice::Camera, confirmed: true },
                DeviceHolder { app: "Zoom".into(), device: HeldDevice::Mic, confirmed: true },
                DeviceHolder { app: "Discord".into(), device: HeldDevice::Mic, confirmed: false },
                DeviceHolder { app: "OBS Studio".into(), device: HeldDevice::Mic, confirmed: true },
            ],
        );
        let f = audit(&s, &[]);
        let cam = f.iter().find(|f| f.id == ids::CONN_CAMERA_HELD).unwrap();
        assert!(cam.title.contains("Microsoft Teams") && cam.fix.contains("Microsoft Teams"));
        assert_eq!(cam.fix_kind, FixKind::Guided);
        let mics: Vec<_> = f.iter().filter(|f| f.id == ids::CONN_MIC_HELD).collect();
        assert_eq!(mics.len(), 1);
        assert!(mics[0].title.contains("Zoom"));
    }

    #[test]
    fn finding_clears_when_the_cause_is_gone() {
        // Completion is detected by re-scanning, never by asking.
        let before = snap(vec![], vec![DeviceHolder { app: "Zoom".into(), device: HeldDevice::Camera, confirmed: true }]);
        assert!(ids_of(&before).contains(&ids::CONN_CAMERA_HELD.to_string()));
        assert!(ids_of(&snap(vec![], vec![])).is_empty());
    }

    fn body(vid: u16, image_only: bool) -> UsbLink {
        UsbLink { name: "EOS M50".into(), role: UsbRole::Camera, vid, image_only, ..Default::default() }
    }

    #[test]
    fn camera_body_seen_as_stills_only_gets_webcam_help() {
        let mut s = snap(vec![body(0x04A9, true)], vec![]);
        let f = audit(&s, &[]);
        let b = f.iter().find(|f| f.id == ids::CONN_BODY_NOT_WEBCAM).unwrap();
        assert!(b.title.contains("Canon") && b.fix.contains("Wi-Fi") && b.fix.contains("EOS Webcam Utility"));
        assert!(!b.fix.contains("EOS Utility is running"));
        // The conflicting maker tool is named when it is running.
        s.connection.as_mut().unwrap().vendor_tools_running = vec!["EOS Utility".into()];
        let f = audit(&s, &[]);
        assert!(f.iter().find(|f| f.id == ids::CONN_BODY_NOT_WEBCAM).unwrap().fix.contains("EOS Utility is running"));
        // Already a webcam, or not a camera maker: nothing.
        assert!(ids_of(&snap(vec![body(0x04A9, false)], vec![])).is_empty());
        assert!(ids_of(&snap(vec![body(0x046D, true)], vec![])).is_empty());
    }

    #[test]
    fn same_run_time_every_time_means_heat_not_cable() {
        assert_eq!(typical_runtime_minutes(&[27, 30, 29]), Some(28));
        // One run, wild spread, too short or too long: not a heat pattern.
        assert_eq!(typical_runtime_minutes(&[28]), None);
        assert_eq!(typical_runtime_minutes(&[12, 45, 30]), None);
        assert_eq!(typical_runtime_minutes(&[2, 3, 2]), None);
        assert_eq!(typical_runtime_minutes(&[120, 118]), None);
        let mut c = cam("Alpha body", UsbSpeed::Super, true);
        c.run_minutes = vec![27, 30];
        let f = audit(&snap(vec![c.clone()], vec![]), &[]);
        let r = f.iter().find(|f| f.id == ids::CONN_RUNTIME_LIMIT).unwrap();
        assert_eq!(r.evidence["typical_run_minutes"], 28.0);
        assert!(!r.title.chars().any(|c| c.is_ascii_digit()));
        c.run_minutes = vec![3, 40, 15];
        assert!(!ids_of(&snap(vec![c], vec![])).contains(&ids::CONN_RUNTIME_LIMIT.to_string()));
    }

    #[test]
    fn warns_before_the_usual_limit() {
        assert!(!near_runtime_limit(30, 10));
        assert!(near_runtime_limit(30, 24));
        assert!(!near_runtime_limit(0, 99));
    }
}