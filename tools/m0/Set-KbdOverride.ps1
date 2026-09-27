<#
.SYNOPSIS
  M0 research helper: writes or removes the per-device keyboard type override on ONE HID keyboard.

.DESCRIPTION
  Experiment tool only (the product will open keys via CM_Open_DevNode_Key, see the plan).
  - Requires an elevated PowerShell.
  - Refuses i8042prt (PS/2) devices and the internal container unless -AllowInternal is given,
    because deleting/altering the built-in keyboard's mapping can break sign-in (INV-PS2).
  - Never touches the global i8042prt\Parameters values.
  - Exports the device's "Device Parameters" key to a timestamped .reg file before writing.

.EXAMPLE
  .\Set-KbdOverride.ps1 -InstanceId 'HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000' -Layout US
  .\Set-KbdOverride.ps1 -InstanceId '...' -Layout Remove
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [Parameter(Mandatory)] [string]$InstanceId,
    [Parameter(Mandatory)] [ValidateSet('JIS', 'US', 'US70', 'Remove')] [string]$Layout,
    [string]$BackupDir = (Join-Path ([Environment]::GetFolderPath('Desktop')) 'MKLM-backup-20260927'),
    [switch]$AllowInternal,
    # M0 #2b: restart the devnode (pnputil /restart-device = DIF_PROPERTYCHANGE) instead of replugging.
    [switch]$Restart,
    [string]$LogPath
)

$ErrorActionPreference = 'Stop'
if ($LogPath) { Start-Transcript -Path $LogPath -Force | Out-Null }

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from an elevated PowerShell (Run as administrator).'
}

$dev = Get-PnpDevice -InstanceId $InstanceId
if ($dev.Class -ne 'Keyboard') { throw "Not a Keyboard-class devnode: $InstanceId (class=$($dev.Class))" }

$service = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_Service).Data
$container = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_ContainerId).Data
$internal = $container -eq '{00000000-0000-0000-FFFF-FFFFFFFFFFFF}'
if (($service -eq 'i8042prt' -or $internal) -and -not $AllowInternal) {
    throw "Refusing to modify an internal/PS2 keyboard ($service, container $container). Use -AllowInternal only for the planned M0 #4 test."
}

# kbdhid and i8042prt read differently named values.
switch ($service) {
    'kbdhid'   { $typeName = 'KeyboardTypeOverride'; $subName = 'KeyboardSubtypeOverride' }
    'i8042prt' { $typeName = 'OverrideKeyboardType'; $subName = 'OverrideKeyboardSubtype' }
    default    { throw "Unsupported keyboard service '$service'." }
}

$values = @{ JIS = @(7, 2); US = @(4, 0); US70 = @(7, 0) }
$regPath = "HKLM\SYSTEM\CurrentControlSet\Enum\$InstanceId\Device Parameters"

New-Item -ItemType Directory -Force $BackupDir | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$safe = ($InstanceId -replace '[\\/:*?"<>|&{}]', '_')
$backup = Join-Path $BackupDir "$stamp-$safe.reg"
& reg.exe export $regPath $backup /y | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Backup failed (reg export exit $LASTEXITCODE); nothing was changed." }
Write-Host "Backup: $backup"

if ($PSCmdlet.ShouldProcess($InstanceId, "set $Layout ($typeName/$subName)")) {
    if ($Layout -eq 'Remove') {
        foreach ($n in $typeName, $subName) { & reg.exe delete $regPath /v $n /f 2>$null | Out-Null }
    } else {
        $t, $s = $values[$Layout]
        & reg.exe add $regPath /v $typeName /t REG_DWORD /d $t /f | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Writing $typeName failed (exit $LASTEXITCODE)." }
        & reg.exe add $regPath /v $subName /t REG_DWORD /d $s /f | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Writing $subName failed (exit $LASTEXITCODE)." }
    }
    & reg.exe query $regPath

    if ($Restart) {
        $before = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_LastArrivalDate).Data
        & pnputil.exe /restart-device $InstanceId
        $code = $LASTEXITCODE
        Write-Host "pnputil /restart-device exit code: $code (0 = restarted, 3010 = reboot required)"
        Start-Sleep -Seconds 3
        $after = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_LastArrivalDate).Data
        $status = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_DevNodeStatus).Data
        $problem = (Get-PnpDeviceProperty -InstanceId $InstanceId -KeyName DEVPKEY_Device_ProblemCode).Data
        Write-Host ("LastArrivalDate: {0} -> {1}; DevNodeStatus=0x{2:X}; ProblemCode={3}" -f $before, $after, $status, $problem)
        $state = & (Join-Path $PSScriptRoot 'Get-KbdState.ps1') -Json | ConvertFrom-Json
        $me = $state.Keyboards | Where-Object { $_.InstanceId -eq $InstanceId }
        Write-Host ("Raw Input now reports 0x{0:X}/0x{1:X}" -f $me.RawType, $me.RawSubtype)
    } else {
        Write-Host "Done. Replug/reconnect the keyboard (or reboot for PS/2), then run Get-KbdState.ps1."
    }
}
