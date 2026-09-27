<#
.SYNOPSIS
  M0 research helper (read-only): dumps everything that decides JIS/US mapping per keyboard.

.DESCRIPTION
  - Keyboard-class devnodes (present only by default) with service, container, display name,
    per-device override values from "Device Parameters", and the type/subtype reported via Raw Input.
  - Global i8042prt\Parameters values (fixed mode vs per-keyboard mode).
  - Input-method lists for the current user and for the sign-in screen (HKU\.DEFAULT).
  Nothing is written. Use -Json to get machine-readable output for before/after diffs.
#>
[CmdletBinding()]
param(
    [switch]$IncludePhantom,
    [switch]$Json
)

$ErrorActionPreference = 'Stop'

$src = @'
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public static class MklmRawInput {
  [StructLayout(LayoutKind.Sequential)] struct RAWINPUTDEVICELIST { public IntPtr hDevice; public uint dwType; }
  [StructLayout(LayoutKind.Sequential)] struct RID_DEVICE_INFO_KEYBOARD { public uint dwType, dwSubType, dwKeyboardMode, dwNumberOfFunctionKeys, dwNumberOfIndicators, dwNumberOfKeysTotal; }
  [StructLayout(LayoutKind.Sequential)] struct RID_DEVICE_INFO { public uint cbSize; public uint dwType; public RID_DEVICE_INFO_KEYBOARD kb; }
  [DllImport("user32.dll", SetLastError=true)] static extern uint GetRawInputDeviceList([Out] RAWINPUTDEVICELIST[] l, ref uint n, uint cb);
  [DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern uint GetRawInputDeviceInfoW(IntPtr h, uint cmd, StringBuilder data, ref uint size);
  [DllImport("user32.dll", SetLastError=true)] static extern uint GetRawInputDeviceInfoW(IntPtr h, uint cmd, ref RID_DEVICE_INFO data, ref uint size);
  [DllImport("user32.dll")] public static extern int GetKeyboardType(int f);
  [DllImport("user32.dll")] static extern int GetKeyboardLayoutList(int n, [Out] IntPtr[] list);
  public static string[] Keyboards() {
    var res = new List<string>(); uint n = 0; uint cb = (uint)Marshal.SizeOf(typeof(RAWINPUTDEVICELIST));
    GetRawInputDeviceList(null, ref n, cb); var arr = new RAWINPUTDEVICELIST[n]; GetRawInputDeviceList(arr, ref n, cb);
    foreach (var d in arr) {
      if (d.dwType != 1) continue;
      uint sz = 0; GetRawInputDeviceInfoW(d.hDevice, 0x20000007, null, ref sz);
      var sb = new StringBuilder((int)sz + 1); GetRawInputDeviceInfoW(d.hDevice, 0x20000007, sb, ref sz);
      var info = new RID_DEVICE_INFO(); info.cbSize = (uint)Marshal.SizeOf(typeof(RID_DEVICE_INFO)); uint isz = info.cbSize;
      GetRawInputDeviceInfoW(d.hDevice, 0x2000000b, ref info, ref isz);
      res.Add(sb.ToString() + "|" + info.kb.dwType + "|" + info.kb.dwSubType);
    }
    return res.ToArray();
  }
  public static string[] Layouts() {
    int n = GetKeyboardLayoutList(0, null); var a = new IntPtr[n]; GetKeyboardLayoutList(n, a);
    var r = new List<string>(); foreach (var h in a) r.Add(((long)h).ToString("X8")); return r.ToArray();
  }
}
'@
if (-not ('MklmRawInput' -as [type])) { Add-Type -TypeDefinition $src }

$internalContainer = '{00000000-0000-0000-FFFF-FFFFFFFFFFFF}'
$overrideNames = 'KeyboardTypeOverride', 'KeyboardSubtypeOverride', 'OverrideKeyboardType', 'OverrideKeyboardSubtype',
                 'KeyboardNumberTotalKeysOverride', 'KeyboardNumberFunctionKeysOverride', 'KeyboardNumberIndicatorsOverride'

function Get-Prop([string]$id, [string]$key) {
    (Get-PnpDeviceProperty -InstanceId $id -KeyName $key -ErrorAction SilentlyContinue).Data
}

# Raw Input: interface path -> instance id (\\?\HID#A#B#{guid} -> HID\A\B)
$raw = @{}
foreach ($line in [MklmRawInput]::Keyboards()) {
    $path, $type, $sub = $line -split '\|'
    $parts = $path.Substring(4) -split '#'
    if ($parts.Count -ge 3) { $raw[("{0}\{1}\{2}" -f $parts[0], $parts[1], $parts[2]).ToUpperInvariant()] = @{ Type = [int]$type; Subtype = [int]$sub } }
}

$devices = if ($IncludePhantom) { Get-PnpDevice -Class Keyboard } else { Get-PnpDevice -Class Keyboard -PresentOnly }
$rows = foreach ($d in $devices) {
    $id = $d.InstanceId
    $container = Get-Prop $id 'DEVPKEY_Device_ContainerId'
    $service = Get-Prop $id 'DEVPKEY_Device_Service'

    # Display name: top-most ancestor that still shares the container (bus-reported name preferred).
    $name = $d.FriendlyName
    if ($container -and $container -ne $internalContainer) {
        $cur = $id
        while ($cur) {
            $parent = Get-Prop $cur 'DEVPKEY_Device_Parent'
            if (-not $parent -or (Get-Prop $parent 'DEVPKEY_Device_ContainerId') -ne $container) { break }
            $bus = Get-Prop $parent 'DEVPKEY_Device_BusReportedDeviceDesc'
            $fn = (Get-PnpDevice -InstanceId $parent -ErrorAction SilentlyContinue).FriendlyName
            if ($bus) { $name = $bus } elseif ($fn) { $name = $fn }
            $cur = $parent
        }
    }

    $values = [ordered]@{}
    try {
        $dp = Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Enum\$id\Device Parameters" -ErrorAction Stop
        foreach ($n in $overrideNames) { if ($null -ne $dp.$n) { $values[$n] = $dp.$n } }
    } catch { }

    $ri = $raw[$id.ToUpperInvariant()]
    [pscustomobject]@{
        InstanceId   = $id
        Name         = $name
        Present      = [bool]$d.Present
        Service      = $service
        Internal     = ($container -eq $internalContainer)
        ContainerId  = $container
        Overrides    = $values
        RawType      = if ($ri) { $ri.Type } else { $null }
        RawSubtype   = if ($ri) { $ri.Subtype } else { $null }
    }
}

$g = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters' -ErrorAction SilentlyContinue
$global = [ordered]@{
    LayerDriverJPN             = $g.'LayerDriver JPN'
    LayerDriverKOR             = $g.'LayerDriver KOR'
    OverrideKeyboardIdentifier = $g.OverrideKeyboardIdentifier
    OverrideKeyboardType       = $g.OverrideKeyboardType
    OverrideKeyboardSubtype    = $g.OverrideKeyboardSubtype
    Mode                       = if ($null -ne $g.OverrideKeyboardType -or $null -ne $g.OverrideKeyboardSubtype) { 'fixed' } else { 'per-keyboard' }
}

function Get-Preload([string]$root) {
    $p = Get-ItemProperty "Registry::$root\Keyboard Layout\Preload" -ErrorAction SilentlyContinue
    if (-not $p) { return @() }
    $p.PSObject.Properties | Where-Object { $_.Name -match '^\d+$' } | Sort-Object { [int]$_.Name } | ForEach-Object { $_.Value }
}

$state = [ordered]@{
    Keyboards       = @($rows)
    Global          = $global
    PreloadUser     = @(Get-Preload 'HKEY_CURRENT_USER')
    PreloadSignIn   = @(Get-Preload 'HKEY_USERS\.DEFAULT')
    LoadedLayouts   = @([MklmRawInput]::Layouts())
    GetKeyboardType = '{0}/{1}' -f [MklmRawInput]::GetKeyboardType(0), [MklmRawInput]::GetKeyboardType(1)
}

if ($Json) {
    $state | ConvertTo-Json -Depth 5
} else {
    $state.Keyboards | ForEach-Object {
        $ov = ($_.Overrides.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ', '
        [pscustomobject]@{
            Name     = $_.Name
            Service  = $_.Service
            Internal = $_.Internal
            Raw      = if ($null -ne $_.RawType) { '0x{0:X}/0x{1:X}' -f $_.RawType, $_.RawSubtype } else { '-' }
            Override = if ($ov) { $ov } else { '(none)' }
            Instance = $_.InstanceId
        }
    } | Format-Table -AutoSize -Wrap
    "Global (i8042prt\Parameters): " + (($global.GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join '; ')
    "Preload (user):    " + ($state.PreloadUser -join ', ')
    "Preload (sign-in): " + ($state.PreloadSignIn -join ', ')
    "Loaded HKLs:       " + ($state.LoadedLayouts -join ', ')
    "GetKeyboardType:   " + $state.GetKeyboardType
}
