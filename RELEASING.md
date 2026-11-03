# Releasing Setup Coach

Installed copies update themselves: on start (after about 10 seconds) the app
looks at the newest **published** GitHub release, downloads the signed
installer, asks "A new version is ready. Install now?" and installs it. Nobody
downloads zips from GitHub Actions any more. Your settings and backups in
`%APPDATA%\SetupCoach\` are never touched.

## One-time setup (about 10 minutes)

Do this on your Windows PC (or let your local Claude session do the commands).

1. Generate the signing key pair (in PowerShell, from the repo's `app` folder):

   ```powershell
   npx @tauri-apps/cli@2 signer generate -w "$env:USERPROFILE\.tauri\setup-coach.key"
   ```

   Choose a password when asked. This creates `setup-coach.key` (private) and
   `setup-coach.key.pub` (public).
2. **Back up `setup-coach.key` and the password offline** (USB stick, password
   manager). If you lose them, installed apps can never be updated again and
   everyone must reinstall by hand. Never put the key in the repo or in chat.
3. On GitHub open the repo: **Settings > Secrets and variables > Actions**.
   - Tab **Secrets**, "New repository secret":
     - `TAURI_SIGNING_PRIVATE_KEY` = the full contents of `setup-coach.key`
     - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = the password from step 1
   - Tab **Variables**, "New repository variable":
     - `TAURI_UPDATER_PUBKEY` = the full contents of `setup-coach.key.pub`
     (the public key is not secret).

The release build stops with a clear message if any of these three is missing.
Normal pull-request builds do not need them and keep working without.

## Every release

1. Bump the version in `app/src-tauri/tauri.conf.json` and
   `app/src-tauri/Cargo.toml` (same number, e.g. `0.2.0`), merge to `main` with
   green CI.
2. Create and push the tag (the message becomes the release notes users see):

   ```bash
   git tag -a v0.2.0 -m "Mic coaching is faster and the camera test remembers your lighting."
   git push origin v0.2.0
   ```
3. The **Release** workflow builds the VST3, the signed installer and
   `latest.json`, and creates a **draft** release. Nothing is public yet.
4. Open GitHub > Releases, check the draft (files: installer, `latest.json`,
   VST3 zip), then press **Publish release**. Users get the update at their
   next start.

If the workflow fails because the tag does not match the version in
`tauri.conf.json`, fix the version, delete the tag, and tag again.

## Good to know

- **The first updater-enabled version must be installed by hand once.** Older
  builds have no updater. After that, all updates are automatic.
- The installer is per-machine, so every update shows one Windows permission
  (UAC) prompt.
- If OBS is running, the app asks the user to close it first so the voice
  plugin (VST3) can be replaced.
- Choosing "Later" simply asks again at the next start.
- Signing the installer for Windows SmartScreen (Azure Artifact Signing or an OV
  certificate) is separate from this updater signing and is not set up yet.
