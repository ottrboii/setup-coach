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
  saved_profile: SavedProfile | null   // last applied setup, re-applied on start (null while setup.json only holds mic answers)
}
ObsStatus = { running: boolean, connected: boolean, version: string | null,
              websocket_enabled: boolean | null, error: string | null }
AudioDeviceInfo = { id: string, name: string, is_default: boolean,
                    is_default_comms: boolean, channels: number, sample_rate: number,
                    profile: ProfileMatch | null,
                    obs_input: string | null,    // OBS audio input capturing this device
                    mic_choice: MicBehind | null,     // what the user said is plugged into it (added 2026-11-05)
                    needs_mic_choice: boolean }       // ask "What mic is plugged into this?" now (see below)
CameraDeviceInfo = { id: string, name: string, obs_source: string | null,
                     profile: ProfileMatch | null }
// Owned view of core's MatchResult (which borrows the profile database).
ProfileMatch = { kind: "mic" | "interface" | "camera", profile_id: string,
                 display_name: string, confidence: "exact" | "name_match" | "generic",
                 verified: boolean, questions: Question[],      // core profiles::Question
                 interface: ProfileMatch | null,                 // mic behind an interface
                 profile: object,                                // full profile JSON
                 chosen: boolean }       // true = the user named this mic (`set_mic_behind`), not guessed from the Windows name
```

`get_environment` auto-connects to OBS when possible: it reads the websocket
port/password from `%APPDATA%\obs-studio\plugin_config\obs-websocket\config.json`
(user consent is shown in the UI before first use).

## Quick checks (the free scan)

| Command | Args | Returns |
|---|---|---|
| `run_quick_checks` | – | `QuickChecks` |
| `apply_actions` | `{ actionIds: string[] }` | `ApplyResult` |
| `log_ui` | `{ level: string, source: string, message: string }` | `()` – appends a line to `%APPDATA%\SetupCoach\logs\ui.log` (in the self-test zip). The UI calls it for every rejected command and caught screen error. (added 2026-11-05) |

`run_quick_checks` runs in a guarded task (45 s limit): a panic or hang rejects with a plain string instead of never answering. It only offers actions that can work: fixes that need administrator rights the app lacks are left out and their finding comes back `fix_kind: "guided"` (no "We can do it for you" sentence). OBS Advanced-mode keyframe interval / rate control are written to the profile's `streamEncoder.json` (with a vault backup).

```ts
QuickChecks = { findings: Finding[], actions: PlannedActionView[], score: Score[] }
PlannedActionView = { id: string, label: string, resolves: string[], needs_admin: boolean,
                      needs_obs_closed: boolean, action: Action }   // Action = coach_core::system Action
ApplyResult = { applied: string[], failed: { id: string, error: string, guided?: CoachStep }[], restart_obs_needed: boolean,
                skipped: { id: string, error: string }[] }   // plain notes; only restore_backup fills it (added 2026-11-03)
```

**Windows audio enhancements / Voice Clarity (added 2026-11-05).** `DisableEnhancements` / `DisableVoiceClarity` are always offered (`needs_admin: true` means "Windows may show its permission prompt"). Apply order: write directly (administrators), else a one-off elevated helper of our own exe (`--fx-helper`, Windows' UAC prompt, changes exactly one value, 60 s), and Voice Clarity through the Windows audio-effects API. Afterwards the backend **re-reads the real state** and only puts the id in `applied` if the effect is really off. When it is not (prompt declined, driver won't allow it, call claimed success but nothing changed) the id lands in `failed` with a plain `error` and a `guided` `CoachStep` (`task: check_setting`, `check: "enhancements_off" | "voice_clarity_off"`, `open_target.uri_or_app: ms-settings:sound-properties?endpointId=<this mic>`). The UI should show a button that calls `open_target` + `start_coach(failure.guided)`: the coach polls the effect state and finishes by itself when it flips (no "did you do it?"). `coach_plan` uses the same step, so the guided path is identical from the plan.

**"Listen to this device" / exclusive control (added 2026-11-05).** `DisableListen` / `DisableExclusiveMode` follow the same rules (always offered, direct write, else the same elevated helper `--fx-helper regflag <listen|exclusive> <endpoint> <0|1>`, read-back before `applied`, `failed[].guided` otherwise). The guided step has `check: "listen_off" | "exclusive_off"` and opens the classic `mmsys.cpl` Sound control panel (these two switches are not in the new Settings page). New `CheckSetting` predicates `listen_off`, `exclusive_off`; new step ids `check-listen_off`, `check-exclusive_off`.

**UI use of `failed[].guided` (2026-11-05).** Scan results, the dashboard re-apply and the mic-apply screen show the plain `error` and one big "Show me how" button. It opens the coaching screen with that single step (`open_for_user(open_target)` + `start_coach(guided)` fire straight away), detects completion itself, then re-runs `run_quick_checks` and returns. "I can't do this" re-checks (and re-scans the mic advice if there is one) and returns.

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
| `play_preview` | `{ which: "before" \| "after" \| "original" \| "final", phone: boolean }` | `()` – `original`/`final` play the saved A/B takes, loudness-matched (see "Settings vault & A/B") |
| `stop_preview` | – | `()` |

```ts
MicAnswers = { mic_model_id?: string, booster?: boolean, gain_at_max?: boolean,
               onboard_auto_level?: boolean }
SegmentKind = "silence" | "read" | "loud" | "typing" | "desk_tap" | "cable_wiggle" | "moving"
```
Event `mic-level`: `{ rms_db: number, peak_db: number, clipped: boolean }` (~20/s).

Guided steps & durations (UI drives them, backend records): silence 5 s, read
~10 s (on-screen text with p/b/s words), loud 3 s, typing 4 s, desk_tap 5 s
(tap twice next to the mic stand, then sit still; **optional**: the UI offers
"Skip this step" and then never sends its `mark_step`, so the report falls back
to typing/quiet-step evidence for desk vibration), cable_wiggle 4 s, moving 5 s.
`finish_mic_test`'s constraints may now include `cant_fix_desk_vibration`
(finding `mic.desk_thumps`; steeper low-cut as backup). `AudioMetrics` gained
`vibration` (`#[serde(default)]`, old saved metrics still load); `Constraint`
gained one variant. No command changes.

`apply_mic_chain`'s `obsInput` may be empty or unknown: the backend then uses
the OBS input capturing the mic from the last test (or OBS's main mic). It writes the chain atomically (temp file + rename) to `%APPDATA%\SetupCoach\chain.json` (read
live by our VST3: mono, HPF, EQ, de-esser), makes sure our VST3 filter exists on
the OBS mic input, and creates/updates OBS built-in filters for gain, expander,
compressor and limiter in the right order via obs-websocket. Hardware gain
changes are coached, never applied.

## What mic is plugged into this? (added 2026-11-05, approved)

A GoXLR or audio interface hides the real mic: Windows only shows the device
("Chat Mic"), so the user names the physical mic. No auto-guessing.

| Command | Args | Returns |
|---|---|---|
| `list_mic_profiles` | – | `MicProfileOption[]` – every specific mic in `profiles/mics.json` (generic fallbacks left out), sorted by brand, model; the UI filters it for the search box |
| `get_mic_behind` | `{ deviceId: string }` | `MicBehind \| null` |
| `set_mic_behind` | `{ deviceId: string, choice: MicBehindInput }` | `MicBehindResult` – rejects with a plain string for an unplugged device or unknown profile |

```ts
MicProfileOption = { id: string, brand: string, model: string, display_name: string /* "Electro-Voice RE20" */,
                     family: string, connection: "usb" | "xlr" | "usb_xlr" | "headset" | "wireless" | "builtin",
                     capsule: "dynamic" | "condenser" | "electret", verified: boolean, aliases: string[] }
MicBehind = { status: "chosen" | "other" | "skipped",   // other = not in the list, skipped = "Not sure"
              profile_id: string | null, display_name: string | null,   // set when status is "chosen"
              device_name: string, chosen_at: string }
MicBehindInput = { kind: "profile", profile_id: string } | { kind: "other" } | { kind: "skipped" } | { kind: "clear" }   // clear = ask again
MicBehindResult = { mic_choice: MicBehind | null, profile: ProfileMatch | null, needs_mic_choice: boolean }
                  // the device's new view: patch `AudioDeviceInfo` with it, no need to reload the environment
```
- Stored per device id in `setup.json` (`mic_behind`) and written through the settings
  vault like every other write: the previous answer is backed up first (label "Before choosing which
  mic is plugged into Chat Mic", item kind `mic_choice`); if the backup can't be written nothing changes;
  "go back to how it was" removes the answer again. An unreadable `setup.json` is kept as
  `setup.json.unreadable`, not overwritten blindly.
- `needs_mic_choice` is true for devices that hide or can't name the mic (interface/mixer inputs, unknown
  USB mics, wireless receivers) while there is no answer yet; false for headsets, Bluetooth, built-in and
  recognised mics, and after any answer (also "Not sure"). UI: ask once, before the scan / mic test, and
  show "Your mic: Chat Mic (Electro-Voice RE20) · Change" on the scan, mic test and mic report screens.
- Effect: with `status: "chosen"` the device's `profile` in `get_environment` is the chosen mic (`chosen: true`,
  `confidence: "exact"`, its own `questions`, `interface` still the GoXLR/interface from the Windows name),
  `mic_context` builds the `MicContext` from it (name, capsule, address, distance, gain needed, connection),
  and auto gain asks it whether the Windows slider is hardware gain. Order in `mic_context`:
  `answers.mic_model_id` (a profile id or the `Brand Model` label a question offered) > the saved answer > the
  Windows-name match. After a change the UI calls `mic_context` again; on the report screen `rescan_mic`
  re-analyses the last recording for the new mic, no retest.

## Camera test

| Command | Args | Returns |
|---|---|---|
| `camera_state` | `{ cameraId: string }` | `CameraState` (core video) |
| `start_camera_test` | `{ cameraId: string }` | `()` (rejects with `CameraError`, below) – emits `camera-preview` `{ image: string /* JPEG data URL, 480 px wide */, paper_guide: [x0, y0, x1, y1] /* fractions */ }` ~4/s until `finish_camera_test` |
| `capture_camera_step` | `{ step: "neutral" \| "turn_head" \| "hold_paper" \| "lean_in_out", seconds: number }` | `{ frames: number, faces_found: number }` |
| `finish_camera_test` | `{ constraints: Constraint[] }` | `CameraReport` – frames are kept, so call again with new constraints to recalibrate (there is no `rescan_camera`) |
| `apply_camera_settings` | `{ cameraId: string, settings: CameraSettings }` | `ApplyResult` |
| `camera_picture_status` | `{ cameraId: string }` | `PictureStatus` – one live probe: can we see the camera, and who has it? (added 2026-11-05) |
| `free_camera` | `{ cameraId: string }` | `FreeCameraResult` – "Free it for me": releases the camera from OBS' video capture source (its own Deactivate button via obs-websocket `PressInputPropertiesButton`), after a vault backup (`obs_camera_active`) |
| `restore_freed_camera` | `()` | `FreeCameraResult` – gives it back (presses the button again only if the source is still inactive). Safe to call any time; `ok: true, source: null` = nothing to give back |
| `get_mains` / `set_mains` | `()` / `{ hz: 50 \| 60 \| null }` | `MainsInfo` – power-grid frequency for anti-flicker; `null` clears the user's choice |

```ts
CameraReport = { metrics: VideoMetrics, findings: Finding[], settings: CameraSettings,
                 score: Score, class: CameraClass }
```

**No usable picture (added 2026-11-05).** A webcam can be opened by one app at a time; when OBS,
Teams, Zoom... hold it we get black or flat frames, an open error or a hang. Such frames are never
analysed: `start_camera_test`, `capture_camera_step` and `finish_camera_test` reject with a
`CameraError` instead of ever returning findings or a plan from them.
```ts
CameraError = { code: "no_picture" | "other", reason?: PictureReason, message: string /* plain English fallback */ }
PictureReason = "black" | "blank" | "cant_open" | "timeout"
PictureStatus = { live: boolean, reason: PictureReason | null,
                  users: { name: string /* "OBS Studio", "Zoom"... */, confirmed: boolean /* Windows says it is using the camera right now */ }[],
                  obs: { running: boolean, connected: boolean,
                         source: string | null,      // the OBS video capture source using this camera
                         holding: boolean,           // OBS has that source's camera open
                         can_free: boolean },        // "Free it for me" can work
                  freed_source: string | null }      // we released this OBS source and have not put it back yet
FreeCameraResult = { ok: boolean, source: string | null, message: string /* plain English, show when !ok */ }
MainsInfo = { hz: 50 | 60, source: "override" | "time_zone" | "region" | "default", detected_hz: 50 | 60 }
```
Flow: on `no_picture` the UI shows "Your camera is being used by X, so we can't see it" and polls
`camera_picture_status` (~2 s) until `live`, then restarts the test by itself; options are
`free_camera` ("Free it for me"), "I'll close it myself" (just keep polling) and "I can't do this"
(skip the camera). The camera is a `restore_freed_camera` call away from going back to OBS (the UI
calls it when leaving the camera/coach screens); the vault entry `obs_camera_active` undoes it too.
Event `camera-picture` `{ live: boolean, reason?: PictureReason }` is sent when the picture is lost
or returns, during the preview and live camera coaching (coaching skips unusable frames).
The mains frequency comes from the Windows time zone first (a PC set to a US region sitting in Europe
is 50 Hz), then the region, then 50 Hz; the user's choice (`set_mains`, under Advanced) wins.
`finish_camera_test` and the live flicker coach use it.
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

## Testers' notes: "Tell us what's wrong" (added 2026-11-05, approved)

Always-available panel in the UI (sidebar button, Ctrl+Shift+F). It lives outside the screen area, so it never interrupts a running test, preview, apply or recording. The draft text is kept by the UI (localStorage) until saved. Notes stay on the PC: no network, never auto-uploaded.

| Command | Args | Returns |
|---|---|---|
| `save_feedback` | `{ input: FeedbackInput }` | `{ count: number }` – appends one line to `%APPDATA%\SetupCoach\feedback.jsonl`. Rejects with a plain message when `text` is blank. |
| `get_feedback` | – | `{ count: number, text: string }` – all notes as readable text, for "Copy my notes". |
| `save_feedback_to_desktop` | – | `string` – writes `SetupCoach-notes.txt` to the Desktop and returns its path; rejects when there are no notes. |

```ts
FeedbackInput = { text: string, route: string,                 // route = current UI route, e.g. "mic"
                  tag: "confusing"|"broken"|"looks_wrong"|"idea"|null,
                  mic_name: string|null, camera_name: string|null }
// one jsonl line: { at, route, tag, text, app_version, mic_name, camera_name }
```
`feedback.jsonl` sits in the data folder, so `--self-test` packs it into the zip (`SetupCoach/feedback.jsonl`) with no extra step.
## Persistence

| Command | Args | Returns |
|---|---|---|
| `save_setup` | `{ name: string }` | `SavedProfile` |
| `reapply_saved_setup` | – | `ApplyResult` – also run automatically at startup and on camera re-plug |

```ts
SavedProfile = { name: string, saved_at: string, mic_chain: ProcessingChain | null,
                 camera_settings: CameraSettings | null, camera_id: string | null,
                 obs_input: string | null,
                 mic_behind: { [deviceId: string]: MicBehind } }   // added 2026-11-05, see "What mic is plugged into this?"
```
Stored in `%APPDATA%\SetupCoach\setup.json`. `save_setup` keeps `mic_behind`. A `setup.json` that only holds mic answers has `name: ""`, `saved_at: ""` and is not reported as `saved_profile`.

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
  `enhancements_off`, `listen_off`, `exclusive_off`, `camera_privacy_allowed`, `obs_running`. A step with
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

## Settings vault & A/B (added 2026-11-03)

### Settings vault — `%APPDATA%\SetupCoach\backups\`

Before **every** write the app makes, exactly what is about to change is saved
first; if that backup can't be written, nothing is changed (the command
returns a failure saying so). Covered writes:

| Write | What's saved first (vault item `kind`) |
|---|---|
| `apply_actions` (OBS via websocket) | `obs_filters` (all filters of that source: name, kind, enabled, full settings, order), `obs_input_settings`, `obs_input_mute`, `obs_monitoring`, `obs_profile_param`, `obs_video_settings` |
| `apply_actions` (OBS files, OBS closed) | `obs_file` (copy of the scene collection / `basic.ini`), `obs_websocket_enabled` (only the on/off flag — the file also holds the WebSocket password, which is never stored) |
| `apply_actions` (Windows) | `windows` (ducking, endpoint level, enhancements, Voice Clarity, Listen, exclusive mode, USB power saving — raw values) |
| `apply_mic_chain` | previous `chain_json` (or "none"), `obs_filters` of the mic input, the scene file when OBS's mono switch is used |
| `apply_camera_settings`, camera auto-coach | `camera` (all camera control values), `obs_input_settings` of the OBS webcam source |
| auto gain (`start_coach` SetGain), GoXLR phantom | `endpoint_gain` (Windows level, dB) or `goxlr` (mic type + gain); once per coach step |
| `reapply_saved_setup`, `restore_backup` | the same items, so a restore can itself be undone |

Layout: `backups/original/` and `backups/<id>/` (`id` = UTC timestamp, e.g.
`20261103T101530Z-000`), each `snapshot.json` + `files/` (copied OBS files).
Written atomically (temp + rename). **`original`** gets each item the first
time Setup Coach ever touches it and never overwrites it — it is "how it was
before Setup Coach" across sessions and app updates. Per-change snapshots:
newest 30 kept (an identical repeat of the newest one isn't stored again).
The folder is outside the installer's "delete app data" folders
(`%APPDATA%\app.setupcoach.desktop`, `%LOCALAPPDATA%\app.setupcoach.desktop`),
so it survives updates and uninstall.

| Command | Args | Returns |
|---|---|---|
| `list_backups` | – | `BackupInfo[]` – `original` first (if any), then newest first |
| `restore_backup` | `{ id: string }` – `"original"` = put everything back how it was before Setup Coach | `ApplyResult` |
| `preview_restore` | `{ id: string }` – dry run of `restore_backup`, writes nothing (added 2026-11-05) | `RestorePreview` – error (plain) if the backup is gone |
| `obs_running` | – (added 2026-11-05) | `boolean` – the OBS program is running right now (not "connected over websocket") |

```ts
BackupInfo = { id: string, created_at: string /* ISO UTC */,
               label: string,      // plain language: "How it was before Setup Coach",
                                   // "Before setting up your Yeti Stereo Microphone", "Before quick fixes",
                                   // "Before: Turn off Windows audio enhancements", "Before changing your HD Pro Webcam C920 settings",
                                   // "Before auto-adjusting your mic level", "Before going back to …"
               items: BackupItem[] }   // changed 2026-11-05: was string[]
BackupItem = { kind: BackupItemKind,   // stable; word it yourself in plain language
               label: string }         // English fallback, e.g. "OBS: filters on \"Mic/Aux\"", "Windows: Voice Clarity"
BackupItemKind = "obs_filters" | "obs_source_settings" | "obs_mute" | "obs_monitoring" | "obs_setting"
               | "obs_video" | "obs_scene_file" | "obs_profile_file" | "obs_websocket"
               | "windows_setting" | "mic_level" | "camera" | "voice_chain" | "goxlr"
               | "mic_choice"          // added 2026-11-05: the "what mic is plugged into this" answer
               | "obs_camera_active"   // added 2026-11-05: OBS was holding the camera and we released it

RestorePreview = { id: string, label: string,
                   items: RestorePreviewItem[],   // newest first = the order restore goes through them
                   obs_running: boolean }         // same as `obs_running`, at the time of the call
RestorePreviewItem = { key: string,               // same id `restore_backup` reports the item under
                       kind: BackupItemKind, label: string,
                       action: "change"          // will be put back
                             | "skip"            // will be left alone, `note` says why (device unplugged, OBS closed
                                                 //   for live OBS items, GoXLR Utility not running, source gone)
                             | "needs_obs_closed", // OBS file/flag: only possible while OBS is closed
                       note: string | null }
```
`preview_restore` mirrors the checks `restore_backup` makes but does not compare
values: "change" means "will be written", not "differs from now". Suggested
flow: on "Go back to this", call `obs_running` + `preview_restore`; if any item
is `needs_obs_closed`, ask the user to close OBS first (poll `obs_running` until
false), then restore. Word the confirmation from `kind` + `action`.
`restore_backup` result ids are vault keys (`obs_filters:Mic/Aux`,
`windows:voice_clarity:<endpoint>`, `camera:<id>`, `chain_json`,
`obs_file:<path>`, `goxlr:<serial>` …). Restore goes newest item first (so
the earliest saved value wins), and:
- an unplugged mic/camera, a missing OBS source, GoXLR Utility not running, or
  OBS closed (for live OBS items) → `skipped` with a plain note, never a failure;
- OBS files need OBS closed: with OBS open they come back in `failed` with
  "Close OBS, then press Restore again…" (same handling as `needs_obs_closed`).
  Suggested UI flow: restore with OBS open (live items), then if any `failed`
  id starts with `obs_file:`/`obs_websocket`, ask the user to close OBS and
  press Restore again (restoring twice is harmless). `restart_obs_needed` is
  true when OBS files were put back.
- Restoring `chain.json` also clears the session's applied chain.

### Real A/B recordings — `%APPDATA%\SetupCoach\recordings\<device-slug>\`

`device-slug` = the device name in lower-case words joined by `-`
(`Microphone (Yeti Stereo Microphone)` → `microphone-yeti-stereo-microphone`).
- `original.wav` + `original.json`: the **first `finish_mic_test` for a device
  in an app session, before anything was applied for it** (chain, Windows
  actions for that endpoint, auto gain). Whole take, mono (channels averaged),
  32-bit float WAV at the capture rate (48 kHz on most PCs). An existing
  original is kept as `original-<date>.wav/.json`, never deleted.
- `final-raw.wav` (as captured), `final.wav` (through the applied chain with
  core's offline renderer — static processing only, mirroring the OBS filters
  + our VST3; no AI), `final.json`.
- `*.json` = `{ device_id, device_name, recorded_at, sample_rate, segments, speech_lufs, noise_floor_dbfs, peak_dbfs, score, backup_id, chain }`.

| Command | Args | Returns |
|---|---|---|
| `record_final` | `{ deviceId: string, context: MicContext, constraints: Constraint[] }` | `AbInfo` |
| `get_ab` | `{ deviceId: string }` | `AbInfo` – rejects with `AbError` (below), e.g. `no_first_test` |
| `export_ab` | `{ deviceId: string }` | `string` (rejects with `AbError`) – folder path: `Desktop\Setup Coach A-B - <device name>\A original.wav`, `B final.wav` (the matched reading clips) |

```ts
AbInfo = { original: AbTake, final: AbTake | null,
           matched_gain_db: number,
           findings: Finding[],        // added 2026-11-05: open (minor or worse) findings of the FINAL take, as found when it was
                                       //   recorded; [] without a final take. Source for the "I can't do this" path (rescan_mic with constraints).
                                       //   Saved in final.json; takes recorded before this change give [].
           note: string | null }       // plain-language caveat, e.g. when the takes differ by >30 dB and could only be matched part way; show it when set   // final LUFS − original LUFS (0 when no final); >0 = final was louder
AbTake = { path: string, lufs: number /* reading part, LUFS */, score: number /* mic score 0–100 */ }

// Rejection value of get_ab, record_final and export_ab (an object, not a string; added 2026-11-05):
AbError = { code: AbErrorCode, message: string /* plain English fallback */ }
AbErrorCode = "no_first_test"      // get_ab/record_final: no first test saved for this mic yet -> run the mic test
            | "no_final_take"      // export_ab: nothing recorded after the fixes yet
            | "mic_missing"        // get_ab: that mic isn't plugged in and we have no recordings for it
            | "no_recording"       // record_final: start_mic_test wasn't running
            | "wrong_mic"          // record_final: the running recording is from another mic
            | "too_short"          // record_final: under a second recorded
            | "no_read_step"       // record_final: the read step wasn't marked
            | "no_applied_chain"   // record_final: apply the new settings first
            | "unreadable"         // a saved recording is damaged -> run the mic test again
            | "other"
```
`get_ab` with an original but no final take is **not** an error: `final` is null.
Other commands still reject with a plain string. UI: show your own wording per
`code`, fall back to `message`.
The "final take" is **not a live recording through OBS**: it is the new read
run through the applied chain with core's offline renderer. Word it as "your new
sound" / "how it should sound with your new settings", not as a live take.

Final-take flow (UI): after fixes are applied and verified, call
`start_mic_test { deviceId }`, run the **same guided "read" step with the same
on-screen passage** as the first test (~10 s; `mark_step` read start/end —
other steps are optional), then `record_final` instead of `finish_mic_test`.
Errors: no recording running, recording from a different mic, too short / no
read step, no applied chain yet ("Apply your new settings first…"), no
original for this mic.

The A/B compares the **reading part** of both takes. Playback
(`play_preview { which: "original" | "final", phone }`, for the mic of the last
test) and `export_ab` are **loudness-matched**: the louder take is turned
down by `|matched_gain_db|` (capped at 30 dB), so the UI can say e.g. "Final
was 4 dB louder — matched for a fair comparison". `phone` applies the
phone-speaker simulation after matching.


## Updates (added 2026-11-03, revised 2026-11-04)

The app updates itself from the newest *published* GitHub release
(`.../releases/latest/download/latest.json`, signed installer). All of it runs
in Rust; **the UI has no update commands and needs nothing**. About 10 s after a
normal start (never in `--self-test`) it checks in the background and
**downloads the update silently**; nothing is installed until the user
confirms. Then a native dialog asks "A new version of Setup Coach is ready
(x.y.z). Install now?": Yes installs (Windows shows a permission prompt) and
restarts the app, No installs nothing and asks again next start. If OBS is
running it first asks the user to close OBS so the voice plugin can be replaced
(at most 3 times, then it gives up until the next start). Failures are silent
(logged only). `%APPDATA%\SetupCoach\` is never touched. A build without an
updater public key (PR/main CI, local dev) never checks.
(`check_for_update` / `install_update` were removed: no UI called them and
`install_update` skipped the confirmation.)

### Profile questions (`Question`)
Every closed question ends with `I'm not sure` (core adds it) and the UI adds it if missing. A switch is one question per control: `{ id: 'switch_positions', key: 'bass_tilt', prompt, options[], help? }`. `help` = `{ what, where?, positions: [{label, meaning}], image?, manual_url? }` (plain words, no numbers; `image` is a path under app/ui/, `manual_url` must be https). The UI stores the answer under `switch_positions:<key>` and a second tap clears it. Profile JSON: `controls: [{ key, prompt, options, help, source }]` replaces the old `switches` string list. Switch answers are UI-only for now: `MicAnswers` has no switch field (follow-up).

## Connection health (2026-11-05)
No contract change. \QuickChecks.findings\ now also carries ids \connection.usb3_on_usb2_port\, \connection.behind_hub\, \connection.device_dropouts\, \connection.cameras_share_controller\, \connection.camera_held_by_app\, \connection.mic_held_by_app\ (areas camera/mic, \ix_kind\ manual or guided, numbers only in \evidence\: \speed_mbps\, \drops_7d\). They clear on the next scan once the cause is gone. Backend: \SystemInspector::connection_state()\; Windows reads USB speed/hubs/drops with a PowerShell probe (cached 15 s) and the Windows camera/mic privacy log.

### Field-complaint extras (camera body, HDMI, heat, lip sync)
No contract change. New finding ids, all plain-text with numbers only in `evidence`: `connection.camera_body_not_webcam` (QuickChecks, guided; names the maker tool that is running), `connection.camera_switches_off_after_a_while` (QuickChecks, manual; evidence `typical_run_minutes`, `runs`), `camera.hdmi_overlay` (camera test report, capture-card cameras only), `sync.av_offset` (area mic; evidence `offset_ms`, `score`, `claps`; produced by `coach_core::avsync`, now returned by the clap test, see "Lip-sync clap test"). `CameraError` for a capture-card camera with black/flat frames now carries the HDMI checklist in `message` (same `code`/`reason`). `near_runtime_limit` now drives `QuickChecks.runtime_warnings` (see "Lip-sync clap test and camera runtime warning").


## Capture-card checks (dual-PC, single stream PC, 2026-11-06)
No command change; new finding ids, plain text, numbers only in `evidence`, all `can_decline: true` ("I can't do this" → recalibration) and cleared by the next scan once the cause is gone (re-measure, no "did you do it?"):
- QuickChecks (area camera, guided): `connection.capture_card_on_slow_port` (evidence `speed_mbps`), `connection.capture_card_behind_hub` (`hub_bus_powered`), `connection.capture_card_shares_controller` (`devices_on_controller`). They replace the generic `usb3_on_usb2_port` / `behind_hub` / `cameras_share_controller` for capture cards, never both.
- QuickChecks (area camera, guided, needs OBS): `camera.capture_fps_mismatch` (`source_fps`, `stream_fps`): the card's source is set to a fixed 25/50 rate while the stream is 24/30/60 (or the reverse). "Device Default" sources are never accused.
- QuickChecks (area mic, guided, needs OBS): `mic.capture_card_audio_and_mic` (`mic_monitored`): the card's own HDMI sound (source "Audio Output Mode" is not capture-only, a custom audio device, or the card's endpoint as an input) is live together with a separate unmuted mic.
- Camera test report (capture-card cameras only): `camera.hdmi_interlaced`, from comb artefacts on moving edges (`coach_core::video::hdmi::interlaced_combing`).
Data: `UsbLink.capture_card` (bool, from the profile class or a well-known name), `ObsVideoCapture.own_audio` / `.audio_device` (from the OBS `dshow_input` settings `audio_output_mode`, `use_custom_audio_device`, `audio_device_id`). The Cam Link 4K profile now has the `requires_usb3` quirk.

## Lip-sync clap test and camera runtime warning (added 2026-11-06)

| Command | Args | Returns |
|---|---|---|
| `start_sync_test` | `{ deviceId: string, cameraId: string }` | `()` (rejects with `CameraError` like `start_camera_test`) – records the mic and the camera picture together; emits `sync-progress` and `camera-preview` |
| `finish_sync_test` | – | `SyncReport` – stops, measures (`coach_core::avsync::measure_offset`), says which side to delay. Plain error "Start the clap test first." if none is running |
| `stop_sync_test` | – | `()` – cancel / "I can't do this" / leaving the screen; safe any time, also twice |
| `apply_sync_fix` | – | `SyncApplyResult` – sets the OBS Sync Offset of the mic input (obs-websocket `SetInputAudioSyncOffset`) to what the last `SyncReport.fix` asked for; only for `side: "delay_sound"` with `can_auto` |
| `undo_sync_fix` | – | `SyncApplyResult` – puts the previous Sync Offset back (`{ ok: true, message: "Nothing to undo." }` if none) |

```ts
SyncReport = { verdict: "in_sync" | "sound_late" | "sound_early" | "unclear",
               offset_ms: number | null,       // what viewers get: + = sound after picture. null when unclear
               score: number, claps: number, frames_per_s: number,
               coarse: boolean,                // < 8 pictures/s: rough guide only
               finding: Finding | null,        // id "sync.av_offset" (title/why for display)
               fix: SyncFix | null,
               hint: string | null,            // plain reason when unclear, or a note when coarse
               obs_sync_offset_ms: number }    // OBS Sync Offset already on the mic input (counted in offset_ms)
SyncFix = { side: "delay_camera" | "delay_sound",
            ms: number,                        // absolute value to type: OBS Render Delay (camera) or Sync Offset (sound)
            change_ms: number,                 // how much more than now
            steps: string[],                   // plain-language manual steps
            can_auto: boolean,                 // delay_sound + OBS connected + mic input found
            obs_input: string | null, camera_source: string | null }
SyncApplyResult = { ok: boolean, input: string | null, previous_ms: number | null, applied_ms: number | null, message: string }
```
Event `sync-progress`: `{ t_s, level_db, claps, clap: boolean, frames }`, ~10/s while recording (live clap counter and meter).
Event `camera-preview` is also sent during the clap test, payload `{ image }` (no `paper_guide`).

Flow: intro (device names; "I can't do this" shows by-eye steps) → `start_sync_test` → live counter until 4 claps (or ~14 s) →
`finish_sync_test` → result. `sound_late` (voice after picture): delay the camera with an OBS Render Delay filter, by hand (`fix.steps`).
`sound_early`: delay the sound; with `can_auto` offer "Set it in OBS for me" (`apply_sync_fix`, plus Undo). Either way "Check again"
runs the test again. The mic is read straight from the device, so the backend adds the OBS Sync Offset already set to what it
measures, and the camera picture comes from OBS' screenshot (assumed to include a Render Delay filter) when OBS is connected.
The result is `in_sync` once it is inside the usual limits (sound ahead 45 ms, behind 125 ms). `unclear` carries a plain `hint`
(too few claps, hands not seen), never a guessed number. The Sync Offset change is not in the settings vault yet (only the
in-session `undo_sync_fix`).

### Drift check (added 2026-11-06)

Is the offset fixed (a Sync Offset / Render Delay fixes it) or does it creep (two clocks; only one clock fixes it)? The clap test is
repeated on **one continuous mic recording**: the mic stays open for the whole check and its own sample count is the audio timeline
(as in OBS), pictures are stamped with the PC clock. Between clap rounds only a sample counter runs; nothing is recorded or processed.

| Command | Args | Returns |
|---|---|---|
| `start_drift_check` | `{ deviceId: string, cameraId: string, intervalS: number }` | `DriftStatus` (rejects with `CameraError` like `start_sync_test`) – opens the mic for the check and opens the first clap round; emits `sync-progress` and `camera-preview` during rounds. `intervalS` is clamped to 1 s – 3 h (UI offers 10 / 30 / 60 min) |
| `start_drift_clap` | – | `()` – opens the next clap round. Error "Start the drift check first." |
| `finish_drift_clap` | – | `DriftStatus` – closes the round, measures it (`coach_core::avsync::measure_offset`), adds it to the series. Errors "Start the drift check first." / "Press Clap now first." |
| `cancel_drift_clap` | – | `DriftStatus` – drops the open clap round without measuring it (Cancel / "I can't do this" / an error in a later round); earlier points and the mic stay. Error "Start the drift check first." |
| `drift_status` | – | `DriftStatus \| null` – null when no drift check runs (the UI uses it to come back to a running check) |
| `stop_drift_check` | – | `()` – ends the whole check ("Stop the check", done, first round cancelled, app exit); safe any time, also twice. `start_sync_test` and a second `start_drift_check` also end a running one |

The mic is opened in shared mode, so the mic test, mic preview and coaching can run while a drift check holds it. The camera is
only read during a clap round; `start_drift_clap` stops a running camera preview.

```ts
DriftStatus = { interval_s: number, elapsed_s: number,
                next_due_in_s: number | null,  // until the next round; null while a round is open or before the first point
                clap_open: boolean,
                points: { t_s: number, offset_ms: number }[],  // t_s on the mic clock; offset without OBS' own Sync Offset
                last: SyncReport | null,       // latest round (hint when unclear; fix.can_auto only when the offset holds)
                report: DriftReport }
DriftReport = { verdict: "steady" | "drifting" | "unclear",
                reason: "too_few" | "noisy" | "too_short" | null,   // when unclear
                message: string,               // plain English
                steps: string[],               // drifting: one clock, 48 kHz, one capture path, re-measure; unclear: how to get an answer
                fixed_offset_ok: boolean,      // steady: a fixed Sync Offset / Render Delay will hold
                short: boolean,                // steady over less than 20 min (message says a longer check is surer)
                finding: Finding | null,       // id "sync.av_drift" when drifting
                // Advanced only:
                slope_ms_per_10min: number, change_ms: number, span_min: number, points: number, spread_ms: number | null }
```
Event `drift-due`: `{ points }`, once when the wait after the latest good round is over (not sent once drifting).

Verdict (`coach_core::avsync::estimate_drift`, least-squares line; starting values): `drifting` when |slope| ≥ 10 ms per 10 min and the
offset moved ≥ 20 ms; `steady` when |slope| < 10 ms per 10 min; `unclear` with fewer than two rounds, a scatter above 30 ms (three or
more rounds), or a fast slope that moved less than 20 ms (`too_short`). `sync.av_drift` is major from 15 ms per 10 min.
`apply_sync_fix` only works after a drift round when the verdict is `steady` (the backend keeps the latest round's fix then);
otherwise it says to run the clap test first.

Flow (in the clap-test screen): any clap result with a number offers "Will it stay this way?" with 10 / 30 / 60 min →
`start_drift_check` → clap round → `finish_drift_clap` → wait screen with countdown ("Clap now" enabled when due; the user streams
as usual and the check keeps running if they leave the screen) → `start_drift_clap` → … `steady`: "The gap holds" + "Show me the
fix" (the clap result, with "Set it in OBS for me" when possible). `drifting`: title/why from the finding, `steps`, "Check again"
(new check after a change) and "I can't do this" (re-check before each stream). Numbers only under Advanced.

**Camera runtime warning.** `QuickChecks` gained `runtime_warnings: RuntimeWarning[]` (`#[serde(default)]`):
`RuntimeWarning = { camera: string, typical_min: number, connected_min: number, minutes_left: number, message: string /* plain English */ }`.
A camera appears once its recorded run times show a pattern (`typical_runtime_minutes`) and the current run has used 80 % of it
(`near_runtime_limit`). `connected_min` comes from the USB probe (`UsbLink.connected_min`: minutes since Windows last started the
device; null when Windows has no start event in the last 7 days, then there is no warning). The UI shows it on Home ("Re-check
before stream") and on the quick scan.

Also fixed in the UI: `camera-preview` payload is `{ image, paper_guide }` per this file, the camera screen now reads `.image`.
