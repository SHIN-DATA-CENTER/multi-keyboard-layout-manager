@echo off
rem MKLM recovery script, generated 2026-09-27 04:00 UTC by MKLM 0.1.0.
rem Puts every keyboard value MKLM changed back to its value before MKLM (baseline).
rem   restore-offline.cmd online        Windows or Safe Mode, from an administrator prompt
rem   restore-offline.cmd offline D:    WinRE command prompt; D: is the Windows drive
rem Run this copy (on the Windows drive). A copy on a USB stick is for reading only.
setlocal EnableExtensions DisableDelayedExpansion
if /i "%~1"=="online" goto :online
if /i "%~1"=="offline" goto :offline
goto :usage

:online
set "ROOT=HKLM\SYSTEM\CurrentControlSet"
call :apply
exit /b %ERRORLEVEL%

:offline
if "%~2"=="" goto :usage
if not exist "%~2\Windows\System32\config\SYSTEM" (echo SYSTEM hive not found under %~2\Windows& exit /b 2)
reg load HKLM\MKLM_OFFLINE "%~2\Windows\System32\config\SYSTEM" >nul || (echo reg load failed& exit /b 3)
set "CS="
set "CUR="
for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Default ^| findstr /c:"Default"') do set /a CS=%%A
for /f "tokens=3" %%A in ('reg query HKLM\MKLM_OFFLINE\Select /v Current ^| findstr /c:"Current"') do set /a CUR=%%A
if not defined CS (reg unload HKLM\MKLM_OFFLINE >nul & echo Select\Default not found& exit /b 4)
if not "%CS%"=="%CUR%" goto :cs_mismatch
set "CSN=00%CS%"
set "ROOT=HKLM\MKLM_OFFLINE\ControlSet%CSN:~-3%"
call :apply
set "RC=%ERRORLEVEL%"
reg unload HKLM\MKLM_OFFLINE >nul
exit /b %RC%

:cs_mismatch
reg unload HKLM\MKLM_OFFLINE >nul
echo Select\Default is %CS% but Select\Current is %CUR%. Nothing was changed; see docs\recovery.md.
exit /b 5

:apply
set "FAILED="
rem PS/2 values set
reg add "%ROOT%\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" /v "OverrideKeyboardType" /t REG_DWORD /d 7 /f >nul || set "FAILED=1"
reg add "%ROOT%\Enum\ACPI\FUJ0309\4&320DB4C2&0\Device Parameters" /v "OverrideKeyboardSubtype" /t REG_DWORD /d 2 /f >nul || set "FAILED=1"
rem HID keyboards
reg add "%ROOT%\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters" /v "KeyboardTypeOverride" /t REG_DWORD /d 4 /f >nul || set "FAILED=1"
reg add "%ROOT%\Enum\HID\VID_3434&PID_D027&MI_00&COL01\8&148AD7E3&0&0000\Device Parameters" /v "KeyboardSubtypeOverride" /t REG_DWORD /d 0 /f >nul || set "FAILED=1"
rem global values deleted
call :del "%ROOT%\Services\i8042prt\Parameters" "OverrideKeyboardType"
call :del "%ROOT%\Services\i8042prt\Parameters" "OverrideKeyboardSubtype"
if defined FAILED (echo Some values could not be restored.& exit /b 1)
echo Done. Restart Windows (a restart, not a shutdown).
exit /b 0

:del
reg delete "%~1" /v "%~2" /f >nul 2>&1
reg query "%~1" /v "%~2" >nul 2>&1 && set "FAILED=1"
exit /b 0

:usage
echo usage: restore-offline.cmd online ^| offline D:
exit /b 2
