# Voice tuning design: "Make it sound better"

_Design only, 2026-11-07. Owner: core-engine. Nothing here changes code or thresholds yet._

## For Otto, in plain words

Today the app fixes faults (boom, mud, harshness, hiss) and stops there. This design makes the voice
sound **good**, not just "not broken", from three separate inputs:

1. **Your voice.** From the test reading we work out how deep your voice is, whether it is bright or dark,
   how strong your "s" sounds are and whether you pop your "p"s. That picks the low-cut, the "s" tamer
   and how much tone shaping is safe.
2. **What you are making.** Streaming, Recording/voiceover and Call each get their own loudness and
   compression, and a call never boosts sound the call app throws away anyway.
3. **Your mic.** Each mic model colours sound in a known way. The maker's published chart tells us how,
   so we can undo part of it. Nothing gets measured from your mic or anyone's recordings as "the right sound".

The goal ("the target") comes only from published research and standards: how average speech is
balanced, audiobook and podcast loudness rules, and the makers' charts. Your recordings are used only to
check that the processing behaves sensibly, never as the goal: your mic isn't perfect, so it can't be the
reference.

Rules we keep: physical fixes first; tone moves stay gentle (never more than 6 dB anywhere, all moves
added together); we **keep your voice's character** (a deep voice stays deep, we only fix what the gear
and the room did); after applying we **measure again** to check it landed.

What you will see: a **Make it sound better** button after the setup checks. You read a few lines, hear
**Before / After** at the same loudness (so "after" can't win just by being louder), and choose
**Keep**, **Gentler** or **Undo**. Numbers only appear under Advanced.

What still needs proof: the small "per-use" tone differences (for example a touch more clarity for
streaming) have no published number behind them. They stay off until a blind listening test shows
people prefer them. Every mic chart starts as "published, not checked on a real mic".

---

## 1. Where we start (current code)

- `audio::analyze` measures f0 (`f0_hz`), third-octave levels and their deviation from
  `REFERENCE_LTASS` (Byrne et al. 1994 [1]) adjusted by f0 (`reference_for_f0`), a `ToneSummary`
  per region, `sibilance_db` (p98 of 5–9 kHz vs speech RMS), `plosive_events`/`plosive_worst_db`.
- `audio::chain::compute_chain` picks HPF from f0 (70/80/100/120 Hz at f0 < 110/150/200/≥200),
  `derive_eq` cuts regions > +3 dB by 0.7× deviation and boosts small (2–3 dB caps), `EQ_LIMIT_DB = 6`
  per band, de-esser fixed at 6.5 kHz when `sibilance_db > −3`, dynamics as research brief §6.
- `SetupProfile` changes pass bars only; `core/tests/setup_profiles.rs` asserts the chain is identical
  across profiles. This design changes that **deliberately** (piece P3 replaces the test, section 9).
- `profiles/mics.json` has an `eq_prior` per mic: unsourced (forum/blog links), unverified, and not
  read by `compute_chain` (only a schema test uses it). It is replaced, not reused (section 3.2).
- VST3: mono, HPF, ≤ 5 EQ bands (`kMaxEqBands = 5`, `vst3/dsp/voice_chain.hpp`), de-esser, from `chain.json`.

## 2. Principles

1. **Correct the gear, keep the voice.** The target is a *tolerance band* around the voice-adjusted
   speech curve, not the curve itself. We only correct what lies outside ±3 dB (today's `derive_eq`
   threshold), and only 70 % of it. A dark voice through a flat mic stays darker than average.
2. **Targets from published sources only** (section 8). Otto's and any user's recordings validate behaviour
   (no new findings, residual shrinks, nothing at the cap), never define the target.
3. **Known before measured.** Subtract what we know (mic chart, switch position) *before* reading the
   residual, so the voice and room are not blamed for the mic, and the mic is not corrected twice.
4. **One budget.** Every EQ move, including mic correction, shares the same ±6 dB cap (section 5.3).
5. **Do no harm, then verify.** Re-render and re-analyse; drop any move that makes something worse.
6. **Unverified data is used at reduced weight, never presented as fact.**

## 3. Data model

### 3.1 `VoiceType` (new, `core/src/audio/voice.rs`)

```rust
pub struct VoiceType {
    pub f0_median_hz: Option<f64>, pub f0_p10_hz: Option<f64>, pub f0_p90_hz: Option<f64>,
    pub pitch: PitchClass,            // Deep | LowMid | HighMid | High | Unknown
    pub tilt_db_per_oct: Option<f64>, // residual slope 250 Hz–8 kHz AFTER known mic correction
    pub tilt: TiltClass,              // Dark | Neutral | Bright (display only)
    pub sib_level_db: Option<f64>,    // = AudioMetrics::sibilance_db
    pub sib_peak_hz: Option<f64>,     // spectral peak of sibilant frames, 4–10 kHz
    pub sibilance: SibClass,          // Soft | Normal | Strong
    pub plosives_per_min: Option<f64>,
    pub plosives: PlosiveClass,       // None | Some | Many
    pub confidence: Confidence,       // Low when < 8 s voiced speech, bandlimited, or clipped
}
```

Classes are for plain-language display; the chain uses the continuous values.

### 3.2 Mic response in `profiles/mics.json` (replaces `eq_prior`)

```json
"response": {
  "points_hz_db": [[100, -4.0], [125, -2.5], ..., [10000, 1.0]],
  "reference": "dB re 1 kHz, on-axis, as published",
  "measured_at_cm": null,
  "pattern_b": 0.5,
  "proximity": "gradient",
  "switch_curves": { "bass_tilt": { "1": [[100, -6.0], ...] } },
  "source": "https://pubs.shure.com/guide/SM7B/en-US, 'Typical frequency response' chart, digitised at 1/3 octave",
  "digitised_precision_db": 1.0,
  "verified": false
}
```

- `points_hz_db`: third-octave points 100 Hz–10 kHz only, read from the maker's chart. No chart → the
  whole `response` is `null` (no mic correction; residual only).
- `pattern_b`: gradient share of the first-order pattern (omni 0, cardioid 0.5, figure-8 1) [12].
  `proximity`: `gradient` | `reduced` (Variable-D, e.g. RE20 [13]) | `none` (omni/headset) | `null`.
- `switch_curves`: per `controls[].key` and option index, the published curve for that position.
- `verified: true` only after a hardware check against a measurement mic (open question 5).

### 3.3 Per-profile tuning (`SetupProfile::tuning()`, new)

| | Streaming | Recording / voiceover | Call / meeting |
|---|---|---|---|
| Tonal target | voice-adjusted LTASS [1] | same | same, judged 100 Hz–7 kHz only |
| Profile tone delta (P) | +1 dB 2–4 kHz **[unverified, off]** | 0 | +1 dB 1–4 kHz **[unverified, off]** (speech-importance weighting [9]) |
| Boosts above 7 kHz | allowed (≤ 2 dB, today's cap) | allowed | none: wideband call codecs stop at 7–8 kHz [7][8] |
| Voice loudness | −16 LUFS short-term (unchanged; cf. Apple −16 [3], AES TD1008 speech −18 +2 LU [4]) | −20 dB RMS (inside ACX −23…−18 [2]) | −16 LUFS (unchanged; no published call norm found) |
| Limiter ceiling | −1.5 dBFS (unchanged) | −3.5 dBFS (ACX peaks < −3 [2], 0.5 dB margin; OBS limiter has no look-ahead) | −1.5 dBFS |
| Compressor | 3:1, ~4 dB GR (unchanged) | 2:1, 2–3 dB GR **[starting value]** | 4:1, 4–6 dB GR **[starting value]** |
| Expander | unchanged | ratio ≤ 2:1 (keeps room tone steady; ACX asks consistent noise [2]) | unchanged |
| De-esser | when measured | when measured | when measured |

## 4. Measuring and deriving

- **f0 median/p10/p90**: run `dsp::estimate_f0` per 40 ms voiced frame of the read segment (today it
  returns one value). Pitch classes reuse today's HPF breakpoints (110/150/200 Hz), so no new threshold.
  Published adult averages for orientation: men ~100–125 Hz, women ~190–220 Hz [10].
- **Tilt**: least-squares slope (dB/octave) of `band_deviation_db` 250 Hz–8 kHz **after subtracting the
  mic correction curve**. Dark/Bright display cut-offs (±1.5 dB/oct) are placeholders to set from synthetic
  sweeps plus the corpus; they drive wording only.
- **Sibilance peak**: frames where 4–10 kHz energy exceeds 300–3000 Hz energy; peak of their averaged
  spectrum. Literature puts English /s/ peaks around 6–8 kHz [11]; clamp the de-esser centre to 4.5–9 kHz.
  Level: existing `sibilance_db`, re-measured on the EQ'd render (EQ before de-esser in the VST3).
- **Plosives**: existing detector, normalised per minute of the plosive-rich script. Physical fix first
  (pop filter, 20–45° off-axis, brief §4); only a `CantFixPlosives` constraint raises HPF/slope.
- **Proximity**: P(f, r) = 10·log10(1 + (b / kr)²), k = 2πf/c, b = `pattern_b` [12]. This idealised model
  overstates real mics (makers roll off the low end), so it is **not** used to compute a correction. It is
  used to (a) predict how much the low end swings across the coached distance range; if > 4 dB, low-end EQ
  moves are limited to ±2 dB; and (b) word the boom finding ("closer than your mic expects" vs "your voice/room").
- **Distance**: never inferred from level; we use the coached range for the profile (`distance_cm`) and
  its midpoint.

## 5. Composing the chain

### 5.1 Order (live order unchanged)
gain → VST3 [mono → HPF → EQ (≤ 5 bands) → de-esser] → OBS expander → compressor → limiter.

### 5.2 Steps
1. Gain staging: unchanged.
2. **HPF from the voice**: proposal `hpf ≤ f0_p10 / 1.5` for 12 dB/oct, which keeps the fundamental within
   0.8 dB (2nd-order Butterworth: |H|² = r⁴/(1+r⁴), r = 1.5 → −0.78 dB). Today's table can cut into the
   bottom of a deep voice (f0 p10 95 Hz → 63 Hz max vs today's 80 Hz). **Not switched in this piece**: P3
   compares both on synth voices and reports before/after numbers. Fault rules (rumble, desk, plosives) still raise it.
3. **Desired curve** D(f) per third-octave, 100 Hz–10 kHz: `D = M + P + R`
   - M (mic): −w × published deviation from 1 kHz, smoothed to 2/3 octave; only broad features
     (|dev| ≥ 2 dB over ≥ 2/3 octave); w = 0.5 if `verified: false`, 0.7 if `true`; |M| ≤ 3 dB.
     Half weight because published curves are "typical", unit spread and measurement distance are usually
     unstated. Switch position from the `controls` answer picks `switch_curves`; "I'm not sure" → default curve.
   - P (profile delta, section 3.3): off until the listening test; |P| ≤ 1 dB.
   - R (residual): render the test with M+P, re-analyse against `reference_for_f0`, then today's
     `derive_eq` region logic on the residual (outside ±3 dB, × 0.7, cuts preferred, boost caps 2–3 dB).
4. **Cap** (5.3), then **fit** D with ≤ 5 biquads (greedy: largest-area region first; peaks Q 0.7–1.4,
   shelves at the ends). Fit error is reported in Advanced; if the synth suite shows RMS fit error > 1 dB
   over 125 Hz–8 kHz, P4 raises `kMaxEqBands` to 8.
5. **De-esser**: centre = `sib_peak_hz`; on only if sibilance on the EQ'd render > −3 dB (today's threshold).
6. **Dynamics** per profile (3.3). 7. **Verify** (section 6).

### 5.3 The ±6 dB rule with mic correction (explicit)
- The cap applies to the **net** EQ curve: all bands summed, evaluated every 1/3 octave from 80 Hz to
  12 kHz. HPF and de-esser are outside it (a filter below the voice and a dynamic cut, as today).
- Net limits: **−6 dB / +3 dB** (boosts stay inside today's largest boost cap).
- Mic correction is **inside** the budget and capped at ±3 dB (half), so it can never use it all.
- When M + P + R exceeds the cap, **R is reduced first**, then P, never M below its own value: the
  remaining fault is physical, so its finding stays open with the physical fix ("beyond what EQ should do").
- Unit tests assert the cap on the evaluated biquad response, not on band gains.

## 6. Verification (re-measure against the target)

**Offline, before showing After** (`process::render_test` → `analyze`):
- V1 tone: every `ToneSummary` region within ±3 dB of the voice-adjusted target, or ≥ 50 % closer where capped.
- V2 do-no-harm: no region > 1 dB further from target than before; no new tonal finding;
  `sibilance_db` after ≤ before + 1 dB; plosive count not higher.
- V3 level: voice loudness within the profile target ±1 (existing `calibrate_makeup`); peaks ≤ ceiling.
- V4 cap: evaluated net curve within 5.3.
- Failing V2: drop the worst band, re-render (max 2 rounds); else fall back to today's chain.

**Live, after applying** (needs the VST3 to report, section 11): the plugin publishes its loaded-chain
hash and a 10 s long-term third-octave spectrum of its output; the app compares regions with the offline
prediction (±1.5 dB) and shows Met / Close / Missed. Without that, the app re-records and renders offline,
which checks the settings, not the live path, and says so in Advanced.

**Later sessions**: the startup drift check re-reads tone; > 3 dB change in boom/body suggests the mic
moved (proximity) and offers a re-tune, never silent re-EQ.

## 7. What the user sees

1. On the report, after the physical checks pass or are marked "I can't": card **Make it sound better**.
2. "Read these lines out loud" (≈ 20 s, script with p/b and s sounds). A test < 10 min old with the same
   mic and profile is reused.
3. Short animation with three plain facts: "Your voice: deep and warm", "Your mic: RE20, we know how it
   colours sound (from the maker's chart)", "Goal: Streaming".
4. **Before / After** toggle on their own recording, **loudness-matched** (equal LUFS, as listening
   tests require level alignment [6]) plus "On a phone speaker" (existing `phone_speaker`).
5. **Keep it** / **Gentler** (scales EQ 50 %) / **Undo** (settings vault, as A/B today).
6. Result: "Checked: it now sounds like we planned" or "Close: we kept it gentle because the room adds echo
   - something soft on the wall would help".
7. Advanced: measured vs target vs mic curve vs final EQ graph, band table, f0/tilt/sibilance numbers,
   mic-curve source link and verified flag, fit error, verify results.

Words to avoid: "studio quality", "perfect" (brief §8.7). Unverified mic data reads "from the maker's chart".

## 8. Evidence: every number and its status

| Number | Source | Status |
|---|---|---|
| Speech balance target (LTASS) | Byrne et al. 1994 [1] | in code; engineer review pending |
| f0 shifts in `reference_for_f0` (+3 dB / −6 dB) | none found | **unverified**, existing |
| Pitch breakpoints 110/150/200 Hz | brief §6 HPF ranges; adult averages [10] | existing starting values |
| HPF ≤ f0_p10/1.5 | filter math (5.2) | derived; needs listening check |
| Tolerance ±3 dB, 0.7 correction, boost caps | existing `derive_eq` | **unverified**, existing |
| Net cap −6/+3, mic ±3, weights 0.5/0.7 | this design; brief "±6 dB cap" | **proposal**, no external source |
| Tilt classes ±1.5 dB/oct | none | **placeholder**, wording only |
| /s/ peak 6–8 kHz, de-esser 4.5–9 kHz | Jongman 2000 via secondary [11] | **check original tables** |
| Proximity model | first-order gradient theory [12] | textbook; real mics deviate |
| Swing > 4 dB → LF ≤ ±2 dB | none | **placeholder** |
| Voiceover −20 dB RMS, ceiling −3.5 | ACX [2] | sourced (ACX is RMS, not LUFS) |
| Streaming −16 LUFS | Apple [3], AES TD1008 [4] (secondary summary) | sourced; TD1008 full text to read |
| No boosts > 7 kHz on calls | Opus WB 8 kHz [7], G.722 50–7000 Hz [8] | sourced; actual app codec mode unknown |
| Profile deltas +1 dB | SII band importance [9] (call only) | **unverified, off by default** |
| Voiceover/call compressor settings | none | **starting values** |
| Mic response points | maker charts [13] | sourced, `verified: false` until hardware |
| Level-matched A/B | ITU-R BS.1116 [6] | sourced |

## 9. Test plan (synthetic first, `audio::synth` + `process::render`)

New synth knobs (P1): `voice_tilt_db_oct`, `sib_peak_hz`, `f0_spread` (pitch movement), and
`mic_response: Option<Vec<(f64,f64)>>` applied as a smoothed filter ("this voice through that mic").

1. **Classification**: f0 90/130/170/230 Hz → Deep/LowMid/HighMid/High over 5 seeds; tilt ±3 dB/oct →
   Dark/Bright; sib peak 5/7/8.5 kHz recovered ±0.5 kHz; classes stable through each mic curve once M is applied.
2. **Do no harm**: default clean scenario through a flat mic → no band > 1 dB, de-esser off.
3. **Mic invariance**: one voice through 3 mic curves → after processing, the spread between the three
   rendered spectra (125 Hz–8 kHz) is smaller than before, by ≥ 30 % (number set in P3 from the run).
4. **Character kept**: a dark voice and a neutral voice through the same mic stay ordered (dark still
   darker after) and their tilt gap shrinks by no more than half.
5. **Cap property**: random D curves (fixed seeds) → evaluated net response within −6/+3; M within ±3.
6. **Sibilance**: presence boost never raises `sibilance_db` after the de-esser by > 1 dB.
7. **Profiles**: replace "chain identical" with "chains differ only in documented fields" (3.3), and
   Call has no band above 7 kHz with positive gain.
8. **Verify loop**: each existing fault scenario (boomy, muddy, harsh, dull) passes V1–V4 or reports why.
9. **Real recordings** (Otto's, test corpus): sanity only. No new findings, no band at the cap, residual
   shrinks. Never used to set a target or tune a number toward "sounds like Otto's mic".
10. **VST3 parity** (vst3-plugin): same `chain.json` through the plugin test harness vs Rust renderer,
   third-octave within 0.5 dB.
11. `cargo run --release --example calibrate voice_<scenario>` prints VoiceType, M, P, R, fit error, V1–V4.

## 10. Risks

- **Over-processing / stacking**: three sources of EQ adding up. Mitigation: one net budget, half-weight
  mic curves, tolerance band, do-no-harm loop, "Gentler", blind test before marketing.
- **Wrong or misread charts**: digitising error ~1 dB; charts are typical units at unstated distances.
  Mitigation: broad features only, half weight, `verified` flag, hardware check.
- **Phase**: IIR EQ is minimum-phase; broad moves (Q ≤ 1.4) keep phase shift gentle and the signal is mono,
  so there is no inter-channel smearing. Risk is with direct monitoring mixing dry + processed in the
  headphones only (not the stream); note it in monitoring advice. A steep 24 dB/oct HPF adds low-end
  group delay; keep 12 dB/oct unless a fault rule asks.
- **Sibilance**: presence/air boosts raise "s"; EQ precedes the de-esser and sibilance is re-measured after EQ.
- **Proximity changes with distance**: a static EQ is right at one distance only. Tune for the middle of the
  coached range, limit low-end moves when the predicted swing is large, `MovesALot` → no low boost.
- **Room and reflections**: comb filtering (`mic.reflection`) must not be EQ'd; those bands are excluded from R.
- **Codecs and call apps**: Discord/Teams AGC and noise suppression act after us; the call profile keeps the
  existing advice to turn them off and does not spend EQ above 7 kHz.
- **Short or bad test**: Low confidence → mic correction and gain only, no voice-based moves.

## 11. What other agents need to do

- **device-profiles**: `response` schema + loader in `core/src/profiles/`, digitise published charts for
  owned/top mics first (RE20, NT1-A, SM7B, Yeti, QuadCast, MV7, PodMic), source link with page/figure,
  `verified: false`; remove `eq_prior` once nothing reads it; `null` where no chart exists.
- **vst3-plugin**: `chain.json` gains `version` and per-band origin (display only, DSP unchanged); possibly
  `kMaxEqBands` 5 → 8 (only if P3's fit error says so); de-esser centre from config (check it is not fixed);
  output long-term third-octave spectrum + chain hash to the app (IPC/shared file) for live verification;
  parity test against the Rust renderer.
- **app-backend**: commands `start_voice_tune` / `apply_voice_tune` / `undo_voice_tune`, loudness-matched
  before/after buffers, "gentler" variant, vault entry, live verify read-out; `app/API.md` types
  `VoiceType`, `VoiceTuneResult` (coordinator approves).
- **app-ui**: the card, reading script, Before/After toggle, Keep/Gentler/Undo, result line, Advanced graph.

## 12. Build pieces (~40 turns each)

- **P1 core-engine: voice measurement.** `VoiceType`, per-frame f0 stats, tilt, sibilant peak, plosive rate,
  synth knobs, tests 1 and 2. No chain change.
- **P2 device-profiles (+ core schema): mic curves.** `response` schema, loader, 7 mics digitised with
  sources, proximity function + tests, `eq_prior` retired.
- **P3 core-engine: composer + verify.** M/P/R, net cap, 5-band fit, profile tuning, de-esser centre, V1–V4,
  tests 3–9, profile-chain test replaced, before/after numbers for any changed behaviour (HPF rule).
- **P4 vst3-plugin + app-backend: apply and live verify.** Plugin spectrum report, chain.json version,
  parity test, backend commands + API.md.
- **P5 app-ui: Make it sound better screen**, mock mode first.
- Then (not a build piece): blind listening test before P deltas go on or any "sounds better" claim.

## 13. Open questions for Otto

1. Profile tone deltas (a touch more clarity for streaming): ship **off** until a blind listening test, or
   on as marked starting values?
2. Voiceover goal: audiobook rules (ACX, quieter, peaks ≤ −3) or podcast/YouTube (−16 LUFS)? Or ask the user?
3. Live verification needs extra VST3 work (plugin reports its output). Worth it, or offline-only for v1?
4. Blind listening test: 5–10 listeners, your recordings + synthetic voices, before any marketing claim. OK?
5. Verifying mic charts needs a measurement mic (~€100, e.g. miniDSP UMIK-1). Buy one, or keep all mic
   curves `verified: false` at half weight?

## Decisions (2026-11-07)

Otto's answers to the open questions above:

1. **Profile tone deltas ship off** until a listening test shows they help. `SetupProfile::tuning()` carries
   the delta fields at 0.
2. **Voiceover loudness is the user's choice**, with three targets: **YouTube video** (default): -14 LUFS
   integrated, true peak <= -1 dBTP. **Podcast**: -16 LUFS. **Audiobook (ACX)**: -20 dB RMS (inside
   -23...-18), peaks <= -3 dBFS. This replaces the single -20 dB RMS / -3.5 ceiling in the Recording column
   of 3.3; P3 builds the choice (and the limiter ceiling per target).
3. **No live VST3 verification in v1.** Verification is offline only (re-render and re-analyse, section 6)
   and says so in Advanced. The plugin spectrum report and chain hash in section 11 are dropped from P4.
4. **No blind listener panel for now.** Instead: level-matched Before/After, Keep / Gentler / Undo, no
   "sounds better" claim anywhere; the wording is "tuned toward published targets".
5. **No measurement-mic purchase now.** All mic curves stay `verified: false` at half weight (5.2).

## Build notes

- P1 (voice measurement) is in `core/src/audio/voice.rs`. The module is `voice.rs` (3.1), `VoiceType` is on
  `AudioMetrics::voice`. Class members gained `Unknown` (not in 3.1) so an insufficient result has no guess.
- Confidence is on **voiced seconds** (frames with a clear pitch), not total speech: `Insufficient` below
  1.5 s, `Low` below 2.5 s (not the 8 s of 3.1: a 9-10 s read holds ~4 s of voiced frames), and `Low` for
  noise (SNR < 15 dB, or a tilt band < 10 dB over the silence step), band limit (< 7 kHz) or clipping.
  Thresholds are calibrated on synthetic voices only.
- Tilt takes an optional mic correction curve (Hz, dB re 1 kHz, already weighted by the caller); none is
  passed yet.
- Display classes (tilt +-1.5 dB/oct, "s" > -3 strong / < -20 soft, plosives >= 6 per min "many") are
  placeholders for wording; nothing is tuned from them.
- P3 (composer + verify) is `core/src/audio/tune.rs` (`tune::compose` -> `VoiceTune`); profile tuning is
  `SetupProfile::tuning_for(VoiceoverTarget)`; `MicContext::voiceover_target` carries the voiceover choice.
  `cargo run --release --example voice_tune [scenario]` prints M/P/R, fit error, HPF comparison, V1-V4.
- The scan chain now follows the profile in the documented dynamics fields only (3.3); Streaming is
  unchanged. The make-up loop keeps normal speech peaks (95th-percentile 20 ms frame peak) at
  `ProfileTuning::speech_peak_limit_db`: -3 dBFS for Streaming and Call (unchanged), the limiter ceiling for
  Voiceover. With the old -3 dBFS rule a 2:1 voiceover chain stopped at about -20.6 LUFS, so the YouTube
  -14 LUFS default was never reached; with the ceiling bounding peaks it lands at -14.2...-14.4 LUFS at
  2:1 (synthetic average and dark voices), sample peak -1.5 dBFS (ceiling). ACX stays at -20 dB RMS,
  peaks -3.5 dBFS (<= -3). Tests: `voiceover_youtube_reaches_minus_14`, `voiceover_acx_meets_rms_and_peaks`.
  Cost: up to about 5 % of 20 ms speech frames may touch the limiter on a YouTube voiceover (none of this changes
  Streaming or Call).
- A de-esser-only candidate counts as "improved" only when the "s" drops by more than 0.5 dB
  (`DEESS_MIN_GAIN_DB`): its high cut alone lowers the tone RMS even when the "s" rises (synthetic
  sibilant voice, de-esser at 7 kHz / -8 dB: "s" 1.4 -> 2.7 dB with the tone RMS down).
- The summed mic-band response is held to +-3 dB (`limit_mic_sum`, one scale factor on all mic bands),
  not only each band.
- Deviations from section 6, with reasons: "before" is the same chain **without** the tune (same HPF and
  dynamics), so only the tune is judged. V2's "s" rule counts as harm only when the "s" also reaches the
  strong range (> -3 dB): a soft "s" rising from -28 to -24 dB is not harm. V3 also passes within 1 dB of
  the untuned chain, because the make-up loop itself accepts 1.5 dB under target. V1 failing alone gives
  "Close" (applied, kept gentle), not a rejection. The do-no-harm loop drops a harmful de-esser first.
- R is skipped when the `mic.reflection` finding fires (comb filtering is not EQ'd), not only on low confidence.
- Mic features are judged on the raw chart (>= 2 dB over >= 3 third-octaves), then smoothed, so smoothing
  can't widen a one-band wiggle into a correction.
- Fit error on the synthetic presence-lift mic: 0.43 dB RMS (< 1 dB), so `kMaxEqBands` can stay 5.
- HPF `f0_p10 / 1.5` (reported, not switched): f0 120 Hz voice, p10 106 Hz: today 80 Hz loses 1.2 dB at p10,
  proposal 71 Hz loses 0.8 dB. Dark voice with today's boom raise (95 Hz): 4.3 dB vs 1.6 dB.
- Found, not fixed (follow-ups): the offline de-esser alone *raises* the measured "s" (+1.3 dB) and costs
  ~3.5 dB of loudness on the sibilant synth voice; verification rejects it, but the renderer/VST3 de-esser
  needs a look. A +3 dB/oct bright synth voice trips `mic.reflection`, so bright voices currently fall
  back to mic-only correction.

## Sources

1. Byrne D. et al. (1994), "An international comparison of long-term average speech spectra", JASA 96(4):2108. https://doi.org/10.1121/1.410152
2. ACX audio submission requirements (RMS −23…−18 dB, peaks < −3 dB, noise floor ≤ −60 dB RMS, consistent levels/noise). https://help.acx.com/s/article/acx-audio-submission-requirements
3. Apple Podcasts audio requirements (≈ −16 LKFS ±1, true peak ≤ −1 dBFS). https://podcasters.apple.com/support/893-audio-requirements
4. AES TD1008.1.21-9 (2021) loudness for internet audio streaming; speech −18 LUFS, +2 LU upper tolerance, as summarised by Telos Alliance (full text to be read). https://docs.telosalliance.com/docs/understanding-loudness-for-streaming-audio
5. EBU R 128 and ITU-R BS.1770 (loudness measurement used by the LUFS meter). https://tech.ebu.ch/publications/r128 · https://www.itu.int/rec/R-REC-BS.1770
6. ITU-R BS.1116, methods for subjective assessment of small impairments (level alignment in listening tests). https://www.itu.int/rec/R-REC-BS.1116
7. RFC 6716, Opus codec (wideband = 8 kHz audio bandwidth). https://www.rfc-editor.org/rfc/rfc6716
8. ITU-T G.722, 7 kHz wideband audio coding (50–7000 Hz). https://www.itu.int/rec/T-REC-G.722
9. ANSI/ASA S3.5-1997 Speech Intelligibility Index (band-importance weights). http://www.sii.to/
10. Titze I.R. (1989), "Physiologic and acoustic differences between male and female voices", JASA 85(4):1699, https://doi.org/10.1121/1.397959; averages as summarised by NCVS: https://ncvs.org/tutorials/factors-influencing-fundamental-frequency/
11. Jongman A., Wayland R., Wong S. (2000), "Acoustic characteristics of English fricatives", JASA 108(3):1252, https://doi.org/10.1121/1.1288413 (6–8 kHz /s/ peak via a citing paper; check tables).
12. Proximity effect, first-order gradient model: near-field compensation paper https://zenodo.org/records/1282755 ; Schoeps https://schoeps.de/en/knowledge/knowledge-base/technical-basics/proximity-effect.html ; Sound On Sound https://www.soundonsound.com/techniques/proximity-effect
13. Maker charts to digitise: Shure SM7B guide https://pubs.shure.com/guide/SM7B/en-US ; Electro-Voice RE20 (Variable-D, bass tilt) https://products.electrovoice.com/na/en/re20 ; RØDE NT1-A https://rode.com/en/microphones/studio-condenser/nt1-a
14. Internal: `docs/research-brief.md` §4 tonal map and §6 chain targets (starting values, engineer review pending).
