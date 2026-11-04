# 2026-11-03 — Coordinator moves from cloud to Otto's desktop

Otto wants the coordinator to run in **Claude desktop (Code tab) on his Windows PC**, folder `C:\setup-coach`, instead of the cloud. Windows PCs can't be attached to cloud sessions, and a local session can run the app, the self-test and `cargo test` against real hardware directly.

## Done (all on main, CI green — run 41612468387)
- #1 VST3 bundled in the installer (perMachine, `app/src-tauri/installer-hooks.nsh`, `tauri.vst3.conf.json`), CI 7-Zip check that the VST3 is packed.
- #2 CI installer end-to-end (`.github/scripts/installer-e2e.ps1`): silent install, VST3 in place, app alive 10 s, silent uninstall removes everything.
- #3 `setup-coach.exe --self-test` → zip on Desktop (`app/src-tauri/src/selftest.rs`), skips the OBS apply test while streaming/recording.
- Otto installed the new installer on his PC.

## Also merged 2026-11-03
- #5 auto-updater (`app/src-tauri/src/updater.rs`, `.github/workflows/release.yml` → **draft** releases, `RELEASING.md`). Not active until Otto does the one-time key setup in RELEASING.md (secrets `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, variable `TAURI_UPDATER_PUBKEY`). Untested with real keys: `.sig` creation, release.yml, signature check, install/restart, Yes/No dialogs.
- #6 settings vault + real A/B (`app/src-tauri/src/vault.rs`, `wav.rs`; API.md "Settings vault & A/B"): backup before every write with a never-overwritten `original`, `list_backups` / `restore_backup`, `record_final`, loudness-matched `get_ab`, `play_preview` "original"/"final", `export_ab`. Self-test zips leave `recordings/*.wav` out unless `--include-recordings`. Needs a real-PC check: Windows settings read/restore, OBS restore round-trip, a real final take.

## Next
1. Fix the self-test timeouts below (app-backend), re-run the self-test on this PC.
2. app-ui: update button (`check_for_update`/`install_update`), "Put everything back how it was" (`list_backups`/`restore_backup`, show `skipped` as notes), A/B player + export after the final read.
3. Otto's one-time updater key setup (RELEASING.md), then first release v0.2.0 (draft → Otto publishes) and install it by hand once.

## First real-hardware finding: self-test (2026-11-03 10:34 UTC, Otto's PC)
Windows build 26200.9711 (25H2). Result: 4 pass, 5 fail, 3 skip.
- PASS: versions, 5 mics found (Windows default mic = "Microphone (C922 Pro Stream Webcam)" — note: Otto's webcam is a **C922**, not C920; and the default mic being the webcam is itself a coaching finding), VST3 installed, saved setup (none yet).
- FAIL by **5 s timeout**: Windows AI effects, Cameras, OBS is open, OBS WebSocket, GoXLR Utility. OBS skips followed from the websocket fail.
- 5 × 5 s ≈ the 25 s Otto saw ("it waited for OBS"): it didn't wait for OBS, he opened it during the timeouts.
- Prime suspect: the checks that failed shell out to `tasklist` (`platform/windows/mod.rs::running_processes`, used by `processing_apps` twice, `obs_running`, GoXLR detection) and `sc query FrameServer` (`windows_audio`). Measure on the PC (`Measure-Command { tasklist /FO CSV /NH }`), check `duration_ms` in the zip's `self-test.json`, then replace with native APIs (CreateToolhelp32Snapshot / OpenSCManager+QueryServiceStatus), list processes once per run, and look for any shared lock/COM hang in cameras. Zip: `C:\Users\ottr\Desktop\SetupCoach-selftest-20261103-103458.zip`.
- Also add `c922` handling to profiles if missing (device-profiles).

## Then
Smoke test (`SMOKE_TEST.md`) with real mics → feedback triage; known gaps in PROJECT_OVERVIEW.
