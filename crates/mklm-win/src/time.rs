//! Journal timestamps in local time (design m3 G.1; the M2 real-machine test R4 found them shown
//! in UTC). The journal keeps UTC milliseconds (`mklm_core::Timestamp`); only the display
//! converts, with the time zone rules that applied at that moment
//! (`SystemTimeToTzSpecificLocalTimeEx` with the current dynamic time zone, which knows past
//! daylight saving changes).

use mklm_core::Timestamp;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTimeEx,
};

use crate::error::Error;
use crate::sys::win32;

/// Milliseconds between 1601-01-01 (FILETIME) and 1970-01-01 (Unix).
const EPOCH_DIFFERENCE_MS: u64 = 11_644_473_600_000;

/// A local date and time with its offset from UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    /// Local minus UTC, in minutes (e.g. +540 for Japan).
    pub utc_offset_minutes: i32,
}

impl LocalTime {
    /// `YYYY-MM-DD HH:MM:SS +HH:MM`.
    pub fn to_iso_text(&self) -> String {
        let sign = if self.utc_offset_minutes < 0 {
            '-'
        } else {
            '+'
        };
        let offset = self.utc_offset_minutes.unsigned_abs();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {sign}{:02}:{:02}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            offset / 60,
            offset % 60
        )
    }
}

fn filetime(ticks: u64) -> FILETIME {
    FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    }
}

fn ticks(filetime: FILETIME) -> u64 {
    (u64::from(filetime.dwHighDateTime) << 32) | u64::from(filetime.dwLowDateTime)
}

/// Converts a journal timestamp to local time.
pub fn local_time(at: Timestamp) -> Result<LocalTime, Error> {
    let utc_ticks =
        at.0.checked_add(EPOCH_DIFFERENCE_MS)
            .and_then(|ms| ms.checked_mul(10_000))
            .ok_or(Error::Win32 {
                function: "FileTimeToSystemTime",
                code: 87, // ERROR_INVALID_PARAMETER
            })?;
    let utc_file = filetime(utc_ticks);
    let mut utc = SYSTEMTIME::default();
    // SAFETY: both pointers refer to live locals of the right types.
    unsafe { FileTimeToSystemTime(&utc_file, &mut utc) }
        .map_err(|error| win32("FileTimeToSystemTime", &error))?;
    let mut local = SYSTEMTIME::default();
    // SAFETY: no time zone (the current one); both pointers refer to live locals.
    unsafe { SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local) }
        .map_err(|error| win32("SystemTimeToTzSpecificLocalTimeEx", &error))?;
    let mut local_file = FILETIME::default();
    // SAFETY: both pointers refer to live locals of the right types.
    unsafe { SystemTimeToFileTime(&local, &mut local_file) }
        .map_err(|error| win32("SystemTimeToFileTime", &error))?;
    // Both FILETIMEs have millisecond precision here, so the difference is whole minutes.
    let difference = ticks(local_file) as i128 - utc_ticks as i128;
    Ok(LocalTime {
        year: local.wYear,
        month: local.wMonth,
        day: local.wDay,
        hour: local.wHour,
        minute: local.wMinute,
        second: local.wSecond,
        utc_offset_minutes: (difference / 600_000_000) as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_with_the_machines_zone() {
        // 2026-09-27 13:22:05 UTC. The result depends on the machine's time zone; the offset is
        // whole minutes within ±14 h and the text has the fixed shape.
        let local = local_time(Timestamp(1_790_515_325_000)).unwrap();
        assert!(local.utc_offset_minutes.abs() <= 14 * 60);
        let text = local.to_iso_text();
        assert_eq!(text.len(), "2026-09-27 22:22:05 +09:00".len(), "{text}");
        assert!(text.starts_with("2026-09-2"), "{text}");
    }

    #[test]
    fn iso_text() {
        let local = LocalTime {
            year: 2026,
            month: 9,
            day: 27,
            hour: 22,
            minute: 22,
            second: 5,
            utc_offset_minutes: -210,
        };
        assert_eq!(local.to_iso_text(), "2026-09-27 22:22:05 -03:30");
    }
}
