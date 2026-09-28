//! Process identity and liveness (M2): the journal owner check of plan 2.3 and the pipe peer's
//! image check.
//!
//! No process handle is opened. A normal process's DACL grants nothing to Administrators, so
//! `OpenProcess` fails when the helper runs as another user (a standard user who typed an
//! administrator's credentials into UAC; design review S3). Everything here reads the system
//! process table instead, which works across users and integrity levels:
//! - `NtQuerySystemInformation(SystemProcessInformation)`: PID and `CreateTime` of every process;
//! - `NtQuerySystemInformation(SystemProcessIdInformation)`: the image name of one PID, as an NT
//!   path (`\Device\HarddiskVolume3\…`). Paths are compared in that NT form on both sides, never
//!   converted to drive letters.
//!
//! Both structures are defined here with `#[repr(C)]` (ntexapi.h); the `windows` crate lacks the
//! second and only has the first in an opaque form.

use std::mem::offset_of;

use mklm_core::{Liveness, ProcessIdentity};
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS, SystemProcessInformation,
};
use windows::Win32::Foundation::{FILETIME, HANDLE, STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING};
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessTimes};
use windows::core::PWSTR;

use crate::error::Error;
use crate::sys::{nt_check, win32};

/// `SystemProcessIdInformation`.
const SYSTEM_PROCESS_ID_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(88);

/// Attempts while the process table or an image name keeps outgrowing the buffer.
const MAX_ATTEMPTS: usize = 8;

/// `SYSTEM_PROCESS_ID_INFORMATION`: the caller provides the name buffer.
#[repr(C)]
struct SystemProcessIdInformation {
    process_id: HANDLE,
    image_name: UNICODE_STRING,
}

/// The fixed head of `SYSTEM_PROCESS_INFORMATION`, up to `UniqueProcessId`. Only used for its
/// field offsets; entries are parsed from the byte buffer.
#[repr(C)]
#[allow(dead_code, reason = "the kernel's layout; only some fields are read")]
struct ProcessEntryHead {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UNICODE_STRING,
    base_priority: i32,
    unique_process_id: HANDLE,
}

// The documented x64 / ARM64 offsets of the fields this module reads.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(offset_of!(ProcessEntryHead, create_time) == 32);
    assert!(offset_of!(ProcessEntryHead, unique_process_id) == 80);
};

/// This process's PID and creation time (`GetProcessTimes` on the pseudo handle; the same
/// `CreateTime` that `SystemProcessInformation` reports).
pub fn current_process_identity() -> Result<ProcessIdentity, Error> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle with full access to this process; all
    // four out pointers are valid.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    }
    .map_err(|error| win32("GetProcessTimes", &error))?;
    Ok(ProcessIdentity {
        // SAFETY: GetCurrentProcessId has no preconditions.
        pid: unsafe { GetCurrentProcessId() },
        creation_time: (u64::from(creation.dwHighDateTime) << 32)
            | u64::from(creation.dwLowDateTime),
    })
}

/// From `SystemProcessInformation`: a process with the same PID and creation time →
/// [`Liveness::Alive`]; none → [`Liveness::Dead`]; the query itself failed →
/// [`Liveness::Unknown`].
pub fn process_liveness(process: &ProcessIdentity) -> Liveness {
    match process_table() {
        Ok(table) if table.contains(&(process.pid, process.creation_time)) => Liveness::Alive,
        Ok(_) => Liveness::Dead,
        Err(_) => Liveness::Unknown,
    }
}

/// `(PID, CreateTime)` of every process.
fn process_table() -> Result<Vec<(u32, u64)>, Error> {
    const FUNCTION: &str = "NtQuerySystemInformation(SystemProcessInformation)";
    let mut size: usize = 512 * 1024;
    for _ in 0..MAX_ATTEMPTS {
        // u64 elements keep the entries 8-byte aligned, as the kernel lays them out.
        let mut buffer = vec![0u64; size.div_ceil(8)];
        let len = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
        let mut returned = 0u32;
        // SAFETY: `buffer` has `len` writable bytes; `returned` is a valid out pointer.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buffer.as_mut_ptr().cast(),
                len,
                &mut returned,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // Processes can start before the next call: leave some room.
            size = (returned as usize).max(size) + 64 * 1024;
            continue;
        }
        nt_check(FUNCTION, status)?;
        let bytes: Vec<u8> = buffer.iter().flat_map(|word| word.to_ne_bytes()).collect();
        let used = (returned as usize).min(bytes.len());
        return parse_process_table(&bytes[..used]).ok_or_else(|| Error::UnexpectedData {
            path: "SystemProcessInformation".to_string(),
        });
    }
    Err(Error::NtStatus {
        function: FUNCTION,
        status: STATUS_INFO_LENGTH_MISMATCH.0 as u32,
    })
}

/// Walks the `NextEntryOffset` chain; `None` when an entry does not fit in `bytes`.
fn parse_process_table(bytes: &[u8]) -> Option<Vec<(u32, u64)>> {
    let head = size_of::<ProcessEntryHead>();
    let pid_offset = offset_of!(ProcessEntryHead, unique_process_id);
    let time_offset = offset_of!(ProcessEntryHead, create_time);
    let mut table = Vec::new();
    let mut offset = 0usize;
    loop {
        let entry = bytes.get(offset..offset.checked_add(head)?)?;
        let next = u32::from_ne_bytes(entry.get(..4)?.try_into().ok()?) as usize;
        let create_time =
            i64::from_ne_bytes(entry.get(time_offset..time_offset + 8)?.try_into().ok()?);
        let pid_bytes = entry.get(pid_offset..pid_offset + size_of::<usize>())?;
        let pid = usize::from_ne_bytes(pid_bytes.try_into().ok()?);
        // PIDs are multiples of 4 below 2^32; a HANDLE-sized field only carries them.
        table.push((u32::try_from(pid).ok()?, create_time as u64));
        if next == 0 {
            return Some(table);
        }
        offset = offset.checked_add(next)?;
    }
}

/// NT image path of a running process (`SystemProcessIdInformation`), e.g.
/// `\Device\HarddiskVolume3\Program Files\MKLM\mklm-cli.exe`.
pub fn process_image_nt_path(pid: u32) -> Result<String, Error> {
    const FUNCTION: &str = "NtQuerySystemInformation(SystemProcessIdInformation)";
    // In bytes; UNICODE_STRING lengths are u16 and even.
    let mut capacity: u16 = 520;
    for _ in 0..MAX_ATTEMPTS {
        let mut name = vec![0u16; usize::from(capacity / 2)];
        let mut info = SystemProcessIdInformation {
            process_id: HANDLE(std::ptr::without_provenance_mut(pid as usize)),
            image_name: UNICODE_STRING {
                Length: 0,
                MaximumLength: capacity,
                Buffer: PWSTR(name.as_mut_ptr()),
            },
        };
        // SAFETY: `info` is the plain-data layout of class 88; its name buffer points at `name`,
        // which has `capacity` writable bytes and outlives the call.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_ID_INFORMATION,
                (&raw mut info).cast(),
                size_of::<SystemProcessIdInformation>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // `MaximumLength` now holds the size the name needs.
            let needed = info.image_name.MaximumLength;
            capacity = if needed > capacity {
                needed.saturating_add(1) & !1
            } else {
                capacity.saturating_mul(2) & !1
            };
            continue;
        }
        nt_check(FUNCTION, status)?;
        let units = name
            .get(..usize::from(info.image_name.Length / 2))
            .unwrap_or_default();
        return String::from_utf16(units)
            .ok()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| Error::UnexpectedData {
                path: format!("image name of process {pid}"),
            });
    }
    Err(Error::NtStatus {
        function: FUNCTION,
        status: STATUS_INFO_LENGTH_MISMATCH.0 as u32,
    })
}

/// True when `pid`'s image is in the same directory as this process's image, both taken from
/// [`process_image_nt_path`] and compared case-insensitively (the helper's check of its caller,
/// design E.2).
pub fn same_image_directory(pid: u32) -> Result<bool, Error> {
    // SAFETY: GetCurrentProcessId has no preconditions.
    let own = process_image_nt_path(unsafe { GetCurrentProcessId() })?;
    let other = process_image_nt_path(pid)?;
    Ok(match (image_directory(&own), image_directory(&other)) {
        (Some(own), Some(other)) => equal_ignoring_case(own, other),
        _ => false,
    })
}

// ---- M5b additions (design m5b H.3, D.7, D.8; WP-H) ----

/// PID → identity with its creation time (SystemProcessInformation); `None` if gone.
#[allow(unused_variables)] // Skeleton (M5b)
pub fn process_identity(pid: u32) -> Result<Option<ProcessIdentity>, Error> {
    Err(skeleton("process_identity (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// NT path of a file (`GetFinalPathNameByHandleW(VOLUME_NAME_NT)`), for comparing with
/// `process_image_nt_path`.
#[allow(unused_variables)] // Skeleton (M5b)
pub fn file_nt_path(path: &std::path::Path) -> Result<String, Error> {
    Err(skeleton("file_nt_path (m5b skeleton)")) // Skeleton (M5b): WP-H
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProcess {
    pub identity: ProcessIdentity,
    /// Index into the `nt_paths` argument.
    pub path_index: usize,
    pub session_id: u32,
}

/// Every process whose image NT path equals one of `nt_paths` (case-insensitive).
#[allow(unused_variables)] // Skeleton (M5b)
pub fn processes_with_images(nt_paths: &[String]) -> Result<Vec<ImageProcess>, Error> {
    Err(skeleton("processes_with_images (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// One row of `WTSEnumerateProcessesW(WTS_CURRENT_SERVER_HANDLE)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessUser {
    pub session_id: u32,
    /// `pUserSid` as `S-1-…` text (`ConvertSidToStringSidW`); `None` when absent.
    pub user_sid: Option<String>,
}

/// PID → session and user SID of every process (design m5b D.8 step 2; FIX-VERIFICATION-5).
/// Whether `pUserSid` is filled for other users' processes when called from an elevated,
/// non-SYSTEM process is unverified (design m5b I.12).
pub fn process_users() -> Result<std::collections::HashMap<u32, ProcessUser>, Error> {
    Err(skeleton("process_users (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// Polls `process_liveness` every 250 ms; true when all are gone within `timeout`.
#[allow(unused_variables)] // Skeleton (M5b)
pub fn wait_for_exit(
    processes: &[ProcessIdentity],
    timeout: std::time::Duration,
) -> Result<bool, Error> {
    Err(skeleton("wait_for_exit (m5b skeleton)")) // Skeleton (M5b): WP-H
}

/// What the WP-0 skeleton returns (design m5b G.2): `ERROR_NOT_SUPPORTED`.
fn skeleton(function: &'static str) -> Error {
    Error::Win32 { function, code: 50 }
}

/// Directory part of an NT image path (without the last `\`); `None` without one.
fn image_directory(path: &str) -> Option<&str> {
    path.rsplit_once('\\')
        .map(|(directory, _)| directory)
        .filter(|directory| !directory.is_empty())
}

/// Ordinal comparison ignoring case, as the file system compares names.
fn equal_ignoring_case(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    // SAFETY: both slices are valid UTF-16 buffers with their lengths.
    let result = unsafe { CompareStringOrdinal(&a, &b, true) };
    result == CSTR_EQUAL
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};

    use super::*;

    fn creation_time(child: &std::process::Child) -> u64 {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the child's process handle is open (owned by `child`); out pointers are valid.
        unsafe {
            GetProcessTimes(
                HANDLE(child.as_raw_handle()),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        }
        .expect("GetProcessTimes of the child");
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime)
    }

    #[test]
    fn this_process_is_alive() {
        let me = current_process_identity().expect("own identity");
        assert_eq!(me.pid, std::process::id());
        assert_eq!(process_liveness(&me), Liveness::Alive);
        // Same PID, other creation time: a reused PID is not the same process.
        let impostor = ProcessIdentity {
            creation_time: me.creation_time + 1,
            ..me
        };
        assert_eq!(process_liveness(&impostor), Liveness::Dead);
    }

    #[test]
    fn an_exited_child_is_dead() {
        // The test executable itself, listing its tests: starts and exits on its own.
        let exe = std::env::current_exe().expect("test executable");
        let mut child = Command::new(exe)
            .args(["--list", "--exact", "no such test"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start child");
        let identity = ProcessIdentity {
            pid: child.id(),
            creation_time: creation_time(&child),
        };
        child.wait().expect("child exits");
        assert_eq!(process_liveness(&identity), Liveness::Dead);
    }

    #[test]
    fn own_image_path_is_the_test_executable() {
        let path = process_image_nt_path(std::process::id()).expect("own image path");
        let exe = std::env::current_exe().expect("test executable");
        let file_name = exe
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name");
        assert!(path.starts_with(r"\Device\"), "{path}");
        assert!(
            path.to_ascii_lowercase()
                .ends_with(&format!(r"\{}", file_name.to_ascii_lowercase())),
            "{path} vs {file_name}"
        );
        assert_eq!(same_image_directory(std::process::id()), Ok(true));
    }

    #[test]
    fn other_directories_and_missing_processes() {
        // PID 4 is the System process, whose image is not in the test's directory (and may have
        // no path at all).
        assert_ne!(same_image_directory(4), Ok(true));
        // PIDs are multiples of 4, so this one never exists.
        assert!(process_image_nt_path(0xFFFF_FFF3).is_err());
    }

    #[test]
    fn directory_comparison() {
        assert_eq!(
            image_directory(r"\Device\HarddiskVolume3\MKLM\mklm-cli.exe"),
            Some(r"\Device\HarddiskVolume3\MKLM")
        );
        assert_eq!(image_directory("mklm-cli.exe"), None);
        assert_eq!(image_directory(r"\mklm-cli.exe"), None);
        assert!(equal_ignoring_case(
            r"\Device\HarddiskVolume3\Program Files\MKLM",
            r"\device\harddiskvolume3\PROGRAM FILES\mklm"
        ));
        assert!(!equal_ignoring_case(
            r"\Device\HarddiskVolume3\MKLM",
            r"\Device\HarddiskVolume4\MKLM"
        ));
    }

    #[test]
    fn process_table_parsing_stops_at_bad_offsets() {
        let head = size_of::<ProcessEntryHead>();
        let mut bytes = vec![0u8; head * 2];
        // First entry: next = head, pid 8; second: last, pid 12.
        bytes[..4].copy_from_slice(&(head as u32).to_ne_bytes());
        let pid = offset_of!(ProcessEntryHead, unique_process_id);
        bytes[pid..pid + size_of::<usize>()].copy_from_slice(&8usize.to_ne_bytes());
        bytes[head + pid..head + pid + size_of::<usize>()].copy_from_slice(&12usize.to_ne_bytes());
        assert_eq!(parse_process_table(&bytes), Some(vec![(8, 0), (12, 0)]));
        // A next offset past the end.
        bytes[head..head + 4].copy_from_slice(&(head as u32).to_ne_bytes());
        assert_eq!(parse_process_table(&bytes), None);
        assert_eq!(parse_process_table(&[]), None);
    }
}
