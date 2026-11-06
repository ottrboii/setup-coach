# Setup Coach — Project overview
_Updated 2026-11-06. Owner: Otto (Ottr Gaming Influencer AB). Coordinator session: SETUP COACH: COORDINATOR._

## What it is
A Windows app that coaches non-technical streamers (the "bought an SM7B and left it on defaults" crowd) to a properly set-up **mic, camera and stream**. Philosophy: **no AI processing** — fix the physical setup first with live, animated coaching, then apply plain static settings (OBS filters + our VST3, Windows settings, camera controls, GoXLR via GoXLR Utility), then re-measure to verify. Business: up-front purchase + affiliate links at the moment a physical limit is measured (or affiliate-only for volume).

## Status
First full build done and green on Windows CI (installer + VST3 artifacts). Not yet run on real hardware — **next milestone: Otto's smoke test** (`SMOKE_TEST.md`) with his gear: RE20 on GoXLR (GoXLR Utility), NT1-A, DJI Mic 3, headsets, C920, a6000 via Cam Link 4K, phone.
- Merged since 2026-11-02: auto-updater (#5), settings vault + A/B (#6), self-test mode (#3, #7), installer e2e (#2), review fixes (#8), vault/A-B UI in mock mode (#9). The new UI has not been run against the real backend.
- Connection health (#22): USB link speed/hub/drop-out findings and who holds the camera or mic (`core/src/system/connection.rs`, `usb_probe.ps1`).
- Camera/mic extras (#23): HDMI chain checklist (`video/hdmi.rs`), camera-body-not-a-webcam, "switches off after a while" runtime-limit pattern (`video/guard.rs`), lip-sync core (`core/src/avsync.rs`), mic vibration (`audio/vibration.rs`).
- Lip sync (#25): clap test screen (`app/ui/screens/sync.js`), commands `start/finish/stop_sync_test`, `apply_sync_fix` (sets OBS Sync Offset on the mic input via obs-websocket) and `undo_sync_fix`; camera runtime pre-stream warning; `connected_min` in quick checks. See `app/API.md`.
- Capture-card checks (#26, `system/capture.rs`): Cam Link on a non-USB3 path, Sony HDMI fps family and interlace, double capture audio.
- Research: `docs/field-complaints/*.md` (camera, mic-audio, stream-obs, dual-pc with gap list, #24).
- Core engine: mic/camera analysis, findings, processing chain + verification, system/OBS audit, stream health, device profiles, live coaching engine.
- App: Tauri 2 backend with Windows platform code (type-checked, CI-built, never run on a real PC yet) + static UI with live animated coaching and a full mock mode.
- VST3: mono / high-pass / EQ / de-esser, SDK validator passes.
- Known gaps: camera re-apply on re-plug, Mic Boost read, Studio Effects / Multi-App Camera detection, real camera fps, one-time flicker check false positives on static bright objects, unsigned installer.

## Stack and how to run
Rust workspace (`core/` = `coach_core`, `app/src-tauri/` = `setup-coach-app`), static HTML/CSS/JS UI (`app/ui/`, no build step), C++17 VST3 (`vst3/`, Steinberg SDK 3.8 MIT via CMake FetchContent), device data in `profiles/*.json`.
- Test: `cargo test --workspace --release` (≈225 core tests plus backend integration tests; mock platform on non-Windows).
- Windows check from Linux: `cargo check --target x86_64-pc-windows-msvc -p setup-coach-app`.
- UI preview without the app: open `app/ui/index.html` in a browser (mock mode).
- Build VST3 first (Windows): `cmake -S vst3 -B build-vst3 -DSC_BUILD_PLUGIN=ON && cmake --build build-vst3 --config Release`. Then the installer, which bundles it and installs it to `%CommonProgramFiles%\VST3` (admin/UAC once): `cd app && npx @tauri-apps/cli@2 build --config src-tauri/tauri.vst3.conf.json`. Without `--config` the installer is built without the VST3.
- CI: `.github/workflows/ci.yml` builds both and uploads them as artifacts.
- Installer end-to-end: CI silently installs the built installer, checks the app + VST3 files, launches the app for 10s, then uninstalls and checks everything is gone (`.github/scripts/installer-e2e.ps1`).
- Local checkout on Otto's PC: `C:\setup-coach`.

## Where things live
| Area | Path | Agent |
|---|---|---|
| Analysis, findings, chain, coaching engine | `core/src/{audio,video,system,health,coach}` | core-engine |
| Device database | `profiles/*.json`, `core/src/profiles/` | device-profiles |
| App backend, Windows integration, OBS, GoXLR | `app/src-tauri/` | app-backend |
| UI | `app/ui/` | app-ui |
| VST3 effect | `vst3/` | vst3-plugin |
| CI, installer, signing, releases | `.github/`, `app/src-tauri/tauri.conf.json` | release-ci |
| Contract between backend and UI | `app/API.md` | app-backend + app-ui (coordinator approves changes) |

## Decisions (see docs/research-brief.md for the why)
- Windows only. Standalone app + obs-websocket + our VST3 — **not** an OBS plugin (GPL). Virtual mic driver is v2 (license Thesycon TVirtAudio or SysVAD), only when Streamlabs/Discord/TikTok demand is proven.
- GoXLR owners: recommend **GoXLR Utility** first (official app unmaintained; both can't run together).
- Coaching must keep users in the app: live updates, animations, auto-adjust where possible, open other apps for them and come back when the change is detected.
- Exposure/WB metered on a white paper (skin-tone adaptive). EQ capped at ±6 dB; beyond that the fix is physical.
- Company for signing: Ottr Gaming Influencer AB (Azure Artifact Signing needs org + ~3 yr history, else OV cert).

## Open questions
- Real-hardware calibration of thresholds (test corpus + audio engineer review + blind listening tests).
- Unverified profile facts (address direction for some mics, USB IDs, a6000 clean HDMI, DJI Mic 3 processing names).
- Wave Link read-only integration (unofficial local WebSocket, port 1824+).
- Pricing, product name final, code signing route.
- Clap test and new connection/capture-card checks are unverified on hardware (Sony a6000 + Cam Link 4K).
- NDI / two-PC role pairing and remote obs-websocket host are not built (see `docs/field-complaints/dual-pc.md`).
- OBS Sync Offset is not stored in the settings vault.

