$ErrorActionPreference = 'SilentlyContinue'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class UsbProbe {
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  static extern SafeFileHandle CreateFile(string n, uint a, uint s, IntPtr sec, uint d, uint f, IntPtr t);
  [DllImport("kernel32.dll", SetLastError=true)]
  static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] i, int il, byte[] o, int ol, out int ret, IntPtr ov);
  // 1 = bus-powered hub, 0 = self-powered, -1 unknown. IOCTL_USB_GET_NODE_INFORMATION.
  public static int BusPowered(string hubPath) {
    using (var h = CreateFile(hubPath, 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero)) {
      if (h.IsInvalid) return -1;
      var b = new byte[128];
      int r;
      if (!DeviceIoControl(h, 0x220408, b, 128, b, 128, out r, IntPtr.Zero)) return -1;
      if (b[0] != 0) return -1; // not a hub node
      return b[75] != 0 ? 1 : 0;
    }
  }
  // V1 speed: 0 low, 1 full, 2 high, 3 super; -1 on failure.
  public static int Speed(string hubPath, uint port) {
    using (var h = CreateFile(hubPath, 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero)) {
      if (h.IsInvalid) return -2;
      var b = new byte[1024];
      BitConverter.GetBytes(port).CopyTo(b, 0);
      int r;
      if (!DeviceIoControl(h, 0x220448, b, 1024, b, 1024, out r, IntPtr.Zero)) return -3;
      return b[23];
    }
  }
  // Returns [protocols, flags] or null. IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX_V2.
  public static uint[] Query(string hubPath, uint port) {
    using (var h = CreateFile(hubPath, 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero)) {
      if (h.IsInvalid) return null;
      var b = new byte[16];
      BitConverter.GetBytes(port).CopyTo(b, 0);
      BitConverter.GetBytes((uint)16).CopyTo(b, 4);
      int r;
      if (!DeviceIoControl(h, 0x220468, b, 16, b, 16, out r, IntPtr.Zero)) return null;
      return new uint[] { BitConverter.ToUInt32(b, 8), BitConverter.ToUInt32(b, 12) };
    }
  }
}
'@

$boot = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
$since = (Get-Date).AddDays(-7)
$removals = @{}
$starts = @{}
$runs = @{}
# 410 = device started, 420 = device removed. Oldest first so a start is paired with the next removal.
Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-Kernel-PnP/Configuration'; Id=410,420; StartTime=$since} -MaxEvents 4000 | Sort-Object TimeCreated | ForEach-Object {
  $k = ([string]$_.Properties[0].Value).ToUpper()
  if ($k -notlike 'USB\*') { return }
  if ($_.Id -eq 410) { $starts[$k] = $_.TimeCreated; return }
  $removals[$k] = 1 + [int]$removals[$k]
  if ($starts.ContainsKey($k)) {
    $mins = [int][Math]::Round(($_.TimeCreated - $starts[$k]).TotalMinutes)
    if (-not $runs.ContainsKey($k)) { $runs[$k] = @() }
    $runs[$k] += $mins
    $starts.Remove($k)
  }
}

$tops = @{}
Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\VID_*' -and $_.Class -in 'Camera','Image','MEDIA','AudioEndpoint' } | ForEach-Object {
  $top = $_.InstanceId
  if ($top -like '*&MI_*') { $top = [string](Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_Parent').Data }
  if (-not $top) { return }
  if (-not $tops.ContainsKey($top)) { $tops[$top] = @() }
  $tops[$top] += $_
}
$out = @()
foreach ($top in $tops.Keys) {
  $kidsOf = $tops[$top]
  $cam = [bool]($kidsOf | Where-Object { $_.Class -in 'Camera','Image' })
  $aud = [bool]($kidsOf | Where-Object { $_.Class -in 'MEDIA','AudioEndpoint' })
  $role = if ($cam) { 'camera' } else { 'audio' }
  $props = @{}
  Get-PnpDeviceProperty -InstanceId $top | ForEach-Object { $props[$_.KeyName] = $_.Data }
  $parent = [string]$props['DEVPKEY_Device_Parent']
  $port = [uint32]($props['DEVPKEY_Device_Address'])
  $loc = [string]$props['DEVPKEY_Device_LocationPaths'] -split "`n" | Select-Object -First 1
  $name = [string]$props['DEVPKEY_Device_BusReportedDeviceDesc']
  if (-not $name) { $name = $kidsOf[0].FriendlyName }
  # Depth = number of USB(n) hops after USBROOT; > 1 means a hub sits in between.
  $segs = ([regex]::Matches($loc, 'USB\(\d+\)')).Count
  $ctl = ($loc -split '#USBROOT')[0]
  $speed = 'unknown'; $capable = $null
  $hubPath = '\\.\' + ($parent -replace '\\', '#') + '#{f18a0e88-c30c-11d0-8815-00a0c906bed8}'
  $sp = [UsbProbe]::Speed($hubPath, $port)
  if ($sp -ge 0) { $speed = @("low","full","high","super")[[Math]::Min($sp,3)] }
  $q = [UsbProbe]::Query($hubPath, $port)
  if ($q) {
    $proto = $q[0]; $fl = $q[1]
    $super = ($fl -band 1) -ne 0
    $capSuper = ($fl -band 2) -ne 0
    if ($super -and $speed -ne 'super') { $speed = 'super' }
    $capable = $capSuper
  }
  # Hub power: bus-powered hubs report it through the hub's own power data; keep it simple here.
  $parentName = (Get-PnpDevice -InstanceId $parent).FriendlyName
  $hubIsRoot = $parent -like 'USB\ROOT_HUB*'
  $busPowered = $false
  if (-not $hubIsRoot) { $busPowered = ([UsbProbe]::BusPowered($hubPath) -eq 1) }
  $vidpid = [regex]::Match($top, 'VID_([0-9A-F]{4})&PID_([0-9A-F]{4})', 'IgnoreCase')
  $drops = [int]$removals[$top.ToUpper()]
  # A camera body that Windows only sees as a stills device (no webcam/video function).
  $imageOnly = ($cam -and -not ($kidsOf | Where-Object { $_.Class -eq 'Camera' }))
  $out += [pscustomobject]@{
    name = $name; role = $role; also_audio = ($cam -and $aud)
    vid = if ($vidpid.Success) { [Convert]::ToInt32($vidpid.Groups[1].Value, 16) } else { 0 }
    pid = if ($vidpid.Success) { [Convert]::ToInt32($vidpid.Groups[2].Value, 16) } else { 0 }
    speed = $speed; superspeed_capable = $capable
    controller = $ctl; behind_hub = (-not $hubIsRoot); hub_bus_powered = $busPowered; hub_name = if ($hubIsRoot) { $null } else { $parentName }
    drops_7d = $drops
    image_only = $imageOnly
    run_minutes = [int[]]@($runs[$top.ToUpper()] | Where-Object { $_ -ne $null })
    connected_min = if ($starts.ContainsKey($top.ToUpper())) { [int][Math]::Round(((Get-Date) - $starts[$top.ToUpper()]).TotalMinutes) } else { $null }
  }
}
ConvertTo-Json -InputObject @($out) -Compress


