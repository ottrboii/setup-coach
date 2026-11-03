# Setup Coach — first smoke test (your PC)

First run on real Windows hardware. Nothing here is risky: the app only
changes OBS filters and a few Windows sound settings, and OBS file edits are
backed up. Take a screenshot whenever something looks wrong or confusing.

## 0. Download (≈ 3.6 MB)

GitHub → setup-coach → **Actions** → *CI* → latest green run on `main` → **Artifacts**:
- `setup-coach-installer` (zip with the `.exe`)
- `setup-coach-voice-vst3` (zip with `SetupCoachVoice.vst3`; already included in the installer, only needed standalone)

## 1. Install

1. OBS 33.0 can't load VST3 yet (native support is expected ~33.1), so install
   **atkAudio** (free OBS plugin) for now. Without it the app tells you so and
   still sets OBS's own gate/compressor/limiter.
2. OBS → *Tools → WebSocket Server Settings* → tick **Enable**. (The app can
   also read the password itself after you allow it.)
3. Run the Setup Coach installer. It isn't signed yet, so Windows shows
   "Windows protected your PC" → **More info → Run anyway**. It asks for admin
   permission once (UAC) because it also installs the VST3 plugin to
   `C:\Program Files\Common Files\VST3\SetupCoachVoice.vst3` (and removes it on
   uninstall). No manual copying. If you installed an earlier per-user build,
   uninstall it first (Settings → Apps).

## Self-test (one command, one zip)

With OBS open, run this in PowerShell (or let Claude Code on your PC run it):

```powershell
& "C:\Program Files\Setup Coach\setup-coach.exe" --self-test | Out-String
```

No window opens and nothing asks questions. It checks the app and Windows
versions, mics (default device, sample rate, Voice Clarity / NVIDIA / Krisp),
cameras, the installed voice plugin, OBS + its WebSocket, OBS VST3 support
(atkAudio / native), GoXLR Utility, and your saved setup. Then it does one safe
OBS test: it makes a hidden scene **"Setup Coach self-test"** with an empty
source, puts the mic filters on it, reads their order back and deletes both
again. Your own scenes and sources aren't touched (the test checks that too).
Each check gives up after 5 seconds.

It prints the path of `SetupCoach-selftest-<date>-<time>.zip` on your
**Desktop** (the path is also saved in `%APPDATA%\SetupCoach\last-self-test.txt`).
The zip holds `self-test.txt` (readable summary), `self-test.json`, everything
in `%APPDATA%\SetupCoach\` and the newest OBS log. It doesn't include passwords.
**Send that zip.**

Options: `--out <folder>` (save the zip somewhere else), `--skip-obs` (don't
connect to OBS), `--no-obs-password` (don't read OBS's saved WebSocket password). Voice recordings are left out of the zip unless you add `--include-recordings`.

## 2. Run through (OBS open)

| # | Do | Check |
|---|---|---|
| 1 | Open the app, allow it to read OBS settings | OBS shows as connected; your mics (GoXLR outputs, DJI Mic 3 receiver, headset) and cameras (C920, Cam Link 4K) are listed with sensible names |
| 2 | GoXLR: pick the mic | It asks which mic is on the GoXLR → choose **EV RE20**; it should point OBS at *Broadcast Stream Mix* |
| 3 | **Quick scan** | Findings make sense? Note any that are wrong. Tick one harmless auto-fix, apply, check OBS changed as described |
| 4 | **Mic test** with the RE20, sitting as you stream | Level meter moves live; all 7 steps work |
| 5 | Mic report | Does the score and the list match how you think you sound? Try **"I can't do this"** on one item — it should re-tune |
| 6 | **Set up my mic** | In OBS, the mic input gets: Setup Coach Voice (via atkAudio), expander, compressor, limiter — in that order |
| 7 | Before / after + phone toggle | Does *after* sound better? Honest opinion is the most useful data |
| 8 | Repeat 4–7 quickly with the **DJI Mic 3** and a **headset** | Different findings per mic? |
| 9 | **Camera**: C920, hold a white sheet in the box when asked | Report + apply; does the image look better in OBS? |
| 10 | Unplug/replug the C920 | Known gap: settings aren't re-applied yet — note what happens |
| 11 | Camera: **a6000 via Cam Link** | Should switch to camera-menu instructions instead of changing settings |
| 12 | **Stream health**: start monitoring, then a short test stream (Twitch: add `?bandwidthtest=true` to the stream key to avoid going live) | Status lights + after-stream report appear |
| 13 | Close and reopen the app | Saved setup is re-applied |

## 3. Send back

- Screenshots of anything wrong/confusing.
- The self-test zip from your Desktop (run the self-test again at the end;
  it already contains `%APPDATA%\SetupCoach\` and the latest OBS log).
- One line per mic: did *after* sound better than *before*?
