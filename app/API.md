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
                    profile: MatchResult }
CameraDeviceInfo = { id: string, name: string, obs_source: string | null,
                     profile: MatchResult }
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
| `verify_mic` | – | `{ before: MicReport, after: MicReport }` – re-records? no: renders the last recording through the applied chain and re-analyses |
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

`apply_mic_chain` writes the chain to `%APPDATA%\SetupCoach\chain.json` (read
live by our VST3: mono, HPF, EQ, de-esser), makes sure our VST3 filter exists on
the OBS mic input, and creates/updates OBS built-in filters for gain, expander,
compressor and limiter in the right order via obs-websocket. Hardware gain
changes are coached, never applied.

## Camera test

| Command | Args | Returns |
|---|---|---|
| `camera_state` | `{ cameraId: string }` | `CameraState` (core video) |
| `start_camera_test` | `{ cameraId: string }` | `()` – emits `camera-preview` (small JPEG data URL, ~4/s) |
| `capture_camera_step` | `{ step: "neutral" \| "turn_head" \| "hold_paper" \| "lean_in_out", seconds: number }` | `{ frames: number, faces_found: number }` |
| `finish_camera_test` | `{ constraints: Constraint[] }` | `CameraReport` |
| `apply_camera_settings` | `{ cameraId: string, settings: CameraSettings }` | `ApplyResult` |

```ts
CameraReport = { metrics: VideoMetrics, findings: Finding[], settings: CameraSettings,
                 score: Score, class: CameraClass }
```
Frames come from OBS (`GetSourceScreenshot` on the camera source) when OBS
holds the camera, else from the camera directly. Face rectangles come from the
Windows built-in face detector (metering only). The paper rectangle is the
on-screen guide box the user fills with a white sheet during `hold_paper`.

## Stream health

| Command | Args | Returns |
|---|---|---|
| `start_health_monitor` | – | `()` – polls OBS every 2 s; emits `health-alert` (core `Alert`) and `health-sample` |
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
