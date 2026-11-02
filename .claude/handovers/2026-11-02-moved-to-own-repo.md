# 2026-11-02 — Setup Coach moved to its own repo

Built in `ottrboii/channel-hub` (branch `ccr-37ca5b74-vaqfwj`, folder `setup-coach/`) and moved here with full history via `git subtree split`.

Done: research (docs/research-brief.md), core engine (analysis, findings, chain + verification, system/OBS audit, health, profiles, live coaching), VST3, Tauri backend (Windows code type-checked + CI-built), UI with live animated coaching + mock mode, CI producing installer + VST3 artifacts.

Next for the coordinator:
1. Get CI green in this repo (paths changed from `setup-coach/` to repo root).
2. Otto's smoke test (`SMOKE_TEST.md`) → turn feedback into tasks.
3. Bundle the VST3 into the installer (release-ci), so users don't copy it by hand.
4. Fix: one-time flicker check false positives on static bright objects (core-engine); camera re-apply on re-plug; Mic Boost read; Studio Effects / Multi-App Camera detection (app-backend).
5. Wave Link read-only integration (unofficial local WebSocket, port 1824+) after checking a client library's source.
