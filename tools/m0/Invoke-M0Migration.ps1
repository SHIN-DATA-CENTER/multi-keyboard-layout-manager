<#
.SYNOPSIS
  M0 #4 experiment: switch from fixed mode to per-keyboard mode while keeping every PS/2 keyboard's mapping.

.DESCRIPTION
  Forward (default), in this order (INV-PS2):
    1. Save the current global i8042prt\Parameters values and every i8042prt keyboard's device values to JSON.
    2. Write the current global OverrideKeyboardType/Subtype to each i8042prt keyboard's "Device Parameters"
       (including non-present ones) so their mapping does not change.
    3. Delete the global OverrideKeyboardType/Subtype. LayerDriver JPN and OverrideKeyboardIdentifier stay as they are.
  -Undo, in reverse order:
    1. Restore the global OverrideKeyboardType/Subtype from the saved JSON.
    2. Restore each PS/2 keyboard's device values to what they were before (deleting values that did not exist).
  A PC restart is required after either direction. Requires an elevated PowerShell.
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [switch]$Undo,
    [string]$StateDir = (Join-Path ([Environment]::GetFolderPath('Desktop')) 'MKLM-backup-20260927'),
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'
if ($LogPath) { Start-Transcript -Path $LogPath -Force | Out-Null }

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

$globalKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters'
$statePath = Join-Path $StateDir 'm0-4-premigration.json'
New-Item -ItemType Directory -Force $StateDir | Out-Null

function Get-DwordOrNull([string]$path, [string]$name) {
    $item = Get-ItemProperty -Path $path -ErrorAction SilentlyContinue
    if ($item -and $null -ne $item.$name) { return [int]$item.$name }
    return $null
}

function Set-OrRemove([string]$path, [string]$name, $value) {
    if ($null -eq $value) {
        Remove-ItemProperty -Path $path -Name $name -ErrorAction SilentlyContinue
    } else {
        New-ItemProperty -Path $path -Name $name -PropertyType DWord -Value $value -Force | Out-Null
    }
}

$ps2 = Get-PnpDevice -Class Keyboard | Where-Object {
    (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_Service -ErrorAction SilentlyContinue).Data -eq 'i8042prt'
}
if (-not $ps2) { throw 'No i8042prt keyboard devnode found; nothing to protect, so this experiment does not apply.' }

if (-not $Undo) {
    if (Test-Path $statePath) { throw "State file already exists ($statePath). Run with -Undo first, or move it away." }

    $gType = Get-DwordOrNull $globalKey 'OverrideKeyboardType'
    $gSub = Get-DwordOrNull $globalKey 'OverrideKeyboardSubtype'
    if ($null -eq $gType -or $null -eq $gSub) { throw 'Global OverrideKeyboardType/Subtype are already absent (per-keyboard mode). Nothing to migrate.' }

    $devices = foreach ($d in $ps2) {
        $dp = "HKLM:\SYSTEM\CurrentControlSet\Enum\$($d.InstanceId)\Device Parameters"
        [ordered]@{
            InstanceId = $d.InstanceId
            Path       = $dp
            Type       = Get-DwordOrNull $dp 'OverrideKeyboardType'
            Subtype    = Get-DwordOrNull $dp 'OverrideKeyboardSubtype'
        }
    }
    $state = [ordered]@{ GlobalType = $gType; GlobalSubtype = $gSub; Devices = @($devices) }
    $state | ConvertTo-Json -Depth 4 | Set-Content -Path $statePath -Encoding UTF8
    Write-Host "Saved pre-migration state: $statePath"

    if ($PSCmdlet.ShouldProcess('i8042prt keyboards + global Parameters', 'migrate to per-keyboard mode')) {
        # Step 2: pin every PS/2 keyboard to the current global type first.
        foreach ($dev in $devices) {
            if (-not (Test-Path $dev.Path)) { New-Item -Path $dev.Path -Force | Out-Null }
            Set-OrRemove $dev.Path 'OverrideKeyboardType' $gType
            Set-OrRemove $dev.Path 'OverrideKeyboardSubtype' $gSub
            Write-Host ("Pinned {0} to {1}/{2}" -f $dev.InstanceId, $gType, $gSub)
        }
        # Step 3: only then remove the global fixed-mode values.
        Remove-ItemProperty -Path $globalKey -Name 'OverrideKeyboardType' -ErrorAction Stop
        Remove-ItemProperty -Path $globalKey -Name 'OverrideKeyboardSubtype' -ErrorAction Stop
        Write-Host 'Removed global OverrideKeyboardType/Subtype (per-keyboard mode).'
    }
} else {
    if (-not (Test-Path $statePath)) { throw "No saved state at $statePath; cannot undo." }
    $state = Get-Content $statePath -Raw | ConvertFrom-Json

    if ($PSCmdlet.ShouldProcess('global Parameters + i8042prt keyboards', 'restore fixed mode')) {
        # Reverse order: global first, then the per-device pins.
        Set-OrRemove $globalKey 'OverrideKeyboardType' $state.GlobalType
        Set-OrRemove $globalKey 'OverrideKeyboardSubtype' $state.GlobalSubtype
        Write-Host ("Restored global OverrideKeyboardType/Subtype = {0}/{1}" -f $state.GlobalType, $state.GlobalSubtype)
        foreach ($dev in $state.Devices) {
            Set-OrRemove $dev.Path 'OverrideKeyboardType' $dev.Type
            Set-OrRemove $dev.Path 'OverrideKeyboardSubtype' $dev.Subtype
            Write-Host ("Restored {0} device values" -f $dev.InstanceId)
        }
        Move-Item $statePath ($statePath + '.undone-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    }
}

Write-Host '--- global ---'
& reg.exe query 'HKLM\SYSTEM\CurrentControlSet\Services\i8042prt\Parameters'
foreach ($d in $ps2) {
    Write-Host "--- $($d.InstanceId) ---"
    & reg.exe query "HKLM\SYSTEM\CurrentControlSet\Enum\$($d.InstanceId)\Device Parameters"
}
Write-Host 'Restart the PC (Restart, not Shut down) for the change to take effect.'
