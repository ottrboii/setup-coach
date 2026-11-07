# Setup Coach Voice (VST3)

A small VST3 audio effect that adds the voice processing **OBS doesn't ship**:

| Stage | Notes |
|---|---|
| Mono downmix | With `mono_downmix`, a channel > 30 dB below the loudest (interface input 1 arriving as left-only stereo) is dropped instead of averaged in, so the voice keeps full level in both ears. Otherwise channels are averaged. |
| High-pass | 12 or 24 dB/oct (one or two RBJ biquads, Q = 0.707). |
| EQ | Up to 5 bands: peak, low shelf, high shelf (RBJ cookbook). |
| De-esser | Relative split-band: complementary split (`x = low + high`, low-pass at 0.7·freq), band-pass side chain (Q 1) at `freq`; when the 'ess' share of total energy exceeds `threshold_db`, highs drop by `(share − threshold)·2` dB, capped at `max_reduction_db`. |

The output is the processed mono signal on every output channel. Gain, expander,
compressor and limiter stay in OBS's built-in filters, which the Setup Coach app
configures over obs-websocket. No AI processing.

The formulas are identical to `core/src/audio/{dsp,process}.rs`. The Rust renderer
is the reference used for before/after previews and verification.

## Settings: `chain.json`

The plug-in has no GUI and only one parameter (**Bypass**). Settings come from
the Setup Coach app, which writes the chain it computed (the serde JSON of
`coach_core::audio::ProcessingChain`) to:

- Windows: `%APPDATA%\SetupCoach\chain.json`
- elsewhere (tests): `$XDG_CONFIG_HOME/SetupCoach/chain.json` or `~/.config/SetupCoach/chain.json`
- override: `SETUP_COACH_CHAIN=<path>`

The plug-in reads `mono_downmix`, `highpass_hz`, `highpass_slope_db_oct`, `eq[]`
(`kind`, `freq_hz`, `gain_db`, `q`) and `deesser` (`freq_hz`, `threshold_db`,
`max_reduction_db`); every other field is ignored. Values are clamped to safe
ranges and at most 5 EQ bands are used.

The de-esser's `freq_hz` is the centre of its 'ess' band, so it can sit on the
measured "s" peak (the core composer clamps it to 4.5-9 kHz; the plug-in accepts
2-12 kHz). A `deesser` block without `freq_hz` (or with a null/non-numeric
one, same for `threshold_db` and `max_reduction_db`) keeps the original 6.5 kHz
centre, so older chain.json files sound as before.

- A background thread checks the file's timestamp and size about twice a
  second. When it changes, the thread parses it and builds the new filter state
  off the audio thread, then hands it over lock-free. The audio thread
  crossfades from the old settings to the new ones over 20 ms, so there are no
  clicks.
- If the file is missing or invalid, the plug-in passes audio through
  bit-exactly.
- The app should write the file atomically: write `chain.json.tmp`, then rename it.

## Loading it in OBS

- **OBS with native VST3 support:** the VST3 host filter (obsproject/obs-studio
  PR #12752) was approved for the ~33.1 milestone. Once it ships, add
  *Setup Coach Voice* as a VST3 filter on the mic source.
- **Until then:** OBS's built-in VST filter is VST 2.x only. Use the free
  **atkAudio** OBS plug-in (AGPL, installed by the user) as the VST3 host. Our
  plug-in is a separate binary loaded at runtime, so its licence is not affected.
- **Recommended filter order on the mic source:** Gain → **Setup Coach Voice** →
  Expander → Compressor → Limiter. The Rust reference puts the de-esser after
  the compressor. Here it runs before the compressor, which is fine because it
  is relative (level-independent).

Install path (per machine): `C:\Program Files\Common Files\VST3\SetupCoachVoice.vst3`
(bundle folder). The app's installer copies it there.

## Build

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release            # DSP tests only (Linux default)
cmake -S . -B build -DSC_BUILD_PLUGIN=ON                   # + plug-in (default ON on Windows)
cmake --build build --config Release
ctest --test-dir build -C Release
```

- **VST 3 SDK:** `v3.8.0_build_66`, fetched with CMake FetchContent. It is
  **MIT-licensed** (see the SDK's `LICENSE.txt`), so a closed-source plug-in is
  fine. Pass `-DVST3SDK_DIR=<checkout>` to use a local copy.
- **JSON:** nlohmann/json v3.11.3 (MIT), fetched by hash.
- **Tests:** `sc_dsp_tests` covers:
  - high-pass attenuation vs. the analytic response
  - EQ gains within 0.1 dB of the analytic response
  - left-only input reaching both ears at full level
  - the de-esser: more than 4 dB reduction on 5–9 kHz noise, 1 kHz untouched,
    at 48 and 44.1 kHz
  - exact pass-through with no file and under host bypass
  - parsing, clamping and rejection of `chain.json`
  - the file watcher
  - click-free settings switches

  When the plug-in is built, `vst3_validator` also runs Steinberg's validator
  (47 tests) on the bundle.

Plug-in ID (never change): `2FE397E4-AF0F4DA3-A3CC5EA0-70FE052D`, vendor
"Ottr Gaming Influencer AB", category `Fx|EQ`.
