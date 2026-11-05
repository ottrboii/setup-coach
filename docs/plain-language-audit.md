# Plain-language audit (first-time user path)

Rule (Otto): the end user is very tech illiterate. Every question and label must be answerable without knowing any term; show where to look and what each choice sounds like; "I'm not sure" always leads to a safe default; no numbers outside Advanced.
Scope: a text sweep (grep of `app/ui` and the findings copy in `core/`), NOT a click-through of every screen. Items marked FIXED are in this PR.

## Fixed in this PR
- Welcome, profile questions: "How are the switches on your mic set?" with one button. Now one question per switch, exclusive answers, "I'm not sure", an expandable "Where is it? What does this mean?", and a "Got it / tap again to change" line.
- Welcome, gain question: "Is the gain knob ... turned all the way up?" (nonsense for a GoXLR). Now never asked for a GoXLR; for boxes with a real knob: "Look at the round knob that controls how loud your mic is..." with help.
- Welcome (OBS card): "the websocket password" -> "the password OBS made for this"; WebSocket step now says it is OBS's remote-control switch.
- Config booster blurb: "adds about +25 dB of clean gain" -> "makes a quiet mic louder without adding hiss" (number outside Advanced).

## Still to do (screen, exact text, problem, suggested wording)
1. Welcome/Mic questions, pattern knob: "Which pattern is your mic set to? (the symbol on the knob) Out of the box it is Cardioid." Options are internal names (Cardioid, Omni, Figure8). Suggest: "Which picture is the knob pointing at?" with a small drawn symbol per option and help; labels like "Heart shape (picks up what is in front)". Needs drawn art.
2. Welcome, "Is your mic's own sound processing switched on? (names of built-in effects)": effect names are jargon. Suggest per-device wording from the profile, with `help` (profile JSON now supports it; not yet filled for `onboard_dsp`).
3. Welcome, booster question: "Is there a small box (like a Cloudlifter or FetHead) between the mic and the interface?" Add help: what it looks like (a small metal box with two XLR sockets) and "No" for most people.
4. Welcome, camera question "Which camera is plugged into your capture card?" has a photo key but no help text/photos. Camera files were out of scope here.
5. Findings (core/src/audio/findings.rs:109) "We'll switch your mic to mono in OBS so both ears get your voice." Suggest: "We'll make your voice play in both ears." (also in the Apply screen).
6. core/src/audio/findings.rs:399 "...usually from sitting too far or a low-cut switch on the mic... check the mic's bass-cut switch is off." Jargon "low-cut", "bass-cut". Suggest: "...or the small switch that thins your voice. Check it is off (see the picture on the first screen)".
7. core/src/system/camera.rs:101, video/findings.rs:543: "Switch the video format to MJPEG." Suggest: "We'll change the camera's video type so it looks sharper and runs smoother."
8. core/src/video/findings.rs:257 "lock white balance (in Kelvin)": Suggest: "We'll lock the colour so your skin doesn't change tint." Remove "Kelvin".
9. core/src/system/encoder.rs:82, health/mod.rs:287: "NVENC / AMF / QuickSync", "x264 preset". Suggest: "Let your graphics card do the video work so your game runs smoother."
10. core/src/system/obs_audio.rs:172 "OBS's built-in remote control (WebSocket server) is switched off". Suggest dropping the bracket.
11. app/ui/screens/ab.js:111-112 shows LUFS and "matched_gain_db": confirm these sit in an Advanced section only (not verified in this pass).
12. Mic test / coach steps: copy lives in app/ui/coach/copy.js and `app/src-tauri/src/app/coaching.rs` ("phantom power", "48 V", "GoXLR Utility"): coaching.rs:263 says "48 V phantom power" on a main screen. Suggest: "Your mic needs a little power from the GoXLR. We'll switch that on for you."
13. Switch answers are not yet used by the engine: `MicAnswers` has no switch field, so the answer to "bass tilt" is stored in the UI only. Needs app-backend (`types.rs`) + core ctx so the "Your voice sounds thin" advice and EQ start point account for a switched-on bass cut.
14. Not covered (no time): report, apply, camera screens' own strings; recommended follow-up is a click-through with a real non-technical user.
