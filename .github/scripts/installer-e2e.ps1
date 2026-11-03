# Installer end-to-end test (Windows CI): silent install -> files -> launch -> silent uninstall -> clean.
# Usage: pwsh .github/scripts/installer-e2e.ps1 [-InstallerGlob 'target/release/bundle/nsis/*.exe']
param([string]$InstallerGlob = 'target/release/bundle/nsis/*.exe')

$ErrorActionPreference = 'Stop'
$script:failed = $false
function Pass($m) { Write-Host "PASS: $m" }
function Fail($m) { Write-Host "FAIL: $m"; $script:failed = $true }
function Check($ok, $m) { if ($ok) { Pass $m } else { Fail $m } }
function Finish {
  if ($script:failed) { Write-Host 'INSTALLER E2E: FAILED'; exit 1 }
  Write-Host 'INSTALLER E2E: ALL CHECKS PASSED'; exit 0
}

$installer = Get-ChildItem $InstallerGlob | Select-Object -First 1
if (-not $installer) { Fail "no installer found at $InstallerGlob"; Finish }
Write-Host "Installer: $($installer.FullName) ($([int]($installer.Length/1MB)) MB)"

$installDir = Join-Path $env:ProgramFiles 'Setup Coach'
$vst3Dir = Join-Path $env:CommonProgramFiles 'VST3\SetupCoachVoice.vst3'
$vst3Bin = Join-Path $vst3Dir 'Contents\x86_64-win\SetupCoachVoice.vst3'

# 1. Silent install
$p = Start-Process -FilePath $installer.FullName -ArgumentList '/S' -PassThru -Wait
Check ($p.ExitCode -eq 0) "silent install exit code 0 (got $($p.ExitCode))"

# 2. Files
$uninstaller = Join-Path $installDir 'uninstall.exe'
Check (Test-Path $installDir) "install dir exists: $installDir"
if (Test-Path $installDir) { Get-ChildItem $installDir | ForEach-Object { Write-Host "  $($_.Name)" } }
$appExe = Get-ChildItem $installDir -Filter *.exe -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -notmatch '^uninstall' } | Select-Object -First 1
Check ($null -ne $appExe) "app exe present in install dir ($($appExe.Name))"
Check (Test-Path $vst3Bin) "VST3 binary installed: $vst3Bin"
Check (Test-Path $uninstaller) 'uninstall.exe present'

# 3. Launch smoke test
if ($appExe) {
  $app = Start-Process -FilePath $appExe.FullName -PassThru
  Start-Sleep -Seconds 10
  if (-not $app.HasExited) { Pass 'app still running after 10s (no crash on start)' }
  else { Fail "app exited within 10s (exit code $($app.ExitCode))" }
  Get-Process -Name $appExe.BaseName -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 3
  $data = Join-Path $env:APPDATA 'SetupCoach'
  if (Test-Path $data) {
    Write-Host "Contents of ${data}:"
    Get-ChildItem $data -Recurse | ForEach-Object { Write-Host "  $($_.FullName.Substring($data.Length)) ($($_.Length) B)" }
  } else { Write-Host "(no $data created)" }
}

# 4. Silent uninstall (NSIS re-spawns a temp copy, so poll for the result)
if (Test-Path $uninstaller) {
  $u = Start-Process -FilePath $uninstaller -ArgumentList '/S' -PassThru -Wait
  Write-Host "uninstaller exit code $($u.ExitCode)"
}
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline -and ((Test-Path $vst3Dir) -or (Test-Path $installDir))) { Start-Sleep -Seconds 2 }
Check (-not (Test-Path $vst3Dir)) 'VST3 folder removed after uninstall'
Check (-not (Test-Path $installDir)) 'install dir removed after uninstall'
if (Test-Path $installDir) { Get-ChildItem $installDir -Recurse | ForEach-Object { Write-Host "  left: $($_.FullName)" } }
Finish
