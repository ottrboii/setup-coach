# Setup Coach

Windows app that coaches streamers to a properly set-up mic, camera and stream —
**no AI processing**: fix the physical setup first, then apply plain static
settings, then re-measure to verify.

Owner: Ottr Gaming Influencer AB (Otto). Project overview: [`PROJECT_OVERVIEW.md`](PROJECT_OVERVIEW.md).
Research and decisions: [`docs/research-brief.md`](docs/research-brief.md).
Moved out of `channel-hub` on 2026-11-02 with full history.

## Layout

| Path | What |
|---|---|
| `core/` | `coach_core` Rust crate: all analysis and recommendation logic, platform-independent and fully tested. |
| `profiles/` | Device database (JSON): mics, interfaces, boosters, webcams, capture-card camera bodies, generic fallbacks. Embedded into the app. |
| `app/` | Windows app (Tauri 2): `src-tauri/` backend (WASAPI, obs-websocket, UVC, GoXLR Utility, live coaching), `ui/` static UI. Contract: `app/API.md`. |
| `vst3/` | Our VST3 effect for OBS: mono, high-pass, parametric EQ, de-esser (reads `chain.json`). |

## Core modules

- `audio` — guided mic test → measurements (levels, noise & character, hum,
  bandwidth, tone vs. a reference speech curve, room decay, desk reflections,
  plosives, sibilance, typing, desk thumps, cable glitches, level swings,
  auto-level) → plain-language findings → processing chain
  (gain → HPF → EQ ≤ ±6 dB → expander → compressor → de-esser → limiter) with
  loudness calibration → offline render for before/after and verification.
  `audio::synth` generates guided-test recordings with injectable faults.
- `video` — guided camera test (paper-metered, skin-tone-adaptive exposure and
  white balance, flicker, focus, fps, lighting shape, framing, colour range) →
  findings → locked camera settings; coach-only mode for capture-card cameras.
- `system` — Windows / OBS / camera-pipeline / encoder config audit, change
  detection between launches, and auto-fix `Action`s for the app to execute.
- `health` — live stream-health monitor (network vs. encoding vs. rendering)
  and post-stream report.
- `profiles` — profile schema, loader and matcher (USB id → Windows name →
  generic fallback + photo questions).
- `finding`, `score`, `constraints` — shared types: findings, 0–100 scores,
  "I can't do this" constraints that trigger recalibration.

## Run

```sh
cargo test --workspace --release   # all tests (core + app with mock platform)
cd core
cargo run --release --example calibrate          # every synthetic fault scenario
cargo run --release --example calibrate -- room  # one scenario
```

## Status

- Thresholds are starting values from research, calibrated on synthetic
  recordings; they must be re-tuned on real recordings (test corpus) and
  reviewed by an audio engineer.
- No device profile is `verified` yet; the owner's gear (C920, RE20, NT1-A,
  GoXLR, Cam Link 4K + a6000, DJI Mic 3, headsets, phone) is first in line.
