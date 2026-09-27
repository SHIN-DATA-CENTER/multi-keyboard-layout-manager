//! Machine- and session-level actions of M2: boot ID, randomness, PC restart and the post-reboot
//! RunOnce entry.

// Skeleton (M2): the bodies below are `todo!()`; remove this allow once they are implemented.
#![allow(unused_variables)]

use mklm_core::BootId;

use crate::error::Error;

/// Name of the value MKLM puts under `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce`.
pub const RUN_ONCE_VALUE: &str = "SHINDATACENTER.MKLM.PostReboot";

/// The current boot: `NtQuerySystemInformation(SystemBootEnvironmentInformation = 90)`
/// `.BootIdentifier`, a GUID the loader creates for every boot. Unlike the kernel boot time it
/// does not move with clock corrections, resume from sleep or `w32tm /resync` (design review C2).
/// Required properties, verified on the machine by H.2 R9/R10 before M2 is done: unchanged across
/// sleep, hibernation, a Fast Startup "shutdown" and a clock resync; changed by a full restart.
/// If the Fast Startup property fails, M2 falls back to `KUSER_SHARED_DATA.BootId` (a per-boot
/// counter) with the same tests. `SYSTEM_BOOT_ENVIRONMENT_INFORMATION` is defined in this module
/// with `#[repr(C)]` if the `windows` crate lacks it.
pub fn boot_id() -> Result<BootId, Error> {
    todo!("M2")
}

/// Kernel boot time minus `BootTimeBias`
/// (`NtQuerySystemInformation(SystemTimeOfDayInformation)`), FILETIME units. A diagnostic for the
/// journal's history only; never used to decide anything.
pub fn boot_time_hint() -> Result<u64, Error> {
    todo!("M2")
}

/// Fills `buf` from `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)`.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), Error> {
    todo!("M2")
}

/// A random (version 4) UUID, lower-case, hyphenated, without braces.
pub fn new_uuid() -> Result<String, Error> {
    todo!("M2")
}

/// Restarts Windows (plan 3.6): enables `SE_SHUTDOWN_NAME` in this process's token, then
/// `InitiateShutdownW(NULL, NULL, 0, SHUTDOWN_RESTART | SHUTDOWN_RESTARTAPPS,
/// SHTDN_REASON_MAJOR_OPERATINGSYSTEM | SHTDN_REASON_MINOR_RECONFIG | SHTDN_REASON_FLAG_PLANNED)`.
/// Works unelevated (Users hold the privilege on client editions). Never a shutdown: Fast Startup
/// would keep the drivers.
pub fn restart_pc() -> Result<(), Error> {
    todo!("M2")
}

/// Registers `command_line` (absolute, quoted executable path plus arguments) as
/// [`RUN_ONCE_VALUE`] in the current user's RunOnce key. Called by the unelevated GUI/CLI only;
/// the helper never touches HKCU (plan 2.2).
pub fn register_post_reboot(command_line: &str) -> Result<(), Error> {
    todo!("M2")
}

/// Removes [`RUN_ONCE_VALUE`] (missing is fine).
pub fn unregister_post_reboot() -> Result<(), Error> {
    todo!("M2")
}
