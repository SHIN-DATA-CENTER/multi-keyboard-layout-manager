//! Machine- and session-level actions of M2: boot ID, randomness, PC restart and the post-reboot
//! RunOnce entry; and the GUI's autostart Run value (M3). The two HKCU values are the only
//! registry values MKLM writes outside HKLM, and this module is the only one that writes them.

use std::path::Path;

use mklm_core::BootId;
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemTimeOfDayInformation,
};
use windows::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID,
};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_SHUTDOWN_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Registry::KEY_SET_VALUE;
use windows::Win32::System::Shutdown::{
    InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OPERATINGSYSTEM,
    SHTDN_REASON_MINOR_RECONFIG, SHUTDOWN_RESTART, SHUTDOWN_RESTARTAPPS,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{GUID, PCWSTR};
use windows_registry::CURRENT_USER;

use crate::error::{Error, is_not_found};
use crate::sys::{nt_check, own, raw, win32};

/// Name of the value MKLM puts under `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce`.
pub const RUN_ONCE_VALUE: &str = "SHINDATACENTER.MKLM.PostReboot";

/// The RunOnce key, relative to `HKEY_CURRENT_USER`.
const RUN_ONCE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";

/// Longest command line Windows runs from a Run / RunOnce value.
const RUN_ONCE_MAX_CHARS: usize = 260;

/// `SystemBootEnvironmentInformation`.
const SYSTEM_BOOT_ENVIRONMENT_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(90);

/// `SYSTEM_BOOT_ENVIRONMENT_INFORMATION` (ntexapi.h; not in the `windows` crate).
#[repr(C)]
#[allow(dead_code, reason = "the kernel's layout; only some fields are read")]
struct SystemBootEnvironmentInformation {
    /// A GUID the boot loader generates for every boot.
    boot_identifier: GUID,
    /// `FIRMWARE_TYPE`.
    firmware_type: i32,
    boot_flags: u64,
}

/// `SYSTEM_TIMEOFDAY_INFORMATION` (ntexapi.h; the `windows` crate only has it opaque).
#[repr(C)]
#[derive(Default)]
#[allow(dead_code, reason = "the kernel's layout; only some fields are read")]
struct SystemTimeOfDay {
    boot_time: i64,
    current_time: i64,
    time_zone_bias: i64,
    time_zone_id: u32,
    reserved: u32,
    boot_time_bias: u64,
    sleep_time_bias: u64,
}

/// Fills a fixed-size `NtQuerySystemInformation` structure.
///
/// # Safety
///
/// `T` must be the plain-data `#[repr(C)]` layout of `class`'s structure (every bit pattern
/// valid), so that the kernel may write up to `size_of::<T>()` bytes into it.
unsafe fn query_fixed<T>(
    function: &'static str,
    class: SYSTEM_INFORMATION_CLASS,
    info: &mut T,
) -> Result<(), Error> {
    let mut returned = 0u32;
    // SAFETY: `info` is a writable `T` of exactly the size passed; `returned` is a valid out
    // pointer. The layout is guaranteed by the caller.
    let status = unsafe {
        NtQuerySystemInformation(
            class,
            (info as *mut T).cast(),
            size_of::<T>() as u32,
            &mut returned,
        )
    };
    nt_check(function, status)
}

/// The current boot: `NtQuerySystemInformation(SystemBootEnvironmentInformation = 90)`
/// `.BootIdentifier`, a GUID the loader creates for every boot. Unlike the kernel boot time it
/// does not move with clock corrections, resume from sleep or `w32tm /resync` (design review C2).
/// Required properties, verified on the machine by H.2 R9/R10 before M2 is done: unchanged across
/// sleep, hibernation, a Fast Startup "shutdown" and a clock resync; changed by a full restart.
/// If the Fast Startup property fails, M2 falls back to `KUSER_SHARED_DATA.BootId` (a per-boot
/// counter) with the same tests. `SYSTEM_BOOT_ENVIRONMENT_INFORMATION` is defined in this module
/// with `#[repr(C)]` because the `windows` crate lacks it.
///
/// The GUID maps to [`BootId`] as `GUID::to_u128`, so that `BootId`'s text form is the GUID's
/// usual text form. An all-zero identifier is refused ([`Error::UnexpectedData`]).
pub fn boot_id() -> Result<BootId, Error> {
    let mut info = SystemBootEnvironmentInformation {
        boot_identifier: GUID::zeroed(),
        firmware_type: 0,
        boot_flags: 0,
    };
    // SAFETY: `SystemBootEnvironmentInformation` is the plain-data layout of class 90.
    unsafe {
        query_fixed(
            "NtQuerySystemInformation(SystemBootEnvironmentInformation)",
            SYSTEM_BOOT_ENVIRONMENT_INFORMATION,
            &mut info,
        )
    }?;
    match info.boot_identifier.to_u128() {
        0 => Err(Error::UnexpectedData {
            path: "SystemBootEnvironmentInformation.BootIdentifier".to_string(),
        }),
        id => Ok(BootId(id)),
    }
}

/// Kernel boot time minus `BootTimeBias`
/// (`NtQuerySystemInformation(SystemTimeOfDayInformation)`), FILETIME units. A diagnostic for the
/// journal's history only; never used to decide anything.
pub fn boot_time_hint() -> Result<u64, Error> {
    let mut info = SystemTimeOfDay::default();
    // SAFETY: `SystemTimeOfDay` is the plain-data layout of SystemTimeOfDayInformation.
    unsafe {
        query_fixed(
            "NtQuerySystemInformation(SystemTimeOfDayInformation)",
            SystemTimeOfDayInformation,
            &mut info,
        )
    }?;
    u64::try_from(info.boot_time)
        .ok()
        .and_then(|boot_time| boot_time.checked_sub(info.boot_time_bias))
        .ok_or_else(|| Error::UnexpectedData {
            path: "SystemTimeOfDayInformation.BootTime".to_string(),
        })
}

/// Fills `buf` from `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)`.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), Error> {
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        // SAFETY: `chunk` is a writable slice whose length fits the API's u32 size.
        let status = unsafe { BCryptGenRandom(None, chunk, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        nt_check("BCryptGenRandom", status)?;
    }
    Ok(())
}

/// A random (version 4) UUID, lower-case, hyphenated, without braces.
pub fn new_uuid() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    random_bytes(&mut bytes)?;
    // Version 4 (random), variant 10xx (RFC 9562).
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format_uuid(u128::from_be_bytes(bytes)))
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lower case.
fn format_uuid(value: u128) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (value >> 96) as u32,
        (value >> 80) as u16,
        (value >> 64) as u16,
        (value >> 48) as u16,
        value & 0xffff_ffff_ffff
    )
}

/// Restarts Windows (plan 3.6): enables `SE_SHUTDOWN_NAME` in this process's token, then
/// `InitiateShutdownW(NULL, NULL, 0, SHUTDOWN_RESTART | SHUTDOWN_RESTARTAPPS,
/// SHTDN_REASON_MAJOR_OPERATINGSYSTEM | SHTDN_REASON_MINOR_RECONFIG | SHTDN_REASON_FLAG_PLANNED)`.
/// Works unelevated (Users hold the privilege on client editions). Never a shutdown: Fast Startup
/// would keep the drivers.
pub fn restart_pc() -> Result<(), Error> {
    enable_shutdown_privilege()?;
    // SAFETY: no machine name (this PC) and no message; the flags and the reason are constants.
    let code = unsafe {
        InitiateShutdownW(
            PCWSTR::null(),
            PCWSTR::null(),
            0,
            SHUTDOWN_RESTART | SHUTDOWN_RESTARTAPPS,
            SHTDN_REASON_MAJOR_OPERATINGSYSTEM
                | SHTDN_REASON_MINOR_RECONFIG
                | SHTDN_REASON_FLAG_PLANNED,
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Win32 {
            function: "InitiateShutdownW",
            code,
        })
    }
}

/// Enables `SeShutdownPrivilege` in this process's token.
fn enable_shutdown_privilege() -> Result<(), Error> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle; `token` is a valid out pointer.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .map_err(|error| win32("OpenProcessToken", &error))?;
    // SAFETY: OpenProcessToken succeeded, so `token` is an open handle this process owns.
    let token = unsafe { own(token) };
    let mut luid = LUID::default();
    // SAFETY: local system (no name); SE_SHUTDOWN_NAME is a static string; `luid` is valid.
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid) }
        .map_err(|error| win32("LookupPrivilegeValueW", &error))?;
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `token` is open with TOKEN_ADJUST_PRIVILEGES; `privileges` is a valid
    // TOKEN_PRIVILEGES with one entry; the previous state is not requested.
    unsafe { AdjustTokenPrivileges(raw(&token), false, Some(&privileges), 0, None, None) }
        .map_err(|error| win32("AdjustTokenPrivileges", &error))?;
    // AdjustTokenPrivileges also succeeds when the token does not hold the privilege.
    // SAFETY: GetLastError has no preconditions.
    let code = unsafe { GetLastError() };
    if code == ERROR_NOT_ALL_ASSIGNED {
        return Err(Error::Win32 {
            function: "AdjustTokenPrivileges",
            code: code.0,
        });
    }
    Ok(())
}

/// Registers `command_line` (absolute, quoted executable path plus arguments) as
/// [`RUN_ONCE_VALUE`] in the current user's RunOnce key. Called by the unelevated GUI/CLI only;
/// the helper never touches HKCU (plan 2.2).
///
/// A command line that is not a quoted absolute path, contains a NUL or is longer than Windows
/// runs (260 characters) fails with `ERROR_INVALID_PARAMETER` before anything is written.
pub fn register_post_reboot(command_line: &str) -> Result<(), Error> {
    set_user_string(
        RUN_ONCE_KEY,
        RUN_ONCE_VALUE,
        command_line,
        is_run_once_command(command_line),
    )
}

/// Removes [`RUN_ONCE_VALUE`] (missing is fine).
pub fn unregister_post_reboot() -> Result<(), Error> {
    remove_user_value(RUN_ONCE_KEY, RUN_ONCE_VALUE)
}

/// Writes the `REG_SZ` value `name` = `data` under `HKCU\<key>` (created if missing), or fails
/// with `ERROR_INVALID_PARAMETER` before anything is written when `valid` is false. The only
/// HKCU writer (design m2 K, m3 A.5).
fn set_user_string(key: &str, name: &str, data: &str, valid: bool) -> Result<(), Error> {
    let path = format!(r"HKCU\{key}");
    if !valid {
        return Err(Error::Registry {
            path: format!(r"{path}\{name}"),
            code: ERROR_INVALID_PARAMETER.0,
        });
    }
    let opened = CURRENT_USER
        .options()
        .access(KEY_SET_VALUE.0)
        .create()
        .open(key)
        .map_err(|error| Error::registry(&path, &error))?;
    opened
        .set_string(name, data)
        .map_err(|error| Error::registry(format!(r"{path}\{name}"), &error))
}

/// Removes the value `name` under `HKCU\<key>`; a missing key or value is fine.
fn remove_user_value(key: &str, name: &str) -> Result<(), Error> {
    let path = format!(r"HKCU\{key}");
    let opened = match CURRENT_USER.options().access(KEY_SET_VALUE.0).open(key) {
        Ok(opened) => opened,
        Err(error) if is_not_found(&error) => return Ok(()),
        Err(error) => return Err(Error::registry(&path, &error)),
    };
    match opened.remove_value(name) {
        Ok(()) => Ok(()),
        Err(error) if is_not_found(&error) => Ok(()),
        Err(error) => Err(Error::registry(format!(r"{path}\{name}"), &error)),
    }
}

/// Name of the GUI's autostart value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
/// (plan 3.9; design m3 F.3): `"<absolute path>\mklm.exe" --tray`.
pub const AUTOSTART_VALUE: &str = "SHINDATACENTER.MKLM";

/// The Run key, relative to `HKEY_CURRENT_USER`.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where Task Manager's "Startup apps" switch records a disabled Run value (read only).
const STARTUP_APPROVED_RUN_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The autostart value as the user's Windows sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutostartState {
    /// The registered command line, if any.
    pub command_line: Option<String>,
    /// Task Manager (or Settings > Apps > Startup) turned it off: the first byte of the
    /// `StartupApproved\Run` value is odd (2 = on, 3 = off). MKLM then never turns it back on by
    /// itself (design m3 F.3).
    pub disabled_by_user: bool,
}

/// Reads [`AUTOSTART_VALUE`] and its Task Manager state (read only).
pub fn autostart_state() -> Result<AutostartState, Error> {
    let run_path = format!(r"HKCU\{RUN_KEY}");
    let command_line = match crate::reg::open_read(CURRENT_USER, "HKCU", RUN_KEY)? {
        Some(key) => crate::reg::read_string(&key, &run_path, AUTOSTART_VALUE)?,
        None => None,
    };
    let approved_path = format!(r"HKCU\{STARTUP_APPROVED_RUN_KEY}");
    let disabled_by_user =
        match crate::reg::open_read(CURRENT_USER, "HKCU", STARTUP_APPROVED_RUN_KEY)? {
            Some(key) => match key.get_value(AUTOSTART_VALUE) {
                Ok(value) => value.first().is_some_and(|flag| flag & 1 == 1),
                Err(error) if is_not_found(&error) => false,
                Err(error) => {
                    return Err(Error::registry(
                        format!(r"{approved_path}\{AUTOSTART_VALUE}"),
                        &error,
                    ));
                }
            },
            None => false,
        };
    Ok(AutostartState {
        command_line,
        disabled_by_user,
    })
}

/// The GUI's executable, the only program the autostart value may start.
const AUTOSTART_EXE: &str = "mklm.exe";

/// The only arguments the autostart value may carry: start in the taskbar corner.
const AUTOSTART_ARGUMENTS: &str = " --tray";

/// Registers `command_line` as [`AUTOSTART_VALUE`] under the current user's Run key. The GUI
/// writes its own per-user value (allowed: HKCU, plan 3.9); `session` stays the only module that
/// writes HKCU (design m2 K).
///
/// `command_line` must be `"<absolute path>\mklm.exe" --tray` and fit what Windows runs (checked
/// like the RunOnce command line, plus the file name and the arguments); anything else fails with
/// `ERROR_INVALID_PARAMETER` before anything is written. The Task Manager state
/// (`StartupApproved\Run`) is never touched: a value the user turned off stays off
/// (design m3 F.3).
pub fn register_autostart(command_line: &str) -> Result<(), Error> {
    set_user_string(
        RUN_KEY,
        AUTOSTART_VALUE,
        command_line,
        is_autostart_command(command_line),
    )
}

/// Removes [`AUTOSTART_VALUE`] from the current user's Run key (missing is fine). The Task
/// Manager state is left to Windows.
pub fn unregister_autostart() -> Result<(), Error> {
    remove_user_value(RUN_KEY, AUTOSTART_VALUE)
}

/// A RunOnce-style command line ([`is_run_once_command`]) that starts `mklm.exe` (any case) with
/// exactly `--tray`.
fn is_autostart_command(command_line: &str) -> bool {
    if !is_run_once_command(command_line) {
        return false;
    }
    let Some((exe, arguments)) = command_line
        .strip_prefix('"')
        .and_then(|tail| tail.split_once('"'))
    else {
        return false;
    };
    arguments == AUTOSTART_ARGUMENTS
        && Path::new(exe)
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(AUTOSTART_EXE))
}

/// `"<absolute path>"` followed by nothing or by a space and arguments; no NUL; at most
/// [`RUN_ONCE_MAX_CHARS`] UTF-16 units.
fn is_run_once_command(command_line: &str) -> bool {
    if command_line.contains('\0') || command_line.encode_utf16().count() > RUN_ONCE_MAX_CHARS {
        return false;
    }
    let Some((exe, rest)) = command_line
        .strip_prefix('"')
        .and_then(|tail| tail.split_once('"'))
    else {
        return false;
    };
    !exe.is_empty() && Path::new(exe).is_absolute() && (rest.is_empty() || rest.starts_with(' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_uuid_v4(text: &str) -> bool {
        let groups: Vec<&str> = text.split('-').collect();
        groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
            && text
                .chars()
                .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c))
            && groups[2].starts_with('4')
            && groups[3].starts_with(['8', '9', 'a', 'b'])
    }

    #[test]
    fn boot_id_is_stable_across_calls() {
        let first = boot_id();
        assert!(matches!(first, Ok(BootId(id)) if id != 0), "{first:?}");
        assert_eq!(boot_id(), first);
        assert_eq!(boot_id(), first);
    }

    /// Design H.2 R10 (and R9): prints the boot ID so that the orchestrator can compare it across
    /// sleep, hibernation, a Fast Startup shutdown, a clock resync and a restart. Harmless, but it
    /// is only meaningful as part of that manual procedure.
    #[test]
    #[ignore = "H.2 R9/R10: run by hand around sleep, hibernation, shutdown and restart"]
    fn print_boot_id() {
        let id = boot_id();
        let hint = boot_time_hint();
        println!("boot_id = {id:?}, boot_time_hint = {hint:?}");
        assert!(id.is_ok());
    }

    #[test]
    fn boot_time_hint_is_in_the_past() {
        let hint = boot_time_hint();
        // 2020-01-01 in FILETIME units; the boot cannot be earlier on a supported Windows 11.
        assert!(
            matches!(hint, Ok(time) if time > 132_223_104_000_000_000),
            "{hint:?}"
        );
    }

    #[test]
    fn uuids_are_random_version_4() {
        let a = new_uuid().expect("uuid");
        let b = new_uuid().expect("uuid");
        assert!(is_uuid_v4(&a), "{a}");
        assert!(is_uuid_v4(&b), "{b}");
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }

    #[test]
    fn uuid_text_layout() {
        assert_eq!(
            format_uuid(0x3f2a9c1e_5b7d_4e8a_9c0f_1a2b3c4d5e6f),
            "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f"
        );
        assert_eq!(format_uuid(0), "00000000-0000-0000-0000-000000000000");
        assert_eq!(
            GUID::from_u128(0x9b1c0d6e_2f4a_4c8b_a1d3_5e6f7a8b9c0d).to_u128(),
            0x9b1c0d6e_2f4a_4c8b_a1d3_5e6f7a8b9c0d
        );
    }

    #[test]
    fn random_bytes_fill_the_buffer() {
        let mut buf = [0u8; 64];
        assert_eq!(random_bytes(&mut buf), Ok(()));
        assert!(buf.iter().any(|&byte| byte != 0));
        assert_eq!(random_bytes(&mut []), Ok(()));
    }

    #[test]
    fn run_once_commands_are_checked_before_writing() {
        assert!(is_run_once_command(
            r#""C:\Program Files\MKLM\mklm-cli.exe" post-reboot"#
        ));
        assert!(is_run_once_command(r#""C:\MKLM\mklm-cli.exe""#));
        assert!(!is_run_once_command(r"C:\MKLM\mklm-cli.exe post-reboot"));
        assert!(!is_run_once_command(r#""mklm-cli.exe" post-reboot"#));
        assert!(!is_run_once_command(r#""C:\MKLM\mklm-cli.exe"post-reboot"#));
        assert!(!is_run_once_command(r#""C:\MKLM\mklm-cli.exe post-reboot"#));
        assert!(!is_run_once_command("\"C:\\a.exe\" x\0"));
        let long = format!(r#""C:\{}.exe" post-reboot"#, "a".repeat(250));
        assert!(!is_run_once_command(&long));
    }

    /// Only `"<absolute path>\mklm.exe" --tray` is ever written to the Run key (design m3 F.3).
    /// The writes themselves are checked on the real machine (T-AUTO-1), never in tests.
    #[test]
    fn autostart_commands_are_checked_before_writing() {
        assert!(is_autostart_command(
            r#""C:\Program Files\MKLM\mklm.exe" --tray"#
        ));
        assert!(is_autostart_command(r#""D:\Tools\MKLM.EXE" --tray"#));
        for bad in [
            // Not quoted, relative, or no executable.
            r"C:\Program Files\MKLM\mklm.exe --tray",
            r#""mklm.exe" --tray"#,
            r#""" --tray"#,
            // Another program, or another file name.
            r#""C:\Program Files\MKLM\mklm-cli.exe" --tray"#,
            r#""C:\Windows\System32\cmd.exe" --tray"#,
            r#""C:\MKLM\mklm.exe.bat" --tray"#,
            // Other arguments.
            r#""C:\MKLM\mklm.exe""#,
            r#""C:\MKLM\mklm.exe" --post-reboot"#,
            r#""C:\MKLM\mklm.exe" --tray --quit"#,
            r#""C:\MKLM\mklm.exe"  --tray"#,
            r#""C:\MKLM\mklm.exe" --TRAY"#,
            "\"C:\\MKLM\\mklm.exe\" --tray\0",
        ] {
            assert!(!is_autostart_command(bad), "{bad}");
        }
        let long = format!(r#""C:\{}\mklm.exe" --tray"#, "a".repeat(250));
        assert!(!is_autostart_command(&long));
    }
}
