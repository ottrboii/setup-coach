# Streamer Setup Coach — Research Brief (v1 input for build)

Date: 2026-11-02 · Status: research complete, ready for v1 scoping
Confidence tags: [H] high · [M] medium · [L] low · [PK] prior knowledge, verify on hardware

---

## 0. Product decisions (locked)

- **Audience:** non-technical streamers who bought popular gear (SM7B "because everyone does", Yeti, QuadCast, C920) and left it on defaults. They will pay; they don't want to learn audio.
- **Philosophy:** fix the physical setup first, then apply plain static settings (gain, HPF, static EQ, expander, compressor, de-esser, limiter; locked exposure/WB/focus/anti-flicker). **No AI processing** — and actively detect/disable AI effects (Windows Voice Clarity, RNNoise/NVIDIA/Krisp, Studio Effects).
- **Flow:** plug in → identify → pre-flight checks → guided test → analyse → plain-language score + ranked fixes → user marks Done / Can't → recalibrate for constraints → apply → re-measure/verify → before/after → save → re-apply on startup + drift check.
- **Platform:** Windows only.
- **Repo:** new standalone repo (not Channel Hub).
- **Pricing:** up-front purchase + affiliate (or affiliate-only if going for volume). Free scan is the hook; affiliate links appear when a physical limit is measured (booster, interface, light, pop filter, boom arm).

---

## 1. Architecture: standalone app, NOT an OBS plugin

### Why not a native OBS plugin
- libobs is GPLv2, no linking exception; OBS project position: distributed plugins must be GPL-compatible → closed paid plugin = real infringement risk [H on position].
- Only OBS Studio loads OBS plugins. Streamlabs Desktop ships curated modules only; TikTok LIVE Studio has no plugin support [M].
- Must track OBS ABI/plugin-location changes (OBS 33 changed them).
- What a plugin would "gain" (zero-latency processing inside OBS, a dock UI) is achievable without it — see v1.

### v1 (OBS-first, est. 3–4 months)
- **Standalone signed app** (own UI, live coaching, analysis).
- **Raw mic analysis:** WASAPI shared-mode capture alongside OBS (multi-client is fine) [H]; optionally `AUDCLNT_STREAMOPTIONS_RAW` to see the pre-APO signal and compare with what OBS hears [M].
- **Configure OBS via obs-websocket v5** (built in since OBS 28; user must enable + password; app can set it up). Can create/order/enable filters, set input settings/volume/sync offset/tracks, read live meters (`InputVolumeMeters`), take source screenshots (`GetSourceScreenshot`) [H]. Talking over the protocol = no GPL obligation [H].
  - **Gap:** no request for the per-source "Downmix to Mono" flag [H] → do mono in our VST3, or edit scene-collection JSON while OBS is closed [M].
- **OBS built-in audio filters (OBS 33):** gain, noise gate, noise suppression (RNNoise/Speex/NVIDIA), expander, upward compressor, compressor (sidechain), limiter, 3-band EQ, invert polarity, VST 2.x host. **No HPF, no parametric EQ, no de-esser** [H].
- **Our own closed-source VST3** for HPF + parametric EQ + de-esser + mono downmix (+ IPC to the app for meters/control).
  - VST2 is NOT an option: Steinberg stopped new VST2 licences in 2018 [H].
  - VST3 SDK is MIT → closed plugin OK [M–H]; VST3 doesn't link libobs → no GPL issue [H].
  - **Native VST3 host in OBS:** PR #12752 approved by two maintainers 27 Oct 2026, tracked for ~33.1 [M]. Interim: atkAudio (free, AGPL, user-installed host; our plugin unaffected) [M]. Meld Studio hosts VST3 natively [M].
- **Streamlabs Desktop:** guided setup with its built-in filters (its VST host reportedly VST2-only [M]).
- **Discord / TikTok LIVE Studio:** defer to v2 (or VB-CABLE stopgap under VB-Audio's paid licence — poor UX).
- **Camera:** direct UVC control (DirectShow `IAMCameraControl` / `IAMVideoProcAmp`; anti-flicker via `IKsControl` `KSPROPERTY_VIDEOPROCAMP_POWERLINE_FREQUENCY`) — works while OBS streams the camera [H]. Frames for analysis via obs-websocket `GetSourceScreenshot` (1–5 fps) [H].

### v2 (universal)
- **Branded virtual microphone** (how NVIDIA Broadcast / Sonar / Wave Link / Voicemod do it): kernel virtual audio driver + user-mode engine.
  - **Buy:** Thesycon TVirtAudio — commercial, attestation-signed, fully rebranded, C++ API, 60-day eval, quote pricing [H]. **Build:** from Microsoft SysVAD (MS-PL) [H].
  - Signing: EV code-signing cert (~$300–700/yr [M]) + Partner Center attestation (hours–days, no HLK) [M–H]; cross-signed drivers no longer trusted on Win11 24H2+ since Apr 2026 [M–H]; must be HVCI-compatible [M].
  - Anti-cheat: Vanguard once flagged Voicemod's driver [H] → minimal IOCTL surface, pre-clear with Riot/EAC/BattlEye/FACEIT [M].
  - Latency ≈ 20–40 ms vs ~0 for the OBS/VST route [M]; OBS users can stay on v1 route.
- No user-mode-only way to create a selectable mic endpoint [M–H]. APO route is fragile — skip [M].

### Signing (all versions)
- Sign every exe/dll/installer (RSA) for Smart App Control; Microsoft Trusted Signing recommended [H].

---

## 2. Camera platform facts

- **Multi-App Camera mode** (Win 11, rolled out to all June 2026): lets two apps open one camera, **but while on, image settings may only be changed from Windows Settings** [M] → detect and warn.
- **Windows Camera Settings "default values"** apply on every stream start for Brightness, Contrast, Sharpness, Saturation, HDR, Studio Effects (not exposure/WB/focus/anti-flicker) [H]. `IMFCameraConfigurationManager` can write them, documented for Store "camera companion apps" [H docs / L for 3rd-party use] → prototype, don't depend on.
- **Persistence:** UVC values are volatile; lost on replug/power cycle/sometimes sleep; Windows defaults re-applied at stream start [M]. → Re-apply on device arrival (`CM_Register_Notification` / `DeviceWatcher`), on resume, and **after the streaming app starts the stream**; monitor with `IMFCameraControlMonitor` and re-assert on drift [M–H].
- On-device save exists for: Elgato Facecam family (via Camera Hub "save to device"), Razer Kiyo Pro (NVRAM save), Insta360 Link presets [M].
- Linux UVC quirk list confirms C920 ignores controls set before streaming starts (`RESTORE_CTRLS_ON_INIT`) [H] → apply after stream start.
- Windows Studio Effects now work on external USB cams on Copilot+ PCs (late 2025) → detect, warn, disable [M].

---

## 3. Launch gear list (target ≥90% coverage per category)

No public model-level market share exists; shares below are estimates from bestseller snapshots (Amazon US/DE, Best Buy, Newegg, Thomann), StreamingEquip (168 streamers' mics, 98 headsets), Podcast Host 2024 survey (n=511), BCN Japan, vendor claims. **Top streamers ≠ beginners** (SM7B is 52% of famous streamers but a minority of beginners).

### USB mics (~35 SKUs → ~85–90%)
| Family | Est. share | SKUs |
|---|---|---|
| Yeti (Logitech/Blue) | 20–25% | Yeti, Nano, X, GX, Orb, Snowball iCE |
| HyperX | 15–20% | QuadCast, QuadCast S, QuadCast 2, 2 S, SoloCast, SoloCast 2 |
| Fifine | 10–15% | K669B, K678, AM8, A6/A6T, K688 |
| Elgato Wave | 5–8% | Wave:1, Wave:3 / MK.2, Wave Neo |
| Razer Seiren | 4–6% | Mini, V3 Mini, V2 X |
| Rode | 4–6% | NT-USB/NT-USB+, NT-USB Mini, PodMic USB |
| Shure | 3–5% | MV7, MV7+, MV6 |
| Hybrid dynamics | 4–6% | Samson Q2U, ATR2100x, Maono PD200X |
| Other | — | AT2020USB+/X, Tonor TC-777 |

### XLR mics (~15 → ~85–90%)
SM7B (35–50% of XLR users), SM7dB, Rode PodMic (10–15%), Procaster, RE20, RE320, SM58, AT2020, Rode NT1 family, Elgato Wave DX, Shure MV7X, + hybrids in XLR mode.

### Interfaces / mixers (~20 → ~85–90%)
Scarlett Solo/2i2 3rd+4th gen (30–40%; 2nd gen grouped), Elgato Wave XLR MK.1/MK.2/Pro + XLR Dock (10–15%), GoXLR / Mini (10–15%, discontinued), Behringer UMC22/202HD/204HD (10–15%), RODECaster Duo/Pro II + Streamer X, Yamaha AG03/AG06, MOTU M2, SSL2, UA Volt 2.
- **Inline boosters** (Cloudlifter, FetHead, DM1): model as a chain modifier (+~25 dB, needs 48 V passthrough), not a device.

### Headsets (profile by connection path first → >95% usable)
Paths: 3.5 mm analog via Realtek (Mic Boost, noisy jacks) · USB with DAC dongle · 2.4 GHz dongle · **Bluetooth Hands-Free (#1 thing to detect/warn)**. Models for naming: HyperX Cloud II/III/Alpha, Logitech G Pro X/X 2, G432/G435/G733, Razer BlackShark V2/V2 X/V3, Kraken, SteelSeries Arctis Nova, Corsair HS/Virtuoso, Turtle Beach, Astro. Estimated 30–50% of beginners use a headset mic [L].

### Webcams (12 families / ~30 SKUs → ~85–90% of USB webcam users)
C920 family (C920/s/x/e) · C922/C925e/C930e · C270/C310 · Brio 4K/Stream Ed. · MX Brio · Brio 500/505 · Brio 100/101/300 · StreamCam · Elgato Facecam/MK.2/Pro/Neo/4K · Razer Kiyo/X/Pro/Pro Ultra · OBSBOT Tiny 2/2 Lite/3, Meet 2/SE · Insta360 Link/2/2C. Stretch: NexiGo N60/N930AF, EMEET C960/S600/Nova 4K, Anker C200/C300.
- Logitech C920/C922 ≈ 30–35%; Brio line 12–15%; Facecam 8–10% [M].
- **Coach-only class:** capture-card cameras (10–15% of camera streamers; Cam Link 4K, HD60, MS2109/MS2130 dongles) — UVC controls only touch the capture chip. Body picker: Sony ZV-E10/II, a6100/a6400, ZV-1; Canon M50/R50/R10; Panasonic G7/G85/GH5; Fuji X-S10/X-T. Also phones (Camo, DroidCam, Iriun, Windows Virtual Camera), virtual cams.
- **Generic UVC profile** for everything else incl. laptop cams.
- Identify by **USB VID:PID**, name as fallback.

### Generic fallback profiles (audio)
USB condenser side-address · USB condenser top/end-address · USB/XLR hybrid dynamic end-address · XLR dynamic low-output (~60 dB) · XLR condenser (48 V) · headset analog · headset USB/2.4 GHz · headset Bluetooth · webcam/laptop built-in mic. Match: VID/PID → endpoint name → 1–2 photo questions.

### Minimum per-profile fields
Mic: capsule type, address (side/end/top/multi + default-pattern warning), pattern switch, phantom need, gain needed (dB @ ~15 cm), Windows slider = hardware gain?, hardware controls, onboard DSP + can disable (Auto Level, Clipguard, Auto Gain/Clip Safe, APHEX, Blue VO!CE), vendor app, sample rate, mono vs "stereo with one live channel", VID/PID + endpoint name, distance/angle range, EQ prior.
Camera: UVC controls + ranges, vendor-XU-only controls, persistence behaviour, known quirks (AE-priority fps drop, MJPEG vs YUY2 limits, fixed focus range, HDR fps halving, oversharpened default), USB 2/3 needs, VID/PID + name.

### Key device gotchas (verify [PK] on hardware)
- Yeti, QuadCast, AT2020, NT-USB+: **side-address** — people talk into the top.
- SM7B needs ≥60 dB clean gain (Shure) [H]; Scarlett Solo 4th gen 57 dB, older ~50 dB → hiss. Wave XLR (75–80 dB) drives it without booster.
- Scarlett/Behringer input 1 → **left channel only** in OBS.
- Scarlett 2i2 4th gen has Auto Gain + Clip Safe (Solo doesn't).
- Shure MV7/MV7+ Auto Level fights downstream compression; MOTIV app USB-only.
- GoXLR: OBS must use "Broadcast Stream Mix"; product discontinued; open-source GoXLR Utility has API.
- RODECaster multitrack mode breaks some apps; ASIO driver.
- Wave Link 3: OBS should take the Wave Link mix, not raw mic; free, works with any mic, has Auto Gain Wizard + Gain Lock + Clipguard (**competitor for the gain part**).
- C920: ignores controls set before stream start; YUY2 1080p ≈ 5 fps → use MJPEG; low-light compensation drops fps; coarse whole-stop exposure steps.
- StreamCam: OBS exposure steps too coarse (−6 too dark / −5 too bright).
- Facecam: settings persist only after "Save to device"; original Facecam uncompressed → USB 3.
- Kiyo Pro: HDR halves fps.
- OBSBOT: on-device AI tracking reframes — disable.

---

## 4. Mic problems catalogue (ranked)

Detect: S signal · W Windows API/registry · O OBS config · A ask. Apply: Auto / Guided / Manual.

### Top 15
1. **Voice buried under game** (#1 viewer complaint) — O+S meters during gameplay → game 10–15 dB under voice, mic compressor, optional sidechain ducking — Auto
2. **Too quiet / too far** — S peaks < −20 dBFS → closer, gain to −12…−6 dBFS peaks — Guided/Auto
3. **Gate chopping words** — O vs measured floor → close ~6 dB above floor, attack ≤5 ms, hold 150–250 ms, or expander 2–4:1 — Auto
4. **Fan/AC/background noise** — S silence spectrum → move/aim null, closer + less gain, HPF + expander — Manual+Auto
5. **Room echo** — S decay tail → closer, soft furnishings, dynamic mic; settings can't remove it — Manual
6. **Clipping on laughs** — S peaks ≥ −0.3 dBFS → laugh peaks ≈ −6 dBFS, limiter — Guided/Auto
7. **Keyboard/mouse clicks** — S typing step → closer, null at keyboard, boom arm, expander — Manual+Auto
8. **Left ear only** — S R≪L / O mono flag → downmix to mono — Auto
9. **Robotic/underwater (AI suppression)** — O `noise_suppress_filter` method → remove; HPF + expander — Auto
10. **Discord AGC/Krisp/echo-cancel** — A → off with headphones + good mic — Guided
11. **Wrong mic selected** (OBS "Default", webcam/laptop mic) — W/O/S fingerprint → pin device — Auto/Guided
12. **Own voice / Discord echoing on stream** — O duplicate captures, S cross-correlation → single capture path, monitoring off — Auto
13. **Plosives** — S <150 Hz bursts → off-axis 20–45°, pop filter, HPF 80–100 Hz — Manual+Auto
14. **Doubled monitoring / routing** (direct monitor + OBS/Wave Link) — O monitoring_type → Monitor Off, one mix — Auto+Guided
15. **Wrong side of mic** — W/A model + S HF loss → address logo side, cardioid — Manual

### Other high-value checks
- Gain stacking (Windows Mic Boost + OBS gain + Discord) — W `IDeviceTopology`, O → boost 0, endpoint 100%, one digital gain stage — Auto
- SM7B on weak interface — W VID/PID + A → 2–5 cm, booster or better interface — Manual (affiliate moment)
- App changing mic level (Discord AGC) — W `IAudioEndpointVolume` callback — detect
- Windows enhancements / **Voice Clarity** (AI, default since 24H2 where supported) — W `PKEY_AudioEndpoint_Disable_SysFx`, `IAudioEffectsManager` — Auto/Guided
- Sample-rate mismatch 44.1/48 kHz (crackle, robotic, drift) — W mix format, O `basic.ini` — Guided/Auto
- "Reduce other sounds by 80%" ducking — W `UserDuckingPreference` → 3 — Auto
- Exclusive mode — W → off — Auto (admin)
- Mic privacy off — W ConsentStore → deep link `ms-settings:privacy-microphone` — Guided
- USB selective suspend dropouts — W powercfg — Auto
- Windows update reset devices — diff vs snapshot — Auto
- "Listen to this device" on — W — Auto
- Discord input sensitivity auto, attenuation — A — Guided (suggest threshold from measured floor)
- Phantom confusion — S silence + A — Manual
- Mic onboard DSP (MV7 Auto Level, Blue VO!CE, APHEX, Auto Gain) — W model + S flat level — Guided (Clip Safe / Clipguard = peak protection, OK to keep)
- Desk thumps (S tap test), comb filtering (S notches), ground-loop hum (S 50/60 Hz + harmonics), USB/GPU whine (S load-tracking tone), cable crackle (S wiggle test), headset boom position, **Bluetooth Hands-Free** (W endpoint name / 8–16 kHz), moving off-axis (S variance > 6 dB)
- Processing misuse: compressor pumping, big EQ boosts (> ±6 dB), wrong filter order, limiter misuse (OBS limiter has no lookahead → headroom), double processing (Wave Link FX / GoXLR / G Hub / NVIDIA + OBS)
- Mix: alerts 3–6 dB under voice peaks; music 20–30 dB under voice; A/V drift (48 kHz everywhere, sync offset)

### Tonal map (static EQ after physical fixes; cap ±6 dB, prefer cuts)
Muddy/boomy: back off, HPF 80–100, cut 200–400 Hz 2–4 dB · Thin: fix distance/side/BT first, small 120–200 Hz shelf · Nasal/honky: narrow 2–3 dB cut 800 Hz–1.5 kHz · Harsh: off-axis, cut 3–6 kHz, de-ess; check Focusrite Air · Muffled: fix side/Voice Clarity/BT first, +2–3 dB 8–10 kHz · Boxy/hollow: cut 300–600 Hz; room/reflection fixes matter more.

### Test script additions
5 s silence · read with plosives/sibilants · loud laugh · **5 s typing** · **desk tap** · **cable wiggle** · **lean back / turn head**.

### Quick-win checks on every launch
Device pinned + snapshot diff · privacy · enhancements/Voice Clarity off · 48 kHz everywhere · Mic Boost 0 / endpoint 100% / no level hijack · Listen-to-device off, OBS monitoring sane · ducking "Do nothing" · USB suspend off · no Hands-Free endpoint · OBS filter audit (no AI suppressors, order, sane values, mono on input 1) · no double captures · vendor DSP reminder · 5 s idle listen vs baseline (new fan/hum/whine).

---

## 5. Camera problems catalogue (ranked)

Detect: F frame analysis · U UVC state · O OBS config · W Windows · A ask. Apply: S software / I instruct.

### Top 15
1. **Backlit / dark face** (window behind; worst for darker skin) — F face vs background > 1 stop → turn/close blinds/key light; manual exposure metered on face/paper; backlight comp off — I+S
2. **Low light: grain, fps drops to 15–24, smear** — F real frame interval, noise; U gain/AE priority → light first; disable Low Light Compensation (`KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY`), shutter ≥ 1/fps, low gain — I+S
3. **Exposure pumping** — F face luma variance in head-turn/lean steps; U AE auto → stronger key than monitor; lock exposure+gain; RightLight/HDR off — S
4. **Colour drift / wrong skin** (auto WB, mixed 2700/5600/6500 K) — F paper R/G B/G, hue drift → one colour temp; manual WB Kelvin from paper — S+I
5. **Settings reset after reboot/replug** — U vs saved profile → re-apply on arrival + after stream start; one controlling app; Facecam save-to-device — S+I
6. **Camera too low (up-nose)** — F face pitch, eye line → eye level, riser 10–15 cm — I
7. **Wrong resolution/fps/format in OBS** (Device Default, YUY2 on USB 2) — O scene JSON, F delivered res/fps → Custom 1080p30 MJPEG (or 720p60), direct USB 3 — S (OBS closed)/I
8. **Flicker banding** — F row-mean sinusoid + luma oscillation; U power-line vs region → 50/60 Hz; shutter 1/50, 1/100 (50 Hz) or 1/60, 1/120 (60 Hz) — S
9. **Focus hunting / background focus** — F Laplacian over time; U AF auto → light, AF off, sweep to peak face sharpness — S
10. **"Camera in use" / missing** — W privacy, Frame Server, open errors → close other apps, enable access, restart Frame Server — S detect / I
11. **RGB/monitor tinting the face** — F face chroma vs paper → RGB on wall only, white key stronger than monitor, dim screen — I
12. **Harsh shadows / raccoon eyes** (single/overhead light) — F eye socket vs cheek, L/R > 3:1 → large soft key in front, slightly above, down ~30°, diffuse, bounce fill — I
13. **Ring light in glasses/eyes, flat** — F eye-region ring/blob; A glasses → raise + tilt down, 30–45° off-axis, bigger source — I
14. **Washed-out / crushed (colour range mismatch, 601/709)** — O range/space, F black floor ~16 vs 0 → match range, keep Partial/709 — S
15. **USB bandwidth freezes / 2nd cam won't start** — W USB topology, F frame gaps → one cam per controller, no unpowered hubs, MJPEG, suspend off — I+S

### Others
Forehead clipping · crushed/flat contrast · HDR halos/fps drop · motion blur · oversaturated skin · PWM/dimmer flicker · fixed-focus too close · oversharpening halos · buffering setting · Studio Effects/vendor AI on · wall hotspot / background brighter than face · wide-lens distortion too close · framing/headroom/eye line · cluttered background · capture cams: overheating/auto power-off (dummy battery, Auto Power Off Temp High), HDMI overlays (Info Display off), PAL/NTSC fps caps, **lip-sync offset ~80–320 ms** (clap test → OBS sync offset, S), range mismatch.

### Targets for the coach
- **Meter on a white paper the user holds** (paper ≈ 85–92% / 8-bit 215–235, unclipped) and set WB from it → face lands at its natural brightness for any skin tone. **Never push darker skin to a lighter-skin target**; add light instead. Face highlights: no large area ≥ 250; flag crushed if face median < ~25 IRE. (Typical: lighter skin ~55–70 IRE, darker ~35–50 IRE.)
- Background 0.5–1.5 stops below face; warn if brighter.
- Key:fill 2:1 natural, up to 3:1; > 4:1 harsh (left/right cheek ratio in linear light).
- WB: lock from paper; neutral within ±3–5%; all face lights same colour family.
- Shutter vs mains: 50 Hz → 1/50, 1/100; 60 Hz → 1/60, 1/120; ≥ 1/fps; gain/ISO low (< ~1000). Note drivers may step in powers of two → always re-check flicker after apply.
- Delivered fps = requested ±1%.
- Geometry: lens at eye level to ~10° above; 60–80 cm; eyes ~⅓ from top; headroom 5–10%; key just above eye line, down ~30°, front or 30–45° side, 0.5–1 m, diffused.
- Sharpness/saturation: start at default, only go down.

### Quick-win checks on every launch (~2 s)
Camera opens, privacy/Frame Server OK · UVC vs profile → re-apply, log who overwrote · delivered res/fps · OBS source res/fps/format/range/buffering · 1 s flicker check + power-line vs region · face vs background, clipping, noise (re-coach if room changed) · colour drift vs paper reference · focus vs stored + sharpness baseline · Studio Effects/vendor AI off.

---

## 6. Audio targets (starting values; validate with engineer + listening tests)

Raw: speech peaks −18 to −12 dBFS · loudest ≤ −6 dBFS · noise floor ≤ −60 dBFS (≤ −50 OK, > −45 problem) · speech-to-noise ≥ 50 dB (≥ 40 OK).
Chain: Gain → HPF (60–80 deep / 80–100 mid / 100–120 high voice, 12–18 dB/oct) → EQ (≤ 5 bands, ±6 dB cap) → Expander (threshold midway floor↔quiet speech, 2–4:1, attack 1–5 ms, release 100–250 ms) → Compressor (3–4:1, 3–6 dB GR, attack ~5–10 ms, release 60–150 ms) → De-esser (5–9 kHz, 2–4 dB, only if detected) → Limiter (−1.5 dBFS; OBS limiter has no lookahead).
Output: voice ≈ −16 LUFS short-term (−18…−14), peaks −6…−3 dBFS. Mix: game 10–15 dB under voice, music 18–30 dB under, alerts at/under voice peaks (3–6 dB), guests ±2 dB, stream ≈ −16…−14 LUFS integrated.
Mic distance priors: dynamic end-address 3–10 cm, 15–30° off-axis · USB condenser side-address 10–20 cm + pop filter · large XLR condenser 15–25 cm · headset boom corner of mouth 1–3 cm · off-camera boom 30–60 cm (worst case).

---

## 7. Competitive landscape (what changes our angle)

- **Elgato Wave Link 3:** free, any mic, Auto Gain Wizard, Gain Lock, Clipguard, VST inserts → owns "set my gain". We win on: diagnosis + coaching, tone/EQ, system/config audit, camera, vendor-neutral, verify loop.
- **Focusrite Scarlett 4th gen:** Auto Gain + Clip Safe on 2i2 and up.
- **Shure MV7+/MV6 Auto Level, Logitech Blue VO!CE, Rode APHEX:** onboard DSP presets.
- **NVIDIA Broadcast / Krisp / Windows Voice Clarity / Studio Effects:** AI cover-ups — we're the anti-band-aid.
- **Adobe Mic Check:** diagnose-only, browser.
- **Fiverr "fix my OBS mic" gigs** (name SM7B/MV7/GoXLR): humans already paid to do this.
- No vendor-neutral analyse → coach → apply → verify product found.

---

## 8. Still open (before/while building)

1. **Hardware lab:** buy/borrow the top ~10 mics, ~5 interfaces, ~6 webcams + a Cam Link; verify all [PK]/[L] items (device names, VID/PIDs, address directions, UVC controls, persistence).
2. **Test corpus:** record launch mics in 2–3 rooms with deliberate faults (too far, gain maxed, desk reflection, fan, left-only, plosives, BT) → regression set for the analyser.
3. **Audio engineer** (short contract): validate targets, EQ rules, profiles; compare their settings vs app's on the corpus.
4. **Blind listening tests:** do normal listeners prefer "after"? (Also marketing.)
5. **UX test** of a clickable prototype with 5–10 real streamers.
6. **Unverified tech:** Streamlabs filter/VST support + remote API; Twitch Studio status; `IMFCameraConfigurationManager` for non-companion apps; Multi-App Camera write restrictions in practice; EV cert pricing; obs-websocket auto-enable by editing config while OBS closed.
7. **Legal basics:** use own product photos; nominative brand names OK; affiliate disclosure; "audio never leaves your PC"; avoid "studio quality" claims.
8. **Coverage data refresh:** Amazon/Reddit/ProSettings weren't fetchable in research sessions; re-check ranks if network allows.

## 9b. Stream health module (OBS tuning, lag, bitrate) — added 2026-11-02

**Why it's cheap:** OBS already measures everything; obs-websocket exposes it (same connection v1 uses). `GetStats`: CPU, memory, activeFps, averageFrameRenderTime, renderSkippedFrames/renderTotalFrames, outputSkippedFrames/outputTotalFrames. `GetStreamStatus`: outputActive, outputReconnecting, outputCongestion, outputBytes, duration. Profile settings readable/settable (Get/SetProfileParameter, verify) or editable while OBS is closed. OBS log has an end-of-stream frame summary. [M–H; verify against official protocol doc]

**Diagnosis map (OBS's three frame-loss types → plain language):**
- Network dropped frames / congestion → "Your internet can't keep up": bitrate above upload, Wi-Fi, bad ingest server → lower bitrate, wired, auto server.
- Encoding lag → "Your PC can't encode fast enough": x264 preset too slow, GPU encoder saturated → switch to NVENC/AMF/QSV, faster preset, lower output res/fps.
- Rendering lag → "Your game is starving OBS": GPU at 100% → cap game fps, run OBS as admin, reduce sources/filters.

**Config audit rules:** encoder matches hardware; CBR; keyframe 2 s; bitrate fits res/fps and platform (Twitch guideline 6000 kbps for 1080p60; Enhanced Broadcasting/HEVC may allow more — unconfirmed; YouTube higher + HEVC/AV1); output res/downscale; audio AAC 160 kbps @ 48 kHz; wired vs Wi-Fi (Windows API); recording format (MKV/hybrid MP4).

**Existing alternatives:** OBS Auto-Configuration Wizard (incl. Twitch bandwidth test), Twitch Enhanced Broadcasting (auto-configures encoder on supported GPUs), Twitch Inspector, YouTube Live Control Room health. Gap: plain-language cause + fix, live alerts, post-stream report, combined with mic/cam.

**Cost tiers:**
1. Cheap (v1): read config + live stats, rule-based audit, plain-language live alerts, post-stream report from stats/log, hardware detection (GPU/CPU/connection type).
2. Medium (v1.5): guided bandwidth test, encoder benchmark under real game load, apply profile settings, per-game profiles.
3. Expensive (later/never): visual quality scoring (VMAF-style, needs reference capture), own live adaptive bitrate (OBS already has dynamic bitrate), platform-side viewer analytics, multi-PC/capture setups, ISP routing diagnosis.

## 9. Suggested v1 one-shot scope

Windows + OBS (obs-websocket) · our VST3 (HPF/PEQ/de-ess/mono) · profiles: SM7B/PodMic via Scarlett Solo/2i2 + Wave XLR, Yeti family, QuadCast family, Fifine AM8/K669, MV7/MV7+, generic fallbacks, headset-by-connection · cams: C920/C922, Brio family, Facecam family, generic UVC, capture-card coach-only · all launch quick-win checks · guided test → score → coach (Done/Can't) → apply → verify → before/after (+ phone-speaker preview) → re-apply on startup.
Analyser built as a testable library first, validated offline against the test corpus.
