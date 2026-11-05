# Handover: "What mic is behind this device?" (DONE 2026-11-05, branch app-mic-behind-device)

Status: built end to end (see PR). Backend `list_mic_profiles` / `get_mic_behind` / `set_mic_behind` (app/src-tauri/src/app/micchoice.rs, vault item `mic_choice`), `mic_behind` in setup.json, chosen profile drives `get_environment`, `mic_context`, auto gain. UI: app/ui/micpicker.js used on Welcome (gates the scan), scan, mic test (gates Start) and mic report (re-analyses the last recording on change). Documented in app/API.md. Still needs Otto's real PC: pick RE20 on "Chat Mic" and look at the thresholds/questions.
Original brief below.
# Handover: "What mic is behind this device?" (not started)

From Otto's first real run (2026-11-05). Otto picked the right device ("Chat Mic", a GoXLR input) but the GoXLR hides the real mic. Decision (coordinator): no auto-guessing. Ask the user.

## To build
1. New backend commands (add to app/API.md, approved): list the mic profiles from `profiles/*.json` (id, brand, model, for the picker and search) and set/get the "physical mic behind device X" per device id, stored in the saved setup (`setup.json`) and written through the settings vault like other writes (Target + capture/restore in `app/src-tauri/src/app/vault_ops.rs`).
2. Use the chosen profile in `get_environment` (`match_mic_view`) and `mic_context` (it already honours `answers.mic_model_id`): thresholds, questions and advice should come from the chosen mic, not the interface name.
3. UI (welcome.js): ask once the first time an interface/GoXLR-type or generic-named device is selected ("What mic is plugged into this?", list from profiles, search, "Other", "Not sure / skip" allowed). Show "Your mic: Chat Mic (EV RE20)" with a Change link on the scan (done, name only), mic test and mic report screens.
4. mock.js and mock-coach.js: cover the new commands. Tests: core mock flow test that the chosen profile changes the MicContext.

## Already done in this branch
- scan.js shows "Your mic: <device name> · Change" (links to #/welcome). Mic test screen not yet.
- welcome.js now reads `profile.questions` (real backend) as well as `needs_questions` (mock). Before, the profile questions never showed on a real PC because the field name differed.
- Default pick on a real GoXLR PC: `state.micId ??= default mic || first`. The Windows default here is not necessarily the GoXLR; the user is never forced to confirm. Needs the confirm step above.

## Facts from this PC (has a GoXLR Mini + OBS 33)
Mics listed: Line (Elgato Sound Capture), VOD / Broadcast Stream Mix / Chat Mic (GoXLR Mini), Microphone (C922).
