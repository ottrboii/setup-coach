# Setup Coach — working rules

Read `PROJECT_OVERVIEW.md` first, then `docs/research-brief.md` for the why.

## Product rules (non-negotiable)
- **No AI processing** of audio or video, ever (no AI denoise, voice enhancement, beauty/blur). Detect and turn off AI effects users already have (Windows Voice Clarity, RNNoise/NVIDIA/Krisp, Studio Effects).
- Physical fix first, then static settings, then **re-measure to verify**.
- Plain language for the user. Numbers only in "Advanced".
- Coaching keeps users in the app: live updates, animation, auto-adjust where software can, open other apps for them and detect completion. Never "did you do it?".
- "I can't do this" is always allowed and triggers recalibration (`Constraint`).
- Thresholds are starting values: change them only with evidence (test corpus, synthetic scenario, or real recordings) and keep tests green.

## Engineering rules
- `cargo test --workspace --release` green before handing back; also `cargo check --target x86_64-pc-windows-msvc -p setup-coach-app` for backend changes and `cargo clippy --workspace` with no errors.
- Windows-only code lives behind `cfg(windows)` with a mock in `app/src-tauri/src/platform/mock.rs` so everything is testable on Linux.
- `app/API.md` is the backend ⇄ UI contract. Change it deliberately and in the same change as both sides (or flag it in the hand-back).
- Device facts in `profiles/*.json`: only numbers you can source; otherwise `null` and `verified: false`. Mark `verified: true` only after testing on real hardware.
- No secrets in the repo. Signing keys and API tokens never committed.
- Work on a branch, open a PR to `main`; CI must be green.

## Coordinator (session "SETUP COACH: COORDINATOR")
The coordinator owns the plan, `PROJECT_OVERVIEW.md`, `app/API.md` approvals and `.claude/handovers/`. It:
1. Breaks work into tasks and delegates to the agents in `.claude/agents/` (core-engine, app-backend, app-ui, vst3-plugin, device-profiles, release-ci), running independent ones in parallel with clear file ownership.
2. Integrates and verifies their work (tests, Windows check, CI), commits/opens PRs, and keeps the overview's Status and Open questions current.
3. Turns Otto's smoke-test feedback (screenshots, `%APPDATA%\SetupCoach\` zip, opinions per mic) into tasks: profile verification, threshold re-tuning, UX fixes.
4. Writes a handover note in `.claude/handovers/YYYY-MM-DD-<topic>.md` when a session ends mid-work.
Never: publish releases, sign up for services, spend money, or push to `main` without CI green — ask Otto.
