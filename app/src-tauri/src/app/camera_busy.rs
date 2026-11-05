//! "We can't see your camera": checked frames, who holds the camera, freeing
//! it from OBS (and putting that back), and the mains-frequency setting.
//!
//! A webcam can be opened by one app at a time. When another app holds it we
//! get black frames, a flat picture, an open error or a hang. None of that is
//! ever analysed (see `coach_core::video::guard`).

use super::vault_ops::Target;
use super::*;
use crate::platform::{MainsGuess, MainsSource};
use crate::types::{FreeCameraResult, MainsInfo, ObsCameraHold, PictureReason, PictureStatus};
use coach_core::video::{check_frame, MainsHz, NoPicture};

pub(super) fn reason_of(np: NoPicture) -> PictureReason {
    match np {
        NoPicture::Black => PictureReason::Black,
        NoPicture::Blank => PictureReason::Blank,
        NoPicture::NoFrames => PictureReason::CantOpen,
    }
}

impl AppCore {
    /// The OBS video capture source that uses this camera, if OBS is connected.
    pub(super) async fn camera_obs_source(&self, camera_id: &str) -> Option<String> {
        let o = self.obs().await?;
        let name = self.platform.camera.list().into_iter().find(|c| c.id == camera_id).map(|c| c.name).unwrap_or_default();
        obs::read_state(o.as_ref(), self.obs_dir().as_deref()).await.ok().and_then(|s| obs_source_for_camera(&s, &name))
    }

    /// One frame, or why there is no usable picture. Tries OBS' screenshot of
    /// the camera source first (OBS may hold the camera), then the camera
    /// itself with a time limit. Black or flat frames count as "no picture".
    pub(super) async fn grab_checked(&self, camera_id: &str, source: Option<&str>) -> Result<video::Frame, PictureReason> {
        let mut last = PictureReason::CantOpen;
        if let (Some(src), Some(o)) = (source, self.obs().await) {
            if let Ok(f) = obs::screenshot(o.as_ref(), src, 640).await.and_then(|b| camera::decode_image(&b, 0.0)) {
                match check_frame(&f) {
                    None => return Ok(f),
                    Some(np) => last = reason_of(np),
                }
            }
        }
        let cam = self.platform.camera.clone();
        let id = camera_id.to_string();
        let job = tokio::task::spawn_blocking(move || cam.grab_frame(&id));
        match tokio::time::timeout(self.grab_timeout, job).await {
            // The blocked read is abandoned: the camera is not answering.
            Err(_) => Err(PictureReason::Timeout),
            Ok(Ok(Ok(f))) => match check_frame(&f) {
                None => Ok(f),
                Some(np) => Err(reason_of(np)),
            },
            Ok(_) => Err(last),
        }
    }

    /// Tell the UI when the picture is lost or comes back (only on change).
    pub(super) fn note_picture(&self, lost: Option<PictureReason>) {
        let live = lost.is_none();
        {
            let mut i = self.inner.lock().unwrap();
            if i.picture_live == Some(live) {
                return;
            }
            i.picture_live = Some(live);
        }
        self.emitter.emit(crate::types::events::CAMERA_PICTURE, json!({ "live": live, "reason": lost }));
    }

    /// Can we see the camera right now, and if not, who has it?
    pub async fn camera_picture_status(&self, camera_id: &str) -> PictureStatus {
        let running = self.platform.system.obs_running();
        if running && self.obs().await.is_none() {
            // OBS is open: try its WebSocket (settings from OBS' own config).
            let _ = self.connect_obs(None).await;
        }
        let connected = self.obs().await.is_some();
        let source = self.camera_obs_source(camera_id).await;
        let probe = self.grab_checked(camera_id, source.as_deref()).await;
        self.note_picture(probe.as_ref().err().copied());
        let holding = match (&source, self.obs().await) {
            (Some(s), Some(o)) => o.request("GetSourceActive", json!({ "sourceName": s })).await.ok().and_then(|r| r["videoActive"].as_bool()).unwrap_or(false),
            _ => false,
        };
        let sys = self.platform.system.clone();
        let users = tokio::task::spawn_blocking(move || sys.camera_users()).await.unwrap_or_default();
        let freed_source = self.inner.lock().unwrap().freed_camera.clone();
        PictureStatus {
            live: probe.is_ok(),
            reason: probe.err(),
            users,
            obs: ObsCameraHold { running, connected, can_free: connected && source.is_some() && holding, source, holding },
            freed_source,
        }
    }

    /// "Free it for me": release the camera from OBS' video capture source
    /// (the source's own Deactivate button), after backing the state up in the
    /// vault. `restore_freed_camera` presses it again.
    pub async fn free_camera(&self, camera_id: &str) -> FreeCameraResult {
        let fail = |m: &str, source: Option<String>| FreeCameraResult { ok: false, source, message: m.to_string() };
        if self.obs().await.is_none() && self.platform.system.obs_running() {
            let _ = self.connect_obs(None).await;
        }
        let Some(o) = self.obs().await else {
            return fail("We can't reach OBS. Close OBS, or turn on its WebSocket server (Tools > WebSocket Server Settings), and we'll carry on.", None);
        };
        let Some(src) = self.camera_obs_source(camera_id).await else {
            return fail("We couldn't find which OBS source uses your camera. Close OBS and we'll carry on.", None);
        };
        let active = |o: Arc<dyn ObsApi>, s: String| async move { o.request("GetSourceActive", json!({ "sourceName": s })).await.ok().and_then(|r| r["videoActive"].as_bool()) };
        match active(o.clone(), src.clone()).await {
            Some(false) => return fail("OBS isn't holding your camera. Another app is: close it and we'll carry on.", Some(src)),
            None => return fail("OBS didn't answer. Close OBS and we'll carry on.", Some(src)),
            Some(true) => {}
        }
        if let Err(e) = self.backup_before("Before freeing your camera from OBS", &[Target::ObsSourceActive(src.clone())]).await {
            return fail(&e, Some(src));
        }
        if let Err(e) = o.request("PressInputPropertiesButton", json!({ "inputName": src, "propertyName": "activate" })).await {
            return fail(&format!("OBS wouldn't let go of the camera ({e}). Close OBS and we'll carry on."), Some(src));
        }
        // Give the driver a moment to let go, then check.
        for _ in 0..10 {
            if active(o.clone(), src.clone()).await == Some(false) {
                self.inner.lock().unwrap().freed_camera = Some(src.clone());
                return FreeCameraResult { ok: true, source: Some(src), message: "Your camera is free. We'll give it back to OBS when we're done.".into() };
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Still held: put the button back to where it was and say so.
        let _ = o.request("PressInputPropertiesButton", json!({ "inputName": src, "propertyName": "activate" })).await;
        fail("OBS didn't let go of the camera. Close OBS and we'll carry on.", Some(src))
    }

    /// Give the camera back to OBS if we freed it. Safe to call any time.
    pub async fn restore_freed_camera(&self) -> FreeCameraResult {
        let Some(src) = self.inner.lock().unwrap().freed_camera.clone() else {
            return FreeCameraResult { ok: true, source: None, message: "Nothing to give back.".into() };
        };
        let Some(o) = self.obs().await else {
            // OBS was closed in the meantime: it opens its sources fresh anyway.
            self.inner.lock().unwrap().freed_camera = None;
            return FreeCameraResult { ok: true, source: Some(src), message: "OBS was closed, so it will take the camera back when it starts.".into() };
        };
        let now = o.request("GetSourceActive", json!({ "sourceName": src })).await.ok().and_then(|r| r["videoActive"].as_bool());
        let result = match now {
            Some(false) => o.request("PressInputPropertiesButton", json!({ "inputName": src, "propertyName": "activate" })).await.map(|_| ()),
            _ => Ok(()),
        };
        match result {
            Ok(()) => {
                self.inner.lock().unwrap().freed_camera = None;
                FreeCameraResult { ok: true, source: Some(src), message: "Your camera is back in OBS.".into() }
            }
            Err(e) => FreeCameraResult {
                ok: false,
                source: Some(src.clone()),
                message: format!("We couldn't hand the camera back to OBS ({e}). In OBS, open \"{src}\" properties and press Activate, or use Backups to undo."),
            },
        }
    }

    // ------------------------------------------------------------ mains frequency

    fn prefs_path(&self) -> PathBuf {
        self.data_dir.join("prefs.json")
    }

    fn mains_override(&self) -> Option<MainsHz> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(self.prefs_path()).ok()?).ok()?;
        match v["mains_hz"].as_u64()? {
            50 => Some(MainsHz::Hz50),
            60 => Some(MainsHz::Hz60),
            _ => None,
        }
    }

    /// Power-grid frequency used for anti-flicker: the user's choice under
    /// Advanced, else the time zone, else the region, else 50 Hz.
    pub fn mains(&self) -> MainsHz {
        self.mains_override().unwrap_or_else(|| self.platform.system.mains())
    }

    pub fn mains_info(&self) -> MainsInfo {
        let guess: MainsGuess = self.platform.system.mains_guess();
        match self.mains_override() {
            Some(hz) => MainsInfo { hz: hz.as_u8(), source: MainsSource::Override, detected_hz: guess.hz.as_u8() },
            None => MainsInfo { hz: guess.hz.as_u8(), source: guess.source, detected_hz: guess.hz.as_u8() },
        }
    }

    /// Set (50 or 60) or clear (None) the user's mains frequency.
    pub fn set_mains(&self, hz: Option<u8>) -> Result<MainsInfo, String> {
        if !matches!(hz, None | Some(50) | Some(60)) {
            return Err("Mains frequency is 50 or 60 Hz.".into());
        }
        let path = self.prefs_path();
        let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        match hz {
            Some(h) => v["mains_hz"] = json!(h),
            None => {
                v.as_object_mut().map(|o| o.remove("mains_hz"));
            }
        }
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())?;
        Ok(self.mains_info())
    }
}
