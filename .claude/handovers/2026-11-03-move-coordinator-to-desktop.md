# 2026-11-03 — Coordinator moves from cloud to Otto's desktop

Otto wants the coordinator to run in **Claude desktop (Code tab) on his Windows PC**, folder `C:\setup-coach`, instead of the cloud. Windows PCs can't be attached to cloud sessions, and a local session can run the app, the self-test and `cargo test` against real hardware directly.

## Done (all on main, CI green — run 41612468387)
- #1 VST3 bundled in the installer (perMachine, `app/src-tauri/installer-hooks.nsh`, `tauri.vst3.conf.json`), CI 7-Zip check that the VST3 is packed.
- #2 CI installer end-to-end (`.github/scripts/installer-e2e.ps1`): silent install, VST3 in place, app alive 10 s, silent uninstall removes everything.
- #3 `setup-coach.exe --self-test` → zip on Desktop (`app/src-tauri/src/selftest.rs`), skips the OBS apply test while streaming/recording.
- Otto installed the new installer on his PC.

## In flight (cloud subagents; their branches/PRs land on GitHub)
- `feat/auto-updater` (release-ci): tauri-plugin-updater from GitHub Releases `latest.json`, native dialog, asks to close OBS first, `check_for_update`/`install_update` commands, `.github/workflows/release.yml` creating **draft** releases, `RELEASING.md`. Pubkey via repo variable `TAURI_UPDATER_PUBKEY`, private key via secrets — Otto must generate keys (see RELEASING.md); never commit keys.
- `feat/settings-vault-ab` (app-backend): `%APPDATA%\SetupCoach\backups\` snapshot-before-every-write with a never-overwritten `original`, `list_backups` / `restore_backup`; real A/B: `original.wav` on first mic test per device, `record_final`, loudness-matched `get_ab`, `play_preview` "original"/"final", `export_ab` to Desktop.
- Next after that: app-ui for the update button, "Put everything back how it was", and the A/B player (contract in app/API.md from the vault PR).
Check open PRs on GitHub; if a branch exists without a PR, or CI is red, pick it up.

## First real-hardware finding: self-test (2026-11-03 10:34 UTC, Otto's PC)
Windows build 26200.9711 (25H2). Result: 4 pass, 5 fail, 3 skip.
- PASS: versions, 5 mics found (Windows default mic = "Microphone (C922 Pro Stream Webcam)" — note: Otto's webcam is a **C922**, not C920; and the default mic being the webcam is itself a coaching finding), VST3 installed, saved setup (none yet).
- FAIL by **5 s timeout**: Windows AI effects, Cameras, OBS is open, OBS WebSocket, GoXLR Utility. OBS skips followed from the websocket fail.
- 5 × 5 s ≈ the 25 s Otto saw ("it waited for OBS"): it didn't wait for OBS, he opened it during the timeouts.
- Prime suspect: the checks that failed shell out to `tasklist` (`platform/windows/mod.rs::running_processes`, used by `processing_apps` twice, `obs_running`, GoXLR detection) and `sc query FrameServer` (`windows_audio`). Measure on the PC (`Measure-Command { tasklist /FO CSV /NH }`), check `duration_ms` in the zip's `self-test.json`, then replace with native APIs (CreateToolhelp32Snapshot / OpenSCManager+QueryServiceStatus), list processes once per run, and look for any shared lock/COM hang in cameras. Zip: `C:\Users\ottr\Desktop\SetupCoach-selftest-20261103-103458.zip`.
- Also add `c922` handling to profiles if missing (device-profiles).

## Then
Smoke test (`SMOKE_TEST.md`) with real mics → feedback triage; known gaps in PROJECT_OVERVIEW.
