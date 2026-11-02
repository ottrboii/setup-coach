# Setup Coach

Windows app that coaches streamers to a properly set-up mic, camera and stream —
**no AI processing**: fix the physical setup first, then apply plain static
settings, then re-measure to verify.

Parked inside `channel-hub` until it gets its own repo. Research and decisions:
[`../notes/setup-coach-research-brief-2026-11-02.md`](../notes/setup-coach-research-brief-2026-11-02.md).

## Layout

| Path | What |
|---|---|
| `core/` | `coach_core` Rust crate: all analysis and recommendation logic, platform-independent and fully tested. |
| `profiles/` | Device database (JSON): mics, interfaces, boosters, webcams, capture-card camera bodies, generic fallbacks. Embedded into the app. |
| `app/` *(next)* | Windows app (Tauri): WASAPI capture, obs-websocket, UVC control, UI. |
| `vst3/` *(next)* | Our VST3 effect for OBS: high-pass, parametric EQ, de-esser, mono. |

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
cd core
cargo test --release            # all tests
cargo run --release --example calibrate          # every synthetic fault scenario
cargo run --release --example calibrate -- room  # one scenario
```

## Status

- Thresholds are starting values from research, calibrated on synthetic
  recordings; they must be re-tuned on real recordings (test corpus) and
  reviewed by an audio engineer.
- No device profile is `verified` yet; the owner's gear (C920, RE20, NT1-A,
  GoXLR, Cam Link 4K + a6000, DJI Mic 3, headsets, phone) is first in line.
