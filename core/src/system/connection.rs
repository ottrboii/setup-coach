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
        SystemSnapshot { connection: Some(ConnectionState { links, holders }), ..Default::default() }
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
}
