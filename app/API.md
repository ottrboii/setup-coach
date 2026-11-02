# Setup Coach app: backend ⇄ UI contract

Tauri 2. Backend: `app/src-tauri` (Rust, uses `coach_core`). UI: `app/ui`
(static HTML/CSS/JS ES modules, no build step; `frontendDist = "../ui"`).

JS calls commands with `window.__TAURI__.core.invoke(name, args)`; argument
keys are **camelCase** in JS (Tauri converts to the Rust snake_case params).
Events arrive via `window.__TAURI__.event.listen(name, cb)`.

All shared data types are the serde JSON of `coach_core` types (snake_case
fields, enums `rename_all = "snake_case"`): `Finding`, `Score`, `MicReport`,
`ProcessingChain`, `AudioMetrics`, `TargetCheck`, `Constraint`,
`video::{VideoMetrics, CameraSettings}`, `system::{SystemSnapshot, PlannedAction}`,
`health::{Alert, StreamReport}`, `profiles::MatchResult`.
App-only types are defined in `src-tauri/src/types.rs` and listed below.

Every command returns `Result<T, String>`; errors are plain-language strings
safe to show the user.

## Startup / environment

| Command | Args | Returns |
|---|---|---|
| `get_environment` | – | `Environment` |
| `connect_obs` | `{ password?: string }` | `ObsStatus` |

```ts
Environment = {
  app_version: string, windows_build: string | null,
  obs: ObsStatus,
  mics: AudioDeviceInfo[], cameras: CameraDeviceInfo[],
  saved_profile: SavedProfile | null   // last applied setup, re-applied on start
}
ObsStatus = { running: boolean, connected: boolean, version: string | null,
              websocket_enabled: boolean | null, error: string | null }
AudioDeviceInfo = { id: string, name: string, is_default: boolean,
                    is_default_comms: boolean, channels: number, sample_rate: number,
                    profile: ProfileMatch | null,
                    obs_input: string | null }   // OBS audio input capturing this device
CameraDeviceInfo = { id: string, name: string, obs_source: string | null,
                     profile: ProfileMatch | null }
// Owned view of core's MatchResult (which borrows the profile database).
ProfileMatch = { kind: "mic" | "interface" | "camera", profile_id: string,
                 display_name: string, confidence: "exact" | "name_match" | "generic",
                 verified: boolean, questions: Question[],      // core profiles::Question
                 interface: ProfileMatch | null,                 // mic behind an interface
                 profile: object }                               // full profile JSON
```

`get_environment` auto-connects to OBS when possible: it reads the websocket
port/password from `%APPDATA%\obs-studio\plugin_config\obs-websocket\config.json`
(user consent is shown in the UI before first use).

## Quick checks (the free scan)

| Command | Args | Returns |
|---|---|---|
| `run_quick_checks` | – | `QuickChecks` |
| `apply_actions` | `{ actionIds: string[] }` | `ApplyResult` |

```ts
QuickChecks = { findings: Finding[], actions: PlannedActionView[], score: Score[] }
PlannedActionView = { id: string, label: string, resolves: string[], needs_admin: boolean,
                      needs_obs_closed: boolean, action: Action }   // Action = coach_core::system Action
ApplyResult = { applied: string[], failed: { id: string, error: string }[], restart_obs_needed: boolean }
```

## Mic test

| Command | Args | Returns |
|---|---|---|
| `mic_context` | `{ deviceId: string, answers: MicAnswers }` | `MicContext` (core) |
| `start_mic_test` | `{ deviceId: string }` | `()` – starts capture; emits `mic-level` |
| `mark_step` | `{ kind: SegmentKind, edge: "start" \| "end" }` | `()` |
| `finish_mic_test` | `{ context: MicContext, constraints: Constraint[] }` | `MicReport` (core) |
| `rescan_mic` | `{ context: MicContext, constraints: Constraint[] }` | `MicReport` – re-uses the last recording (after "I can't") |
| `apply_mic_chain` | `{ chain: ProcessingChain, obsInput: string }` | `ApplyResult` |
| `verify_mic` | – | `{ before: MicReport, after: MicReport }` – renders the last recording through the applied chain and re-analyses |
| `play_preview` | `{ which: "before" \| "after", phone: boolean }` | `()` |
| `stop_preview` | – | `()` |

```ts
MicAnswers = { mic_model_id?: string, booster?: boolean, gain_at_max?: boolean,
               onboard_auto_level?: boolean }
SegmentKind = "silence" | "read" | "loud" | "typing" | "desk_tap" | "cable_wiggle" | "moving"
```
Event `mic-level`: `{ rms_db: number, peak_db: number, clipped: boolean }` (~20/s).

Guided steps & durations (UI drives them, backend records): silence 5 s, read
~10 s (on-screen text with p/b/s words), loud 3 s, typing 4 s, desk_tap 3 s,
cable_wiggle 4 s, moving 5 s.

`apply_mic_chain`'s `obsInput` may be empty or unknown: the backend then uses
the OBS input capturing the mic from the last test (or OBS's main mic). It writes the chain atomically (temp file + rename) to `%APPDATA%\SetupCoach\chain.json` (read
live by our VST3: mono, HPF, EQ, de-esser), makes sure our VST3 filter exists on
the OBS mic input, and creates/updates OBS built-in filters for gain, expander,
compressor and limiter in the right order via obs-websocket. Hardware gain
changes are coached, never applied.

## Camera test

| Command | Args | Returns |
|---|---|---|
| `camera_state` | `{ cameraId: string }` | `CameraState` (core video) |
| `start_camera_test` | `{ cameraId: string }` | `()` – emits `camera-preview` `{ image: string /* JPEG data URL, 480 px wide */, paper_guide: [x0, y0, x1, y1] /* fractions */ }` ~4/s until `finish_camera_test` |
| `capture_camera_step` | `{ step: "neutral" \| "turn_head" \| "hold_paper" \| "lean_in_out", seconds: number }` | `{ frames: number, faces_found: number }` |
| `finish_camera_test` | `{ constraints: Constraint[] }` | `CameraReport` – frames are kept, so call again with new constraints to recalibrate (there is no `rescan_camera`) |
| `apply_camera_settings` | `{ cameraId: string, settings: CameraSettings }` | `ApplyResult` |

```ts
CameraReport = { metrics: VideoMetrics, findings: Finding[], settings: CameraSettings,
                 score: Score, class: CameraClass }
```
Frames come from OBS (`GetSourceScreenshot` on the camera source) when OBS
holds the camera, else from the camera directly. Screenshot sampling (~4/s)
can't measure the camera's real frame rate, so frames are time-stamped at the
requested fps; delivered-fps problems are caught by the OBS source/stats checks. Face rectangles come from the
Windows built-in face detector (metering only). The paper rectangle is the
on-screen guide box (`paper_guide`, fractions of the frame: x 0.66–0.94,
y 0.35–0.75, beside the face) the user fills with a white sheet during
`hold_paper`; the inner 80 % is metered.

## Stream health

| Command | Args | Returns |
|---|---|---|
| `start_health_monitor` | – | `()` – polls OBS every 2 s; emits `health-alert` (core `Alert`, on raise and clear) and `health-sample` `{ sample: StatsSample, window: WindowStats, active: AlertKind[] }` |
| `stop_health_monitor` | – | `StreamReport` (core) |

## Persistence

| Command | Args | Returns |
|---|---|---|
| `save_setup` | `{ name: string }` | `SavedProfile` |
| `reapply_saved_setup` | – | `ApplyResult` – also run automatically at startup and on camera re-plug |

```ts
SavedProfile = { name: string, saved_at: string, mic_chain: ProcessingChain | null,
                 camera_settings: CameraSettings | null, camera_id: string | null,
                 obs_input: string | null }
```
Stored in `%APPDATA%\SetupCoach\setup.json`.

## Live coaching (added 2026-11-02)

Coaching is a live loop: the user changes something physical (or in another
app) while we measure continuously, show animated guidance, and confirm
success ourselves. Never "did you do it?" — we detect it. Where software can
make the change (Windows endpoint gain, GoXLR Utility, UVC camera controls) we
do it **while the user talks/sits** and show it converge.

### Core engine (`coach_core::coach`)

```rust
pub enum CoachTaskKind {           // serde snake_case
    // mic
    SetGain,          // speech peaks into the green zone (−18…−12 dBFS); auto when gain is software-controllable
    FindDistance,     // closer/farther vs the profile's ideal, from level + proximity bass + roominess vs baseline
    AngleOffAxis,     // say "pop pop pop" until plosive bursts stay under threshold
    QuietRoom,        // live noise floor while silent (fan off, mic null at the noise)
    KeyboardNull,     // type while silent; rotate mic until typing drops
    TalkIntoLogo,     // side-address: presence/air rise vs baseline when facing the logo side
    // camera
    LightFace,        // face ≥ background + 0.5 stop, no face clipping
    LightDirection,   // key:fill ≤ 3:1 (cheek ratio), arrow toward the darker side
    WhiteBalance,     // paper cast within ±4 %
    CameraHeight,     // eye line / pitch: lens at eye level
    Framing,          // face size/position/headroom
    Flicker,          // no banding after anti-flicker/shutter change
    // anything verified by a settings snapshot (Windows/OBS/vendor app)
    CheckSetting,
}
pub struct CoachTarget { pub min: f64, pub max: f64, pub unit: String }
pub enum Phase { Waiting, Working, Almost, Hold, Done, Stuck }   // snake_case
pub enum HintKind { Closer, Farther, TurnUp, TurnDown, Stop, AngleAway, FaceLogo,
                    MoveLightLeft, MoveLightRight, MoveLightUp, AddLight, DimBackground,
                    RaiseCamera, LowerCamera, MoveLeft, MoveRight, MoveBack, MoveForward,
                    KeepTalking, BeQuiet, TypeNow, SayPops, Wait, None }
pub struct Hint { pub kind: HintKind, pub amount: f64 /* 0..1 how far to go */, pub text: String }
pub struct CoachUpdate {
    pub task: CoachTaskKind, pub phase: Phase,
    pub progress: f64,                 // 0..1 toward done (drives rings/bars)
    pub value: Option<f64>,            // main reading, in target.unit
    pub target: Option<CoachTarget>,
    pub headline: String,              // big friendly line, e.g. "A little closer…"
    pub detail: String,                // why / how, one sentence
    pub hint: Hint,
    pub hold_s: f64, pub hold_needed_s: f64,  // "hold steady" ring
    pub readings: BTreeMap<String, f64>,      // Advanced drawer
    pub auto_adjusting: bool,          // the app itself is moving the setting
    pub stuck_alternative: Option<String>,    // shown when phase == Stuck (after ~45 s w/o progress)
}
// Audio: feed 10–50 ms chunks; returns an update at most every ~100 ms.
pub struct LiveMicCoach;   // new(task, sample_rate, &MicContext, &AudioMetrics baseline?) ; push(&[f32]) -> Option<CoachUpdate>
                           // set_gain_auto(current_gain_db, min, max) -> Option<f64> (next gain to apply) when auto
// Video: feed frames (with optional face/paper rects); returns an update per frame.
pub struct LiveCameraCoach; // new(task, &CameraState, CameraClass, MainsHz) ; push(&TaggedFrame) -> CoachUpdate
```

### App commands / events

| Command | Args | Returns |
|---|---|---|
| `coach_plan` | `{ area: "mic" \| "camera" }` | `CoachStep[]` – ordered steps from the latest report's findings |
| `start_coach` | `{ step: CoachStep }` | `()` – emits `coach-update` (~8/s) until done/stopped |
| `stop_coach` | – | `()` |
| `open_for_user` | `{ target: OpenTarget }` | `()` – opens the vendor app / Windows Settings page next to us; we stay visible (always-on-top mini coach) and come back to front when the step's check passes |

```ts
CoachStep = { id: string, finding_id: string | null, task: CoachTaskKind,
              title: string, explain: string,            // plain words + why it matters
              animation: string,                          // UI animation key, e.g. "knob", "mic_distance", "mic_angle", "logo_side", "noise_null", "light_move", "camera_height", "paper"
              auto: boolean,                              // app adjusts it itself
              open_target: OpenTarget | null,             // when the user must click elsewhere
              check: string | null }                      // CheckSetting predicate id
OpenTarget = { kind: "windows_settings" | "vendor_app" | "obs", uri_or_app: string, label: string,
               steps: string[] }                          // click-by-click text shown in the mini coach
```
Event `coach-update`: core `CoachUpdate` (also carries `t_s`: seconds since
the session started, from samples/frame timestamps). Event `coach-camera-overlay`
(~4/s):
```ts
{ frame: dataUrl,                 // small JPEG of the analysed frame
  width: number, height: number,  // size of that frame; face/paper are pixel rects in it
  face: Rect | null, paper: Rect | null, clipped_mask: dataUrl | null,
  arrows: { x, y, dx, dy }[],     // normalised 0–1 frame coords, origin top-left (as the preview is shown)
  face_ok: boolean }              // face exposure OK right now → draw the face box green
```

Notes (implementation, 2026-11-02):
- Camera `HintKind`s (`move_light_left/right`, `move_left/right`,
  `raise/lower_camera`) are in the preview's screen space; `farther` on
  `light_face` means "move the light farther away / dim it".
- `CoachStep.id` is stable per step kind: `check-goxlr-utility`,
  `check-mic-privacy`, `check-voice_clarity_off`, `check-enhancements_off`,
  `check-goxlr-phantom`, `check-auto-level`, `talk-into-logo`,
  `find-distance`, `angle-off-axis`, `set-gain`, `quiet-room`,
  `keyboard-null`, `check-camera-privacy`, and for camera tasks the task name
  (`light_face`, `light_direction`, `white_balance`, `camera_height`,
  `framing`, `flicker`). `finding_id` is always the core finding id (or null
  for steps not caused by a finding, e.g. GoXLR phantom power).
- `CheckSetting` predicates (`check`): `goxlr_utility_running`,
  `goxlr_condenser`, `mic_privacy_allowed`, `voice_clarity_off`,
  `enhancements_off`, `camera_privacy_allowed`, `obs_running`. A step with
  `check: null` waits for the user's "Done". Auto `CheckSetting` steps
  (`check-goxlr-phantom`) make the change themselves, then verify.
- `CoachArea` is `"mic" | "camera"`. `open_for_user` returns an error string
  ("Open X yourself, then come back here.") if it can't launch the target.
- `mic_context` also asks GoXLR Utility (when running and the mic is on a
  GoXLR): `interface_max_gain_db = 72` and `gain_at_max` from the real gain.
- Auto `SetGain` uses the Windows endpoint level only when the mic's profile
  says that slider is the real gain (`windows_slider_is_hw_gain`); otherwise
  the user turns the knob with live guidance.

GoXLR Utility (preferred for all GoXLR owners — recommend installing it and
removing the unmaintained official app): when its daemon answers on
`http://localhost:14564`, `SetGain` runs in auto mode via its API and mic type
/ phantom power can be set; otherwise coach the GoXLR's own gain control.
