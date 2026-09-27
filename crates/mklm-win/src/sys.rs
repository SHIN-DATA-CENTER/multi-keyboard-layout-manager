//! Small helpers shared by the M2 modules: Win32 error mapping, owned handles, wait timeouts and
//! UTF-16 paths.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use windows::Win32::Foundation::{GetLastError, HANDLE, NTSTATUS};

use crate::error::{Error, win32_code};

/// A failed Win32 call reported through a `windows::core::Error` (which carries `GetLastError`).
pub(crate) fn win32(function: &'static str, error: &windows::core::Error) -> Error {
    Error::Win32 {
        function,
        code: win32_code(error),
    }
}

/// A failed Win32 call that reports its error through `GetLastError` only.
pub(crate) fn last_error(function: &'static str) -> Error {
    // SAFETY: GetLastError has no preconditions.
    let code = unsafe { GetLastError() };
    Error::Win32 {
        function,
        code: code.0,
    }
}

/// `Ok` for a success `NTSTATUS`, else [`Error::NtStatus`].
pub(crate) fn nt_check(function: &'static str, status: NTSTATUS) -> Result<(), Error> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::NtStatus {
            function,
            status: status.0 as u32,
        })
    }
}

/// Milliseconds for a Win32 wait, capped just below `INFINITE` so that a long timeout never turns
/// into "wait forever".
pub(crate) fn wait_millis(timeout: Duration) -> u32 {
    u32::try_from(timeout.as_millis()).map_or(u32::MAX - 1, |ms| ms.min(u32::MAX - 1))
}

/// Takes ownership of a handle a Win32 call just returned.
///
/// # Safety
///
/// `handle` must be a valid, open kernel handle that the caller owns and that `CloseHandle` may
/// close (not a pseudo handle, not `INVALID_HANDLE_VALUE`).
pub(crate) unsafe fn own(handle: HANDLE) -> OwnedHandle {
    // SAFETY: guaranteed by the caller.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}

/// The raw `HANDLE` of an owned handle, for passing to Win32 calls while the owner lives.
pub(crate) fn raw(handle: &impl AsRawHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

/// NUL-terminated UTF-16 of an OS string (a path).
pub(crate) fn wide_os(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_never_become_infinite() {
        assert_eq!(wait_millis(Duration::ZERO), 0);
        assert_eq!(wait_millis(Duration::from_millis(1500)), 1500);
        assert_eq!(wait_millis(Duration::from_secs(u64::MAX)), u32::MAX - 1);
        assert_eq!(
            wait_millis(Duration::from_millis(u64::from(u32::MAX))),
            u32::MAX - 1
        );
    }

    #[test]
    fn wide_paths_end_in_nul() {
        assert_eq!(wide_os(OsStr::new("C:\\a")), vec![67, 58, 92, 97, 0]);
    }
}
